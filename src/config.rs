use crate::i18n::Language;
use crate::utils::atomic;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

/// AI 提供方。非法取值在配置反序列化阶段就报错，而不是等查询时才提示。
///
/// 每个预设只固定「可执行文件 + 提示词之前的固定参数」，提示词一律追加为
/// 最后一个参数 —— `claude -p`、`codex exec`、`gemini -p`、`llm`、`aichat`
/// 都是这个约定。需要别的形态就用 [`AppConfig::ai_command`] 自定义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AiProvider {
    /// 关闭 AI 回退，未命中笔记时只给建议。
    None,
    #[default]
    Claude,
    Codex,
    Gemini,
}

impl AiProvider {
    pub fn code(self) -> &'static str {
        match self {
            AiProvider::None => "none",
            AiProvider::Claude => "claude",
            AiProvider::Codex => "codex",
            AiProvider::Gemini => "gemini",
        }
    }

    /// 预设的可执行文件与固定参数；`None` 表示禁用。
    fn preset(self) -> Option<(&'static str, &'static str)> {
        match self {
            AiProvider::None => None,
            AiProvider::Claude => Some(("claude", "-p --output-format text")),
            AiProvider::Codex => Some(("codex", "exec")),
            AiProvider::Gemini => Some(("gemini", "-p")),
        }
    }
}

/// 本次运行实际要执行的 AI 命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiInvocation {
    /// 不对接任何 AI。
    Disabled,
    /// 命令行模板；提示词追加为最后一个参数，也可用 `{prompt}` 指定位置。
    Command(String),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub ask_before_ai: bool,
    pub auto_save_ai: bool,
    pub ask_before_save: bool,
    pub ai_note_language: String,
    pub ai_provider: AiProvider,
    /// 自定义 AI 命令行，优先级高于 `ai_provider` 预设。
    /// 例如 `"llm -m gpt-4o"`、`"ollama run qwen2.5"`。
    pub ai_command: Option<String>,
    /// AI 生成的最长等待秒数；`0` 表示不限制。
    pub ai_timeout_seconds: u64,
    pub editor: Option<String>,
    pub language: Option<Language>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            ask_before_ai: true,
            auto_save_ai: true,
            ask_before_save: false,
            ai_note_language: "zh-CN".to_string(),
            ai_provider: AiProvider::Claude,
            ai_command: None,
            ai_timeout_seconds: 180,
            editor: None,
            language: None,
        }
    }
}

impl AppConfig {
    pub fn load(lang: Language) -> Result<Self> {
        let path = match config_path(lang) {
            Ok(path) => path,
            // 拿不到配置目录时按「没有配置」处理：需要写入或定位笔记目录时
            // 会由 save / resolve_notes_dir 报出真正的原因。
            Err(_) => return Ok(Self::default()),
        };
        load_from(&path, lang)
    }

    pub fn save(&self, lang: Language) -> Result<()> {
        self.save_to(&config_path(lang)?, lang)
    }

    /// [`Self::save`] 的可注入版本；参数化路径是为了脱离进程环境测试。
    fn save_to(&self, path: &Path, lang: Language) -> Result<()> {
        if let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .with_context(|| lang.config_dir_create_failed(&parent.display().to_string()))?;
        }

        let raw = toml::to_string_pretty(self).context(lang.config_serialize_failed())?;
        let target = path.display().to_string();
        atomic::replace_file(path, &raw, ".config-", &|| {
            lang.config_write_failed(&target)
        })
    }

    /// 尚未选择过界面语言，视为首次运行。
    pub fn is_first_run(&self) -> bool {
        self.language.is_none()
    }

    pub fn language(&self) -> Language {
        self.language.unwrap_or_default()
    }

    pub fn editor_spec(&self) -> Option<&str> {
        self.editor
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    /// 解析出本次实际要执行的 AI 命令。
    ///
    /// 优先级：`ai_command` > `GG_AI_BIN`（`GG_CLAUDE_BIN` 为兼容旧版保留）>
    /// `ai_provider` 预设。`GG_AI_BIN` 只替换可执行文件，保留预设参数。
    pub fn ai_invocation(&self) -> AiInvocation {
        self.ai_invocation_from(ai_bin_override())
    }

    /// [`Self::ai_invocation`] 的可注入版本；参数化是为了脱离进程环境测试。
    fn ai_invocation_from(&self, bin_override: Option<String>) -> AiInvocation {
        if let Some(custom) = self
            .ai_command
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return AiInvocation::Command(custom.to_string());
        }

        let Some((bin, preset_args)) = self.ai_provider.preset() else {
            return AiInvocation::Disabled;
        };

        let bin = bin_override.unwrap_or_else(|| bin.to_string());
        AiInvocation::Command(format!("{bin} {preset_args}"))
    }

    /// `ai_timeout_seconds = 0` 表示不限制等待时长。
    pub fn ai_timeout(&self) -> Option<std::time::Duration> {
        match self.ai_timeout_seconds {
            0 => None,
            seconds => Some(std::time::Duration::from_secs(seconds)),
        }
    }
}

/// 从指定路径读取配置。
///
/// 参数化路径是为了脱离进程环境测试文案与行为（`load` 自身依赖
/// `GG_CONFIG_DIR` 等环境变量，不适合在并行测试里改）。
fn load_from(path: &Path, lang: Language) -> Result<AppConfig> {
    if !path.exists() {
        return Ok(AppConfig::default());
    }

    let raw = fs::read_to_string(path)
        .with_context(|| lang.config_read_failed(&path.display().to_string()))?;
    let config: AppConfig = toml::from_str(&raw)
        .with_context(|| lang.config_parse_failed(&path.display().to_string()))?;
    Ok(config)
}

/// 在 clap 解析之前探测界面语言。
///
/// 用途有两处：本地化 `--help` / `--version`，以及本地化**配置自身**的报错
/// —— 后者存在引导问题：语言写在配置里，配置解析失败时就读不到它。
///
/// 语种优先级：命令行 `--lang` > 配置文件里的 `language` > 默认（中文）。
/// 配置文件用宽松解析单独取 `language`，所以即使配置里另有拼错的键（整份配置
/// 会解析失败），报错仍然用得上用户设定的语言。
///
/// 这里刻意吞掉其余错误：真正的配置错误会由 [`AppConfig::load`] 在 `app::run`
/// 中完整报出，此处只决定文案语种。
pub fn peek_language() -> Language {
    if let Some(lang) = language_from_args(env::args().skip(1)) {
        return lang;
    }
    language_from_config().unwrap_or_default()
}

/// 从命令行参数里找 `--lang <值>` / `--lang=<值>`。
fn language_from_args(args: impl IntoIterator<Item = String>) -> Option<Language> {
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix("--lang=") {
            if let Some(lang) = Language::parse(value) {
                return Some(lang);
            }
        } else if arg == "--lang" {
            if let Some(value) = args.next() {
                if let Some(lang) = Language::parse(&value) {
                    return Some(lang);
                }
            }
        }
    }
    None
}

fn language_from_config() -> Option<Language> {
    // 路径拿不到时只影响帮助文案的语种，用默认语言即可。
    let path = config_path(Language::default()).ok()?;
    language_from_config_at(&path)
}

/// 宽松地只取配置里的 `language` 字段，忽略其它键与解析错误。
fn language_from_config_at(path: &Path) -> Option<Language> {
    #[derive(Deserialize)]
    struct Peek {
        language: Option<Language>,
    }

    let raw = fs::read_to_string(path).ok()?;
    toml::from_str::<Peek>(&raw).ok()?.language
}

/// gg 的应用目录：`config.toml`、`notes/` 与 `AGENTS.md` 都在这里。
fn app_dir(lang: Language) -> Result<PathBuf> {
    Ok(config_root_dir(lang)?.join("gg"))
}

pub fn config_path(lang: Language) -> Result<PathBuf> {
    Ok(app_dir(lang)?.join("config.toml"))
}

pub fn default_notes_dir(lang: Language) -> Result<PathBuf> {
    Ok(app_dir(lang)?.join("notes"))
}

/// AI 生成笔记用的提示词文件；不存在或为空时回落内置默认。
pub fn prompt_path(lang: Language) -> Result<PathBuf> {
    Ok(app_dir(lang)?.join("AGENTS.md"))
}

pub fn resolve_notes_dir(cli_override: Option<PathBuf>, lang: Language) -> Result<PathBuf> {
    resolve_notes_dir_with(
        cli_override,
        env::var_os("GG_NOTES_DIR"),
        system_config_dir(),
        lang,
    )
}

pub(crate) fn resolve_notes_dir_with(
    cli_override: Option<PathBuf>,
    env_override: Option<OsString>,
    config_root: Option<PathBuf>,
    lang: Language,
) -> Result<PathBuf> {
    if let Some(path) = cli_override {
        return Ok(path);
    }

    if let Some(path) = env_override {
        return Ok(PathBuf::from(path));
    }

    let root = config_root.context(lang.config_dir_unknown())?;
    Ok(root.join("gg").join("notes"))
}

fn config_root_dir(lang: Language) -> Result<PathBuf> {
    system_config_dir().context(lang.config_dir_unknown())
}

/// 系统配置根目录，按以下优先级取值：
///
/// 1. `GG_CONFIG_DIR` —— 显式覆盖，便于可移植部署与测试
/// 2. `dirs::config_dir()` —— 平台标准位置
/// 3. 环境变量兜底 —— 见 [`env_config_dir`]
///
/// 第 3 步**不是多余的保险**：Windows 上 `dirs` 走 `SHGetKnownFolderPath`，
/// 完全不看 `APPDATA`，在受限环境（CI 容器、服务账户）里会直接失败，导致
/// `gg` 在没有 `--notes-dir` 时整个不可用；而 Linux 上 `dirs` 读的是
/// `XDG_CONFIG_HOME`，所以本地一路绿灯，问题只在别的平台暴露。
fn system_config_dir() -> Option<PathBuf> {
    config_dir_override()
        .or_else(dirs::config_dir)
        .or_else(env_config_dir)
}

/// `GG_CONFIG_DIR` 显式覆盖，优先级最高。
fn config_dir_override() -> Option<PathBuf> {
    config_dir_override_from(|key| env::var_os(key))
}

fn config_dir_override_from(mut lookup: impl FnMut(&str) -> Option<OsString>) -> Option<PathBuf> {
    // 刻意**不**要求绝对路径：这是用户显式写的值，静默忽略比「按相对路径处理」
    // 更糟（Windows 上 `/custom` 这类写法 is_absolute() 为假，会被无声丢弃）。
    // 需要校验绝对路径的是下面的环境变量兜底 —— 那里的 XDG 规范本就要求绝对路径。
    lookup("GG_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// `dirs` 拿不到时的兜底，读的是语义相同的环境变量。
fn env_config_dir() -> Option<PathBuf> {
    env_config_dir_from(|key| env::var_os(key))
}

/// 从给定的查找函数解析兜底目录；参数化是为了脱离进程环境做测试。
fn env_config_dir_from(mut lookup: impl FnMut(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute =
        |value: Option<OsString>| value.map(PathBuf::from).filter(|path| path.is_absolute());

    // 用 cfg!（编译期常量）而不是 #[cfg] 分块：后者会让「哪一块是尾表达式」
    // 随平台变化，也人为地制造出只在一个平台存在的代码路径。
    if cfg!(windows) {
        return absolute(lookup("APPDATA"));
    }
    absolute(lookup("XDG_CONFIG_HOME"))
        .or_else(|| absolute(lookup("HOME")).map(|home| home.join(".config")))
}

fn ai_bin_override() -> Option<String> {
    ["GG_AI_BIN", "GG_CLAUDE_BIN"].iter().find_map(|key| {
        env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_plan() {
        let cfg: AppConfig = toml::from_str("").expect("空 toml 应能反序列化");
        assert!(cfg.ask_before_ai);
        assert!(cfg.auto_save_ai);
        assert!(!cfg.ask_before_save);
        assert_eq!(cfg.ai_note_language, "zh-CN");
        assert_eq!(cfg.ai_provider, AiProvider::Claude);
        assert!(cfg.editor.is_none());
        assert!(cfg.language.is_none());
        assert!(cfg.is_first_run());
    }

    #[test]
    fn custom_toml_overrides_defaults() {
        let raw = r#"
ask_before_ai = false
auto_save_ai = false
ask_before_save = true
ai_note_language = "en"
ai_provider = "claude"
editor = "vim"
language = "zh"
"#;
        let cfg: AppConfig = toml::from_str(raw).expect("合法配置");

        assert!(!cfg.ask_before_ai);
        assert!(!cfg.auto_save_ai);
        assert!(cfg.ask_before_save);
        assert_eq!(cfg.ai_note_language, "en");
        assert_eq!(cfg.ai_provider, AiProvider::Claude);
        assert_eq!(cfg.editor_spec(), Some("vim"));
        assert_eq!(cfg.language(), Language::Zh);
        assert!(!cfg.is_first_run());
    }

    #[test]
    fn blank_editor_is_treated_as_unset() {
        let cfg: AppConfig = toml::from_str("editor = \"   \"").expect("合法配置");
        assert_eq!(cfg.editor_spec(), None);
        assert!(cfg.editor.is_some());
    }

    /// 预设表、`GG_AI_BIN` 覆盖、自定义命令与关闭，全部在这里用纯数据覆盖。
    ///
    /// 放在单测而不是集成测试里：集成测试只能靠假工具「录制自己的 argv」来
    /// 观察参数，而 Windows 上经 cmd 传递多行提示词时那段批处理写法本身就
    /// 不可靠（提示词含换行，`echo %*` 会被截断）。拼接逻辑是纯函数，直接测。
    #[test]
    fn ai_invocation_builds_the_expected_command_line() {
        let with_provider = |provider| AppConfig {
            ai_provider: provider,
            ..AppConfig::default()
        };

        assert_eq!(
            with_provider(AiProvider::Claude).ai_invocation_from(None),
            AiInvocation::Command("claude -p --output-format text".to_string())
        );
        assert_eq!(
            with_provider(AiProvider::Codex).ai_invocation_from(None),
            AiInvocation::Command("codex exec".to_string())
        );
        assert_eq!(
            with_provider(AiProvider::Gemini).ai_invocation_from(None),
            AiInvocation::Command("gemini -p".to_string())
        );
        assert_eq!(
            with_provider(AiProvider::None).ai_invocation_from(None),
            AiInvocation::Disabled
        );

        // GG_AI_BIN 只换可执行文件，预设参数保留
        assert_eq!(
            with_provider(AiProvider::Claude).ai_invocation_from(Some("/opt/claude".to_string())),
            AiInvocation::Command("/opt/claude -p --output-format text".to_string())
        );

        // ai_command 优先级最高，连 none 也压得住
        let custom = AppConfig {
            ai_provider: AiProvider::None,
            ai_command: Some("llm -m gpt-4o".to_string()),
            ..AppConfig::default()
        };
        assert_eq!(
            custom.ai_invocation_from(None),
            AiInvocation::Command("llm -m gpt-4o".to_string())
        );

        // 空白的 ai_command 视为未设置
        let blank = AppConfig {
            ai_command: Some("   ".to_string()),
            ..AppConfig::default()
        };
        assert_eq!(
            blank.ai_invocation_from(None),
            AiInvocation::Command("claude -p --output-format text".to_string())
        );
    }

    #[test]
    fn ai_timeout_defaults_to_180s_and_zero_disables_it() {
        use std::time::Duration;

        assert_eq!(
            AppConfig::default().ai_timeout(),
            Some(Duration::from_secs(180))
        );

        let cfg: AppConfig = toml::from_str("ai_timeout_seconds = 0").expect("合法配置");
        assert_eq!(cfg.ai_timeout(), None, "0 应表示不限制");

        let cfg: AppConfig = toml::from_str("ai_timeout_seconds = 30").expect("合法配置");
        assert_eq!(cfg.ai_timeout(), Some(Duration::from_secs(30)));
    }

    /// 键名拼错必须报错，而不是被当成默认值静默忽略 —— README 承诺
    /// 「取值写错会在启动时直接报错」，而拼错键名正是最常见的写错形式。
    #[test]
    fn unknown_keys_are_rejected_instead_of_silently_ignored() {
        let err =
            toml::from_str::<AppConfig>("ask_befor_ai = false").expect_err("拼错的键名必须报错");
        let msg = format!("{err}");
        assert!(msg.contains("ask_befor_ai"), "实际信息: {msg}");
    }

    #[test]
    fn unknown_provider_is_rejected_at_parse_time() {
        let err = toml::from_str::<AppConfig>("ai_provider = \"openai\"")
            .expect_err("未支持的 provider 必须报错");
        let msg = format!("{err}");
        assert!(msg.contains("openai"), "实际信息: {msg}");
    }

    #[test]
    fn config_round_trips_through_toml() {
        let cfg = AppConfig {
            language: Some(Language::En),
            editor: Some("hx".to_string()),
            ..AppConfig::default()
        };
        let raw = toml::to_string_pretty(&cfg).expect("序列化");
        let parsed: AppConfig = toml::from_str(&raw).expect("反序列化");
        assert_eq!(parsed, cfg);
    }

    /// 原子写入：旧配置被完整替换，且不留临时文件。
    ///
    /// 直接 `fs::write` 会先截断再写，中断即得到半截 TOML；而配置解析是严格的，
    /// 半截文件会让 `gg` 完全无法启动。这里钉住「要么旧、要么新」的语义。
    #[test]
    fn save_replaces_the_config_and_leaves_no_temp_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("gg");
        fs::create_dir_all(&dir).expect("建目录");
        let path = dir.join("config.toml");
        fs::write(&path, "language = \"en\"\n").expect("预置旧配置");

        let cfg = AppConfig {
            language: Some(Language::Zh),
            editor: Some("hx".to_string()),
            ..AppConfig::default()
        };
        cfg.save_to(&path, Language::Zh).expect("保存成功");

        let reloaded = load_from(&path, Language::Zh).expect("重新读取");
        assert_eq!(reloaded, cfg, "新配置应完整覆盖旧配置");

        let leftovers: Vec<String> = fs::read_dir(&dir)
            .expect("读目录")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name != "config.toml")
            .collect();
        assert!(leftovers.is_empty(), "不应留下临时文件: {leftovers:?}");
    }

    #[test]
    fn save_creates_the_config_directory_on_demand() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("brand-new").join("config.toml");

        AppConfig::default()
            .save_to(&path, Language::Zh)
            .expect("应创建父目录并写入");
        assert!(path.is_file());
    }

    /// 配置文件是 0600。
    ///
    /// 这条与笔记共享同一策略（`gg` 创建的文件一律属主可读写），因此两边各钉一次：
    /// 以后若有人把某一侧放宽，会立刻在测试里暴露。
    #[cfg(unix)]
    #[test]
    fn saved_config_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("config.toml");

        AppConfig::default()
            .save_to(&path, Language::Zh)
            .expect("保存成功");

        let mode = fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "配置文件应为 0600，实际 {mode:o}");
    }

    /// 写入失败必须报错，且不得在目标位置留下半成品。
    #[test]
    fn failed_save_reports_an_error_and_writes_nothing() {
        let temp = tempfile::tempdir().expect("tempdir");
        // 父目录的位置被普通文件占住，create_dir_all 必然失败。
        let blocker = temp.path().join("blocked");
        fs::write(&blocker, "not a directory").expect("占位文件");

        let err = AppConfig::default()
            .save_to(&blocker.join("config.toml"), Language::Zh)
            .expect_err("父目录建不出来时必须报错");

        assert!(format!("{err:#}").contains("无法创建配置目录"), "{err:#}");
        assert!(blocker.is_file(), "占位文件不应被改动");
    }

    /// `dirs` 失败时兜底必须能拿到配置目录，否则没有 `--notes-dir` 的调用会
    /// 整体失败（Windows 上 `SHGetKnownFolderPath` 在受限环境里可能失效）。
    #[test]
    fn env_config_dir_uses_the_platform_key() {
        if cfg!(windows) {
            let appdata = |key: &str| {
                (key == "APPDATA").then(|| OsString::from(r"C:\Users\x\AppData\Roaming"))
            };
            assert_eq!(
                env_config_dir_from(appdata),
                Some(PathBuf::from(r"C:\Users\x\AppData\Roaming"))
            );
            assert_eq!(env_config_dir_from(|_| None), None);
            return;
        }

        let both = |key: &str| match key {
            "XDG_CONFIG_HOME" => Some(OsString::from("/custom/config")),
            "HOME" => Some(OsString::from("/home/someone")),
            _ => None,
        };
        assert_eq!(
            env_config_dir_from(both),
            Some(PathBuf::from("/custom/config")),
            "XDG_CONFIG_HOME 应优先"
        );

        let only_home = |key: &str| match key {
            "HOME" => Some(OsString::from("/home/someone")),
            _ => None,
        };
        assert_eq!(
            env_config_dir_from(only_home),
            Some(PathBuf::from("/home/someone/.config"))
        );

        let relative = |key: &str| match key {
            "XDG_CONFIG_HOME" => Some(OsString::from("relative")),
            "HOME" => Some(OsString::from("also-relative")),
            _ => None,
        };
        assert_eq!(
            env_config_dir_from(relative),
            None,
            "相对路径不构成可用目录"
        );
    }

    #[test]
    fn gg_config_dir_overrides_the_platform_default() {
        // 平台中立：不能用 `/custom/...` 这类 POSIX 风格路径，Windows 上
        // `is_absolute()` 对它为假，用例会随平台飘。
        let from = |value: Option<&str>| {
            let value = value.map(OsString::from);
            config_dir_override_from(move |_| value.clone())
        };

        let temp = tempfile::tempdir().expect("tempdir");
        let custom = temp.path().to_path_buf();
        assert_eq!(
            config_dir_override_from({
                let custom = custom.clone().into_os_string();
                move |_| Some(custom.clone())
            }),
            Some(custom)
        );

        assert_eq!(from(None), None);
        assert_eq!(from(Some("")), None, "空值不算覆盖");
        // 相对路径也照样采用：显式配置不该被无声丢弃
        assert_eq!(
            from(Some("relative-config")),
            Some(PathBuf::from("relative-config"))
        );
    }

    #[test]
    fn resolve_notes_dir_obeys_precedence() {
        let cli = PathBuf::from("cli-notes");
        let env = OsString::from("env-notes");
        let cfg = PathBuf::from("cfg-root");

        let path = resolve_notes_dir_with(
            Some(cli.clone()),
            Some(env.clone()),
            Some(cfg.clone()),
            Language::Zh,
        )
        .expect("cli 覆盖");
        assert_eq!(path, cli);

        let path = resolve_notes_dir_with(None, Some(env.clone()), Some(cfg.clone()), Language::Zh)
            .expect("env 覆盖");
        assert_eq!(path, PathBuf::from(env));

        let path =
            resolve_notes_dir_with(None, None, Some(cfg.clone()), Language::Zh).expect("默认目录");
        assert_eq!(path, cfg.join("gg").join("notes"));
    }

    /// 配置解析失败时同样要用上用户设定的语言：`language` 单独宽松读取，
    /// 不受其它键拼写错误的影响。
    #[test]
    fn language_is_read_leniently_even_when_the_config_is_otherwise_invalid() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("config.toml");
        fs::write(&path, "language = \"en\"\nask_befor_ai = false\n").expect("写配置");

        assert_eq!(language_from_config_at(&path), Some(Language::En));
    }

    #[test]
    fn config_errors_are_localized() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("config.toml");
        fs::write(&path, "ai_provider = \"openai\"\n").expect("写配置");

        let err = load_from(&path, Language::En).expect_err("非法配置必须报错");
        assert!(
            format!("{err:#}").contains("Failed to parse the config file"),
            "{err:#}"
        );

        let err = load_from(&path, Language::Zh).expect_err("非法配置必须报错");
        assert!(format!("{err:#}").contains("无法解析配置文件"), "{err:#}");
    }

    #[test]
    fn command_line_language_beats_the_config_file() {
        assert_eq!(
            language_from_args(["--lang=en".to_string()]),
            Some(Language::En)
        );
        assert_eq!(
            language_from_args(["gg".to_string(), "--lang".to_string(), "zh".to_string()]),
            Some(Language::Zh)
        );
        assert_eq!(
            language_from_args(["gg".to_string(), "list".to_string()]),
            None
        );
        assert_eq!(language_from_args(["--lang=jp".to_string()]), None);
    }
}

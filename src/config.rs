use crate::i18n::Language;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

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
#[serde(default)]
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
    pub fn load() -> Result<Self> {
        let path = match config_path() {
            Ok(path) => path,
            Err(_) => return Ok(Self::default()),
        };
        if !path.exists() {
            return Ok(Self::default());
        }

        let raw = fs::read_to_string(&path)
            .with_context(|| format!("无法读取配置文件: {}", path.display()))?;
        let config: Self = toml::from_str(&raw)
            .with_context(|| format!("无法解析配置文件: {}", path.display()))?;
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建配置目录: {}", parent.display()))?;
        }
        let raw = toml::to_string_pretty(self).context("无法序列化配置")?;
        fs::write(&path, raw).with_context(|| format!("无法写入配置文件: {}", path.display()))?;
        Ok(())
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

        let bin = ai_bin_override().unwrap_or_else(|| bin.to_string());
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

/// 在 clap 解析之前探测界面语言，仅用于本地化 `--help` / `--version`。
///
/// 这里刻意吞掉所有错误：真正的配置错误会由 [`AppConfig::load`] 在
/// `app::run` 中完整报出，此处只影响帮助文案的语种。
pub fn peek_language() -> Language {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix("--lang=") {
            if let Some(lang) = Language::parse(value) {
                return lang;
            }
        } else if arg == "--lang" {
            if let Some(value) = args.next() {
                if let Some(lang) = Language::parse(&value) {
                    return lang;
                }
            }
        }
    }

    AppConfig::load()
        .map(|config| config.language())
        .unwrap_or_default()
}

pub fn config_path() -> Result<PathBuf> {
    Ok(config_root_dir()?.join("gg").join("config.toml"))
}

pub fn default_notes_dir() -> Result<PathBuf> {
    Ok(config_root_dir()?.join("gg").join("notes"))
}

pub fn resolve_notes_dir(cli_override: Option<PathBuf>) -> Result<PathBuf> {
    resolve_notes_dir_with(
        cli_override,
        env::var_os("GG_NOTES_DIR"),
        dirs::config_dir(),
    )
}

pub(crate) fn resolve_notes_dir_with(
    cli_override: Option<PathBuf>,
    env_override: Option<OsString>,
    config_root: Option<PathBuf>,
) -> Result<PathBuf> {
    if let Some(path) = cli_override {
        return Ok(path);
    }

    if let Some(path) = env_override {
        return Ok(PathBuf::from(path));
    }

    let root = config_root.context("无法确定系统配置目录")?;
    Ok(root.join("gg").join("notes"))
}

fn config_root_dir() -> Result<PathBuf> {
    dirs::config_dir().context("无法确定系统配置目录")
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

    #[test]
    fn resolve_notes_dir_obeys_precedence() {
        let cli = PathBuf::from("cli-notes");
        let env = OsString::from("env-notes");
        let cfg = PathBuf::from("cfg-root");

        let path = resolve_notes_dir_with(Some(cli.clone()), Some(env.clone()), Some(cfg.clone()))
            .expect("cli 覆盖");
        assert_eq!(path, cli);

        let path =
            resolve_notes_dir_with(None, Some(env.clone()), Some(cfg.clone())).expect("env 覆盖");
        assert_eq!(path, PathBuf::from(env));

        let path = resolve_notes_dir_with(None, None, Some(cfg.clone())).expect("默认目录");
        assert_eq!(path, cfg.join("gg").join("notes"));
    }
}

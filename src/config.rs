use crate::i18n::Language;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

/// AI 提供方。用枚举承载，非法取值在配置反序列化阶段就会报错，
/// 而不是等到查询时才提示「v1 仅支持 claude」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AiProvider {
    #[default]
    Claude,
}

impl AiProvider {
    pub fn code(self) -> &'static str {
        match self {
            AiProvider::Claude => "claude",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct AppConfig {
    pub ask_before_ai: bool,
    pub auto_save_ai: bool,
    pub ask_before_save: bool,
    pub ai_note_language: String,
    pub ai_provider: AiProvider,
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

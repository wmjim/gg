use anyhow::{Context, Result};
use serde::Deserialize;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AppConfig {
    pub ask_before_ai: bool,
    pub auto_save_ai: bool,
    pub ask_before_save: bool,
    pub ai_note_language: String,
    pub ai_provider: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            ask_before_ai: true,
            auto_save_ai: true,
            ask_before_save: false,
            ai_note_language: "zh-CN".to_string(),
            ai_provider: "claude".to_string(),
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
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        let config: Self = toml::from_str(&raw)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
        Ok(config)
    }
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

    let root = config_root.context("Unable to determine system config directory")?;
    Ok(root.join("gg").join("notes"))
}

fn config_root_dir() -> Result<PathBuf> {
    dirs::config_dir().context("Unable to determine system config directory")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_plan() {
        let cfg: AppConfig = toml::from_str("").expect("empty toml should deserialize");
        assert!(cfg.ask_before_ai);
        assert!(cfg.auto_save_ai);
        assert!(!cfg.ask_before_save);
        assert_eq!(cfg.ai_note_language, "zh-CN");
        assert_eq!(cfg.ai_provider, "claude");
    }

    #[test]
    fn custom_toml_overrides_defaults() {
        let raw = r#"
ask_before_ai = false
auto_save_ai = false
ask_before_save = true
ai_note_language = "en"
ai_provider = "claude"
"#;
        let cfg: AppConfig = toml::from_str(raw).expect("valid config");

        assert!(!cfg.ask_before_ai);
        assert!(!cfg.auto_save_ai);
        assert!(cfg.ask_before_save);
        assert_eq!(cfg.ai_note_language, "en");
        assert_eq!(cfg.ai_provider, "claude");
    }

    #[test]
    fn resolve_notes_dir_obeys_precedence() {
        let cli = PathBuf::from("cli-notes");
        let env = OsString::from("env-notes");
        let cfg = PathBuf::from("cfg-root");

        let path = resolve_notes_dir_with(Some(cli.clone()), Some(env.clone()), Some(cfg.clone()))
            .expect("resolve with cli override");
        assert_eq!(path, cli);

        let path = resolve_notes_dir_with(None, Some(env.clone()), Some(cfg.clone()))
            .expect("resolve with env override");
        assert_eq!(path, PathBuf::from(env));

        let path =
            resolve_notes_dir_with(None, None, Some(cfg.clone())).expect("resolve with default");
        assert_eq!(path, cfg.join("gg").join("notes"));
    }
}

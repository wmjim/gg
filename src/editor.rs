//! 打开编辑器。回退顺序：配置指定 > 环境变量 > 终端编辑器 > 系统默认。

use crate::i18n::Language;
use crate::utils::debug_log;
use crate::utils::platform::{self, OpenTarget};
use crate::utils::process::resolve_program;
use anyhow::{Context, Result};
use std::env;
use std::path::Path;
use std::process::Command;

/// 编辑器端口，便于测试时替换为假实现。
pub trait EditorLauncher {
    fn open(&self, path: &Path) -> Result<()>;
}

pub struct SystemEditor {
    /// `config.editor`，优先级最高。
    configured: Option<String>,
    lang: Language,
}

impl SystemEditor {
    pub fn new(configured: Option<String>, lang: Language) -> Self {
        Self { configured, lang }
    }

    pub fn from_config(config: &crate::config::AppConfig) -> Self {
        Self::new(config.editor_spec().map(str::to_string), config.language())
    }
}

impl EditorLauncher for SystemEditor {
    fn open(&self, path: &Path) -> Result<()> {
        if let Some(spec) = self.configured.as_deref() {
            match launch(spec, path) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    eprintln!("{}", self.lang.configured_editor_missing(spec));
                    debug_log!("editor::open: 配置编辑器 `{spec}` 失败: {err:#}");
                }
            }
        } else if let Some(spec) = env_editor_spec() {
            match launch(&spec, path) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    eprintln!("{}", self.lang.env_editor_missing(&spec));
                    debug_log!("editor::open: 环境变量编辑器 `{spec}` 失败: {err:#}");
                }
            }
        }

        open_with_system_default(path, self.lang)
    }
}

/// 启动指定编辑器并等待其退出（编辑场景必须阻塞到用户保存完成）。
fn launch(spec: &str, path: &Path) -> Result<()> {
    let program = resolve_program(spec)?;
    debug_log!(
        "editor::launch: 启动 `{}`, args={:?}, path={}",
        program.bin.display(),
        program.args,
        path.display()
    );

    #[cfg(target_os = "windows")]
    {
        // Windows 上的 code.cmd / code.bat 无法被 CreateProcess 直接执行，
        // 必须经由 cmd.exe 转发。
        if needs_shell_forwarding(&program.bin) {
            let status = Command::new("cmd")
                .arg("/C")
                .arg(&program.bin)
                .args(&program.args)
                .arg(path)
                .status()
                .with_context(|| format!("无法启动 `{}`", program.describe()))?;
            anyhow::ensure!(status.success(), "编辑器退出码 {status}");
            return Ok(());
        }
    }

    let status = Command::new(&program.bin)
        .args(&program.args)
        .arg(path)
        .status()
        .with_context(|| format!("无法启动 `{}`", program.describe()))?;
    anyhow::ensure!(status.success(), "编辑器退出码 {status}");
    Ok(())
}

#[cfg(target_os = "windows")]
fn needs_shell_forwarding(bin: &Path) -> bool {
    bin.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| matches!(ext.to_ascii_lowercase().as_str(), "cmd" | "bat"))
        .unwrap_or(false)
}

fn env_editor_spec() -> Option<String> {
    ["GG_EDITOR", "VISUAL", "EDITOR"]
        .iter()
        .find_map(|key| env::var(key).ok().filter(|value| !value.trim().is_empty()))
}

fn open_with_system_default(path: &Path, lang: Language) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        // 终端编辑器优先，避免在 WSL / 无桌面环境下误开 GUI 程序。
        for editor in ["nvim", "vim", "vi", "hx", "helix", "nano"] {
            if which::which(editor).is_err() {
                continue;
            }
            match Command::new(editor).arg(path).status() {
                Ok(status) if status.success() => {
                    debug_log!("editor::open_with_system_default: 已用 `{editor}` 打开");
                    return Ok(());
                }
                Ok(status) => {
                    debug_log!("editor::open_with_system_default: `{editor}` 退出码 {status}");
                }
                Err(err) => {
                    debug_log!("editor::open_with_system_default: `{editor}` 启动失败: {err}");
                }
            }
        }
    }

    platform::open_path(path, OpenTarget::Editor, lang).with_context(|| lang.no_editor_found())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fake_editor(temp: &Path, name: &str, body: &str) -> std::path::PathBuf {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let path = temp.join(name);
            fs::write(&path, body).expect("写假编辑器");
            let mut perm = fs::metadata(&path).expect("stat").permissions();
            perm.set_mode(0o755);
            fs::set_permissions(&path, perm).expect("chmod");
            path
        }
        #[cfg(not(unix))]
        {
            let _ = (temp, name, body);
            unimplemented!("仅用于 unix 测试")
        }
    }

    #[cfg(unix)]
    #[test]
    fn configured_editor_receives_path_as_argument() {
        let temp = tempfile::tempdir().expect("tempdir");
        let record = temp.path().join("argv.txt");
        let editor = fake_editor(
            temp.path(),
            "recorder",
            &format!("#!/bin/sh\nprintf '%s' \"$1\" > {}\n", record.display()),
        );
        let note = temp.path().join("ls.md");
        fs::write(&note, "# ls\n").expect("写笔记");

        let launcher = SystemEditor::new(Some(editor.display().to_string()), Language::Zh);
        launcher.open(&note).expect("打开笔记");

        let recorded = fs::read_to_string(&record).expect("读取记录");
        assert_eq!(recorded, note.display().to_string());
    }

    #[cfg(unix)]
    #[test]
    fn configured_editor_with_flags_is_split_correctly() {
        let temp = tempfile::tempdir().expect("tempdir");
        let record = temp.path().join("argv.txt");
        let editor = fake_editor(
            temp.path(),
            "recorder",
            &format!(
                "#!/bin/sh\nprintf '%s|%s' \"$1\" \"$2\" > {}\n",
                record.display()
            ),
        );
        let note = temp.path().join("ls.md");
        fs::write(&note, "# ls\n").expect("写笔记");

        let spec = format!("{} -w", editor.display());
        let launcher = SystemEditor::new(Some(spec), Language::Zh);
        launcher.open(&note).expect("打开笔记");

        let recorded = fs::read_to_string(&record).expect("读取记录");
        // 这曾经是 P0 缺陷：整串被当作程序名，导致 NotFound。
        assert_eq!(recorded, format!("-w|{}", note.display()));
    }
}

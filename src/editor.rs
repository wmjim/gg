//! 打开编辑器。回退顺序：配置指定 > 环境变量 > 终端编辑器 > 系统默认。

use crate::i18n::Language;
use crate::utils::debug_log;
use crate::utils::platform::{self, OpenTarget};
use crate::utils::process::{Program, resolve_program};
use anyhow::{Context, Result, bail};
use std::env;
use std::path::Path;
use std::process::Command;

/// 编辑器端口，便于测试时替换为假实现。
pub trait EditorLauncher {
    fn open(&self, path: &Path) -> Result<()>;
}

/// 区分「编辑器没装」与「装了但执行失败」——两者对用户的意义完全不同。
#[derive(Debug)]
enum EditorFailure {
    /// 无法定位可执行文件，可以安全地回退到下一个候选。
    NotInstalled(anyhow::Error),
    /// 编辑器确实运行过但以失败告终，此时再开第二个编辑器只会让人困惑。
    RunFailed(anyhow::Error),
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
            match try_editor(spec, path) {
                Ok(()) => return Ok(()),
                Err(EditorFailure::NotInstalled(err)) => {
                    eprintln!("{}", self.lang.configured_editor_missing(spec));
                    debug_log!("editor::open: 配置编辑器 `{spec}` 无法定位: {err:#}");
                }
                Err(EditorFailure::RunFailed(err)) => {
                    return Err(err).context(self.lang.editor_launch_failed(spec));
                }
            }
        }

        // 配置里的编辑器只是「没装」时，必须继续按优先级链尝试环境变量。
        // 早期版本用 else if 把它们绑定在一起，导致 config.editor 指的软件
        // 未安装时会直接跳过 GG_EDITOR/EDITOR，反而去开 nvim。
        if let Some(spec) = env_editor_spec() {
            match try_editor(&spec, path) {
                Ok(()) => return Ok(()),
                Err(EditorFailure::NotInstalled(err)) => {
                    eprintln!("{}", self.lang.env_editor_missing(&spec));
                    debug_log!("editor::open: 环境变量编辑器 `{spec}` 无法定位: {err:#}");
                }
                Err(EditorFailure::RunFailed(err)) => {
                    return Err(err).context(self.lang.editor_launch_failed(&spec));
                }
            }
        }

        open_with_system_default(path, self.lang)
    }
}

fn try_editor(spec: &str, path: &Path) -> std::result::Result<(), EditorFailure> {
    let program = resolve_program(spec).map_err(EditorFailure::NotInstalled)?;
    run_editor(&program, path).map_err(EditorFailure::RunFailed)
}

/// 启动编辑器并等待其退出（编辑场景必须阻塞到用户保存完成）。
fn run_editor(program: &Program, path: &Path) -> Result<()> {
    debug_log!(
        "editor::run_editor: 启动 `{}`, args={:?}, path={}",
        program.bin.display(),
        program.args,
        path.display()
    );

    #[cfg(target_os = "windows")]
    let status = {
        // Windows 上的 code.cmd / code.bat 无法被 CreateProcess 直接执行，
        // 必须经由 cmd.exe 转发。
        if needs_shell_forwarding(&program.bin) {
            Command::new("cmd")
                .arg("/C")
                .arg(&program.bin)
                .args(&program.args)
                .arg(path)
                .status()
                .with_context(|| format!("无法启动 `{}`", program.describe()))?
        } else {
            spawn_direct(program, path)?
        }
    };

    #[cfg(not(target_os = "windows"))]
    let status = spawn_direct(program, path)?;

    if !status.success() {
        let code = status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_string());
        bail!("编辑器退出码 {code}");
    }
    Ok(())
}

fn spawn_direct(program: &Program, path: &Path) -> Result<std::process::ExitStatus> {
    Command::new(&program.bin)
        .args(&program.args)
        .arg(path)
        .status()
        .with_context(|| format!("无法启动 `{}`", program.describe()))
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
    fn note_in(temp: &Path) -> std::path::PathBuf {
        let note = temp.join("ls.md");
        fs::write(&note, "# ls\n").expect("写笔记");
        note
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
        let note = note_in(temp.path());

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
        let note = note_in(temp.path());

        let spec = format!("{} -w", editor.display());
        let launcher = SystemEditor::new(Some(spec), Language::Zh);
        launcher.open(&note).expect("打开笔记");

        let recorded = fs::read_to_string(&record).expect("读取记录");
        // 这曾经是 P0 缺陷：整串被当作程序名，导致 NotFound。
        assert_eq!(recorded, format!("-w|{}", note.display()));
    }

    #[cfg(unix)]
    #[test]
    fn missing_editor_is_reported_as_not_installed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let note = note_in(temp.path());

        let failure = try_editor("__gg_no_such_editor__", &note).expect_err("必须失败");
        assert!(
            matches!(failure, EditorFailure::NotInstalled(_)),
            "实际: {failure:?}"
        );
    }

    /// 编辑器确实运行过，只是退出码非 0（例如 vim 的 `:cq`）。
    /// 这必须与「没装」区分开，否则会误报「编辑器不存在」。
    #[cfg(unix)]
    #[test]
    fn non_zero_exit_is_reported_as_run_failed_not_missing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let editor = fake_editor(temp.path(), "quitter", "#!/bin/sh\nexit 3\n");
        let note = note_in(temp.path());

        let failure = try_editor(&editor.display().to_string(), &note).expect_err("必须失败");
        match failure {
            EditorFailure::RunFailed(err) => {
                let msg = format!("{err:#}");
                assert!(msg.contains("编辑器退出码"), "实际: {msg}");
            }
            other => panic!("应归类为 RunFailed，实际: {other:?}"),
        }
    }

    /// 执行失败时不得回退到其它编辑器：用户已经看到自己的编辑器了。
    #[cfg(unix)]
    #[test]
    fn run_failure_propagates_instead_of_falling_back() {
        let temp = tempfile::tempdir().expect("tempdir");
        let editor = fake_editor(temp.path(), "quitter", "#!/bin/sh\nexit 3\n");
        let note = note_in(temp.path());

        let launcher = SystemEditor::new(Some(editor.display().to_string()), Language::Zh);
        let err = launcher
            .open(&note)
            .expect_err("执行失败必须向上报错，而不是静默换一个编辑器");

        let msg = format!("{err:#}");
        assert!(msg.contains("quitter"), "报错需指明是哪个编辑器: {msg}");
        assert!(msg.contains("退出码"), "报错需包含真实原因: {msg}");
    }
}

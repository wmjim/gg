//! 打开编辑器。回退顺序：配置指定 > 环境变量 > 终端编辑器 > 系统默认。

use crate::i18n::Language;
use crate::utils::debug_log;
use crate::utils::platform::{self, OpenTarget};
use crate::utils::process::{Program, resolve_program};
use anyhow::{Context, Result, bail};
use std::env;
use std::path::Path;

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

impl std::fmt::Display for EditorFailure {
    /// 仅供诊断/测试打印内部原因；面向用户的文案由 `open` 走 i18n catalog 生成。
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditorFailure::NotInstalled(err) | EditorFailure::RunFailed(err) => {
                write!(formatter, "{err:#}")
            }
        }
    }
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
            match try_editor(spec, path, self.lang) {
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
            match try_editor(&spec, path, self.lang) {
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

fn try_editor(spec: &str, path: &Path, lang: Language) -> std::result::Result<(), EditorFailure> {
    let program = resolve_program(spec, lang).map_err(EditorFailure::NotInstalled)?;
    run_editor(&program, path, lang).map_err(EditorFailure::RunFailed)
}

/// 启动编辑器并等待其退出（编辑场景必须阻塞到用户保存完成）。
fn run_editor(program: &Program, path: &Path, lang: Language) -> Result<()> {
    debug_log!(
        "editor::run_editor: 启动 `{}`, args={:?}, path={}",
        program.bin.display(),
        program.args,
        path.display()
    );

    // Windows 上的 code.cmd / code.bat 转发由 Program::command() 统一处理，
    // 这里不再需要平台分支。
    let status = program
        .command()
        .arg(path)
        .status()
        .with_context(|| lang.program_spawn_failed(&program.describe()))?;

    if !status.success() {
        let code = status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_string());
        bail!("{}", lang.editor_exit_code(&code));
    }
    Ok(())
}

fn env_editor_spec() -> Option<String> {
    ["GG_EDITOR", "VISUAL", "EDITOR"]
        .iter()
        .find_map(|key| env::var(key).ok().filter(|value| !value.trim().is_empty()))
}

fn open_with_system_default(path: &Path, lang: Language) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        // `Command` 只在这个平台分支里用到，导入放在分支内：
        // 放在文件顶部会在别的平台上被判为未使用的导入。
        use std::process::Command;

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

/// 编辑器单元测试整体只在 Unix 上编译。
///
/// 这些用例依赖 POSIX 进程语义：shell 脚本、可执行位、以及并行 fork 导致的
/// ETXTBSY 竞态，无法在 Windows 上表达。Windows 的编辑器行为由集成测试覆盖
/// （`failing_editor_is_reported_instead_of_silently_falling_back` 等）。
/// 把整个模块门控起来，也避免在 Windows 上因所有用例都被 cfg 掉而留下
/// 未使用的导入。
#[cfg(all(test, unix))]
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

    /// 并行测试下其它线程 fork 时可能短暂继承脚本的写 fd，
    /// 导致 exec 得到 ETXTBSY（Text file busy）。
    /// 这是测试环境的竞态（并行用例会 fork 出 `sleep` 等进程），
    /// 不是被测代码的问题，退避重试即可。
    #[cfg(unix)]
    fn retry_on_text_file_busy<T>(
        mut action: impl FnMut() -> std::result::Result<T, EditorFailure>,
    ) -> std::result::Result<T, EditorFailure> {
        const ATTEMPTS: usize = 25;
        for attempt in 1..=ATTEMPTS {
            match action() {
                Err(EditorFailure::RunFailed(err))
                    if attempt < ATTEMPTS && is_text_file_busy(&err) =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                other => return other,
            }
        }
        unreachable!("循环内必返回")
    }

    #[cfg(unix)]
    fn open_with_retry(
        launcher: &dyn EditorLauncher,
        path: &Path,
    ) -> std::result::Result<(), EditorFailure> {
        retry_on_text_file_busy(|| {
            // `open` 已把原因包进 anyhow；这里统一按「执行失败」分类，
            // 只有 ETXTBSY 会被重试，其余立即返回。
            launcher.open(path).map_err(EditorFailure::RunFailed)
        })
    }

    #[cfg(unix)]
    fn is_text_file_busy(err: &anyhow::Error) -> bool {
        err.chain().any(|cause| {
            cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::ExecutableFileBusy)
        })
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
        open_with_retry(&launcher, &note).expect("打开笔记");

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
        open_with_retry(&launcher, &note).expect("打开笔记");

        let recorded = fs::read_to_string(&record).expect("读取记录");
        // 这曾经是 P0 缺陷：整串被当作程序名，导致 NotFound。
        assert_eq!(recorded, format!("-w|{}", note.display()));
    }

    #[cfg(unix)]
    #[test]
    fn missing_editor_is_reported_as_not_installed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let note = note_in(temp.path());

        let failure =
            retry_on_text_file_busy(|| try_editor("__gg_no_such_editor__", &note, Language::Zh))
                .expect_err("必须失败");
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

        let failure = retry_on_text_file_busy(|| {
            try_editor(&editor.display().to_string(), &note, Language::Zh)
        })
        .expect_err("必须失败");
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
        let err = open_with_retry(&launcher, &note)
            .expect_err("执行失败必须向上报错，而不是静默换一个编辑器");

        let msg = format!("{err:#}");
        assert!(msg.contains("quitter"), "报错需指明是哪个编辑器: {msg}");
        assert!(msg.contains("退出码"), "报错需包含真实原因: {msg}");
    }
}

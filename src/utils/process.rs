//! 可执行命令字符串解析。
//!
//! 用户配置形如 `"code -w"`、`"/opt/glow -p"`、`"C:\\Program Files\\glow.exe"`
//! 的字符串。既不能把整串当程序名（`Command::new("code -w")` 必然 NotFound），
//! 也不能盲目按空白切分（会破坏含空格的 Windows 绝对路径）。

use anyhow::{Context, Result, anyhow, bail};
use std::io;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

/// 等待子进程退出的轮询间隔：兼顾响应速度与 CPU 占用。
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// 等待子进程退出，超时则杀死并回收。
///
/// 返回 `Ok(None)` 表示已超时（进程已被杀死且回收，不会留下僵尸）。
/// 外部命令探测必须带超时：`claude --version` 在首次运行、交互式登录提示或
/// 网络阻塞时可能长时间不返回，否则 `gg` 会静默挂住。
pub fn wait_with_timeout(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }

        if Instant::now() >= deadline {
            // 进程可能在 try_wait 与 kill 之间刚好退出，此处忽略 kill 失败。
            let _ = child.kill();
            // 必须 wait 回收，否则留下僵尸进程。
            let _ = child.wait();
            return Ok(None);
        }

        std::thread::sleep(POLL_INTERVAL);
    }
}

/// 解析后的可执行程序：真实路径 + 独立参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub bin: PathBuf,
    pub args: Vec<String>,
}

impl Program {
    /// 构造用于启动的 [`Command`]。
    ///
    /// Windows 上 `.cmd` / `.bat` 无法被 `CreateProcess` 直接执行，必须经
    /// `cmd.exe` 转发。编辑器、AI 后端与 glow 都受这条限制影响，因此统一
    /// 放在这里处理 —— 各处自行 `Command::new(bin)` 正是跨平台 bug 的温床。
    ///
    /// 已知限制：`cmd /C` 有自己的引号剥离规则（命令行以引号开头时，会去掉
    /// 最外层一对引号），因此**可执行文件路径含空格**时（如
    /// `C:\Program Files\...`）转发可能被解析错。当前按参数逐个传递，在
    /// 不含空格的路径下正确 —— 涵盖 CI 与常见的 npm / AppData 安装位置。
    pub fn command(&self) -> Command {
        // 判断内联在这里，而不是抽成 `#[cfg(windows)] fn`：那样会引入一个
        // 「只在 Windows 上被使用」的类型导入（`Path`），Linux 上被判为未使用
        // 的导入——和本仓库踩过的那次是同一个坑。
        let is_windows_script = self.is_windows_script();
        if is_windows_script {
            let mut command = Command::new("cmd");
            command.arg("/C").arg(&self.bin).args(&self.args);
            return command;
        }

        let mut command = Command::new(&self.bin);
        command.args(&self.args);
        command
    }

    /// 是否需要经 `cmd.exe` 转发（Windows 的 `.cmd` / `.bat` 不能被
    /// `CreateProcess` 直接执行）。非 Windows 上恒为 false。
    fn is_windows_script(&self) -> bool {
        if !cfg!(windows) {
            return false;
        }
        self.bin
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| matches!(ext.to_ascii_lowercase().as_str(), "cmd" | "bat"))
            .unwrap_or(false)
    }

    /// 把参数与附加路径拼成命令行字符串，仅用于错误提示。
    pub fn describe(&self) -> String {
        let mut parts = vec![self.bin.display().to_string()];
        parts.extend(self.args.iter().cloned());
        parts.join(" ")
    }
}

/// 解析可执行命令字符串。
///
/// 解析顺序：
/// 1. 整串当作路径解析 —— 保护 Windows 下含空格的绝对路径不被切碎；
/// 2. 含路径分隔符时按「最长前缀」逐段合并 —— 处理 `editor = "/opt/my tools/ed -w"`
///    这种未加引号、同时含空格与参数的写法；
/// 3. 退回 shell 词法切分 —— 支持 `"code -w"`、`"'/opt/my editor' -p"`。
///
/// 可执行文件必须真实存在，否则直接报错，避免把失败推迟到 spawn 阶段。
pub fn resolve_program(spec: &str) -> Result<Program> {
    let spec = spec.trim();
    if spec.is_empty() {
        bail!("可执行命令不能为空");
    }

    let expanded = expand_tilde(spec);
    if let Ok(bin) = which::which(&expanded) {
        return Ok(Program {
            bin,
            args: Vec::new(),
        });
    }

    let tokens = split_command_line(spec)?;

    if let Some(program) = resolve_by_longest_prefix(&tokens) {
        return Ok(program);
    }

    let (program, args) = tokens
        .split_first()
        .ok_or_else(|| anyhow!("可执行命令不能为空"))?;

    let bin = which::which(expand_tilde(program))
        .with_context(|| format!("未找到可执行文件 `{program}`，请确认已安装或改用绝对路径"))?;

    Ok(Program {
        bin,
        args: args.to_vec(),
    })
}

/// 按 shell 词法把命令行切成词，支持单引号与双引号分组。
///
/// 刻意**不**把反斜杠当转义字符：Windows 路径 `C:\Tools\glow.exe` 里的
/// 反斜杠一旦被吃掉，路径就废了。`shell_words::split` 是 POSIX 语义，
/// 正是这么做的 —— 它在 Linux 上完全正确，在 Windows 上毁掉每一个路径。
pub fn split_command_line(spec: &str) -> Result<Vec<String>> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;

    for ch in spec.chars() {
        if let Some(open) = quote {
            if ch == open {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }

        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                in_token = true;
            }
            c if c.is_whitespace() => {
                if in_token {
                    tokens.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            c => {
                current.push(c);
                in_token = true;
            }
        }
    }

    if quote.is_some() {
        bail!("无法解析可执行命令 {spec:?}，请检查引号是否配对");
    }
    if in_token {
        tokens.push(current);
    }
    Ok(tokens)
}

/// 路径含空格又未加引号时，空白切分会把路径拦腰截断。
/// 按最长前缀逐段合并重试，命中即为正确切分点。
fn resolve_by_longest_prefix(tokens: &[String]) -> Option<Program> {
    if tokens.len() < 2 {
        return None;
    }

    for split_at in (1..tokens.len()).rev() {
        let candidate = tokens[..split_at].join(" ");
        if let Ok(bin) = which::which(expand_tilde(&candidate)) {
            return Some(Program {
                bin,
                args: tokens[split_at..].to_vec(),
            });
        }
    }

    None
}

fn expand_tilde(spec: &str) -> PathBuf {
    if let Some(rest) = spec.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    #[cfg(unix)]
    fn install_fake_bin(dir: &Path, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join(name);
        fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write fake bin");
        let mut perm = fs::metadata(&path).expect("stat fake bin").permissions();
        perm.set_mode(0o755);
        fs::set_permissions(&path, perm).expect("chmod fake bin");
        path
    }

    #[test]
    fn rejects_empty_spec() {
        assert!(resolve_program("   ").is_err());
    }

    #[test]
    fn reports_missing_binary_with_the_offending_name() {
        let err = resolve_program("__gg_missing_binary__").expect_err("must fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("__gg_missing_binary__"), "实际信息: {msg}");
    }

    #[cfg(unix)]
    #[test]
    fn splits_program_and_arguments() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bin = install_fake_bin(temp.path(), "gg-fake-editor");
        let spec = format!("{} -w --flag", bin.display());

        let program = resolve_program(&spec).expect("resolve split spec");
        assert_eq!(program.bin, bin);
        assert_eq!(program.args, vec!["-w".to_string(), "--flag".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn keeps_unquoted_path_with_spaces_intact() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bin = install_fake_bin(temp.path(), "my editor");

        let program = resolve_program(&bin.display().to_string()).expect("resolve spaced path");
        assert_eq!(program.bin, bin);
        assert!(program.args.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn supports_quoted_path_with_spaces_plus_arguments() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bin = install_fake_bin(temp.path(), "my editor");
        let spec = format!("\"{}\" -w", bin.display());

        let program = resolve_program(&spec).expect("resolve quoted spec");
        assert_eq!(program.bin, bin);
        assert_eq!(program.args, vec!["-w".to_string()]);
    }

    /// 配置里更自然的写法：路径含空格且未加引号。
    #[cfg(unix)]
    #[test]
    fn supports_unquoted_path_with_spaces_plus_arguments() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nested = temp.path().join("my tools");
        fs::create_dir_all(&nested).expect("创建带空格的目录");
        let bin = install_fake_bin(&nested, "recorder");
        let spec = format!("{} -w --flag", bin.display());

        let program = resolve_program(&spec).expect("resolve unquoted spaced spec");
        assert_eq!(program.bin, bin);
        assert_eq!(program.args, vec!["-w".to_string(), "--flag".to_string()]);
    }

    #[test]
    fn splitter_preserves_windows_backslashes() {
        // POSIX 语义的分词器会把这些反斜杠当转义吃掉，路径就废了
        let tokens = split_command_line(r"C:\Tools\glow.exe -s dark -w 80").expect("切分成功");
        assert_eq!(
            tokens,
            vec![
                r"C:\Tools\glow.exe".to_string(),
                "-s".to_string(),
                "dark".to_string(),
                "-w".to_string(),
                "80".to_string(),
            ]
        );
    }

    #[test]
    fn splitter_honours_quotes_and_collapses_whitespace() {
        assert_eq!(
            split_command_line("  \"a b\"  'c d'  e  ").expect("切分成功"),
            vec!["a b".to_string(), "c d".to_string(), "e".to_string()]
        );
        assert_eq!(
            split_command_line("").expect("切分成功"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn splitter_keeps_empty_quoted_token() {
        assert_eq!(
            split_command_line("cmd \"\"").expect("切分成功"),
            vec!["cmd".to_string(), String::new()]
        );
    }

    /// Windows 上的 AI 预设接线就靠这条：整串路径 + 参数必须停在同一个词上。
    #[test]
    fn resolves_windows_style_spec_with_arguments() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bin = if cfg!(windows) {
            let path = temp.path().join("fake.cmd");
            fs::write(&path, "@echo off\r\n").expect("写脚本");
            path
        } else {
            install_fake_bin(temp.path(), "fake.cmd")
        };

        let spec = format!("{} -p --output-format text", bin.display());
        let program = resolve_program(&spec).expect("解析成功");
        assert_eq!(program.bin, bin);
        assert_eq!(
            program.args,
            vec![
                "-p".to_string(),
                "--output-format".to_string(),
                "text".to_string()
            ]
        );
    }

    #[test]
    fn detects_unbalanced_quotes() {
        let err = resolve_program("\"unterminated").expect_err("must fail");
        assert!(format!("{err:#}").contains("引号"));
    }

    #[cfg(unix)]
    #[test]
    fn resolves_bare_command_from_path() {
        let program = resolve_program("sh").expect("resolve sh from PATH");
        assert!(program.bin.is_absolute());
        assert!(program.args.is_empty());
    }

    #[cfg(unix)]
    fn spawn_sh(script: &str) -> Child {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .spawn()
            .expect("启动 sh")
    }

    #[cfg(unix)]
    #[test]
    fn wait_with_timeout_returns_status_for_fast_process() {
        let mut child = spawn_sh("exit 0");
        let status = wait_with_timeout(&mut child, Duration::from_secs(10))
            .expect("等待不应出错")
            .expect("进程应正常退出");
        assert!(status.success());
    }

    #[cfg(unix)]
    #[test]
    fn wait_with_timeout_kills_and_reaps_hung_process() {
        let mut child = spawn_sh("sleep 60");
        let start = Instant::now();

        let status =
            wait_with_timeout(&mut child, Duration::from_millis(150)).expect("等待不应出错");

        assert!(status.is_none(), "超时应返回 None");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "应在超时附近立即返回，实际 {:?}",
            start.elapsed()
        );
        assert!(
            child.try_wait().expect("try_wait 不应出错").is_some(),
            "超时后子进程必须已被回收，不能留下僵尸"
        );
    }

    #[cfg(unix)]
    #[test]
    fn wait_with_timeout_does_not_flag_non_zero_exit_as_timeout() {
        let mut child = spawn_sh("exit 7");
        let status = wait_with_timeout(&mut child, Duration::from_secs(10))
            .expect("等待不应出错")
            .expect("退出码非 0 不等于超时");
        assert_eq!(status.code(), Some(7));
    }
}

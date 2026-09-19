//! 统一的「用外部程序打开路径」实现。
//!
//! 原先浏览器与编辑器各自维护一份 WSL 探测（`wslpath` / `wslview` /
//! `powershell.exe` / `cmd.exe` / 绝对路径兜底），两份代码已经开始漂移。
//! 这里收敛为单一实现，并统一使用 `which` crate 探测，而不是 shell out 到
//! 系统 `which`（精简容器镜像里常常没有该可执行文件）。

use crate::i18n::Language;
use crate::utils::debug_log;
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

/// 待打开内容的用途，决定候选程序列表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenTarget {
    Browser,
    Editor,
}

impl OpenTarget {
    pub fn label(self, lang: Language) -> String {
        match self {
            OpenTarget::Browser => lang.opener_target_browser(),
            OpenTarget::Editor => lang.opener_target_editor(),
        }
    }
}

// WSL 互操作缺失时，PATH 里可能没有 Windows 程序，仍需尝试这些固定位置。
const POWERSHELL_ABS: &str = "/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe";
const PWSH_ABS: &str = "/mnt/c/Program Files/PowerShell/7/pwsh.exe";
const CMD_ABS: &str = "/mnt/c/Windows/System32/cmd.exe";
const EXPLORER_ABS: &str = "/mnt/c/Windows/explorer.exe";

pub fn is_wsl() -> bool {
    std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::env::var_os("WSL_INTEROP").is_some()
        || std::fs::read_to_string("/proc/version")
            .map(|version| version.to_lowercase().contains("microsoft"))
            .unwrap_or(false)
}

pub fn to_windows_path(path: &Path) -> PathBuf {
    match Command::new("wslpath").arg("-w").arg(path).output() {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if text.is_empty() {
                path.to_path_buf()
            } else {
                PathBuf::from(text)
            }
        }
        _ => path.to_path_buf(),
    }
}

/// 使用系统默认程序打开 `path`。
///
/// 返回 `Err` 表示所有候选程序都失败，错误信息包含已尝试的候选列表，
/// 便于定位是 PATH 缺失还是互操作未开启。
pub fn open_path(path: &Path, target: OpenTarget, lang: Language) -> Result<()> {
    if is_wsl() {
        debug_log!(
            "platform::open_path: 检测到 WSL, target={target:?}, path={}",
            path.display()
        );
        return open_via_wsl(path, target, lang);
    }
    open_native(path, target, lang)
}

#[cfg(target_os = "windows")]
fn open_native(path: &Path, target: OpenTarget, lang: Language) -> Result<()> {
    debug_log!(
        "platform::open_native(windows): target={:?}, path={}",
        target.label(lang),
        path.display()
    );

    let mut command = match target {
        OpenTarget::Browser => {
            let mut command = Command::new("cmd");
            command.args(["/C", "start", ""]);
            command
        }
        OpenTarget::Editor => Command::new("notepad.exe"),
    };
    command.arg(path);

    command.spawn().with_context(|| {
        lang.opener_launch_failed(&target.label(lang), &path.display().to_string())
    })?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn open_native(path: &Path, target: OpenTarget, lang: Language) -> Result<()> {
    debug_log!(
        "platform::open_native(macos): target={:?}, path={}",
        target.label(lang),
        path.display()
    );

    let mut command = Command::new("open");
    if target == OpenTarget::Editor {
        command.arg("-e");
    }
    command.arg(path);

    command
        .spawn()
        .with_context(|| lang.opener_launch_failed("open", &path.display().to_string()))?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn open_native(path: &Path, target: OpenTarget, lang: Language) -> Result<()> {
    // `gio` 需要显式子命令，其余候选直接吃路径。
    let candidates: &[(&str, &[&str])] = match target {
        OpenTarget::Browser => &[
            ("xdg-open", &[]),
            ("gio", &["open"]),
            ("gnome-open", &[]),
            ("kde-open", &[]),
            ("firefox", &[]),
            ("chromium", &[]),
            ("google-chrome", &[]),
        ],
        OpenTarget::Editor => &[
            ("xdg-open", &[]),
            ("gio", &["open"]),
            ("gnome-open", &[]),
            ("kde-open", &[]),
        ],
    };

    let mut tried: Vec<String> = Vec::new();
    for (name, extra_args) in candidates {
        let Ok(bin) = which::which(name) else {
            debug_log!("platform::open_native: `{name}` 不在 PATH 中, 跳过");
            tried.push((*name).to_string());
            continue;
        };

        let mut command = Command::new(&bin);
        command.args(*extra_args).arg(path);
        match command.spawn() {
            Ok(_) => {
                debug_log!(
                    "platform::open_native: 已通过 `{name}` 打开 {} (target={:?})",
                    path.display(),
                    target.label(lang)
                );
                return Ok(());
            }
            Err(err) => {
                debug_log!("platform::open_native: `{name}` 启动失败: {err}");
                tried.push(format!("{name}({err})"));
            }
        }
    }

    bail!(
        "{}",
        lang.no_opener_found(&target.label(lang), &tried.join(", "))
    )
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn open_native(path: &Path, target: OpenTarget, lang: Language) -> Result<()> {
    bail!(
        "{}",
        lang.no_opener_found(&target.label(lang), &path.display().to_string())
    )
}

fn open_via_wsl(path: &Path, target: OpenTarget, lang: Language) -> Result<()> {
    let windows_path = to_windows_path(path);
    let windows_path = windows_path.display().to_string();
    debug_log!("platform::open_via_wsl: 转换路径 -> {windows_path}");

    let mut tried: Vec<String> = Vec::new();
    for (spec, args) in wsl_attempts(&windows_path) {
        let Ok(bin) = which::which(&spec) else {
            debug_log!("platform::open_via_wsl: `{spec}` 不可用, 跳过");
            tried.push(spec);
            continue;
        };

        let mut command = Command::new(&bin);
        command.args(&args);
        match command.status() {
            Ok(status) if status.success() => {
                debug_log!(
                    "platform::open_via_wsl: 已通过 `{spec}` 打开 {windows_path} (target={:?})",
                    target.label(lang)
                );
                return Ok(());
            }
            Ok(status) => {
                debug_log!("platform::open_via_wsl: `{spec}` 退出码 {status}");
                tried.push(format!("{spec}({status})"));
            }
            Err(err) => {
                debug_log!("platform::open_via_wsl: `{spec}` 启动失败: {err}");
                tried.push(format!("{spec}({err})"));
            }
        }
    }

    bail!(
        "{}",
        lang.wsl_opener_failed(&target.label(lang), &tried.join(", "))
    )
}

fn wsl_attempts(windows_path: &str) -> Vec<(String, Vec<String>)> {
    let path = windows_path.to_string();
    vec![
        ("wslview".to_string(), vec![path.clone()]),
        ("powershell.exe".to_string(), powershell_args(&path)),
        ("pwsh.exe".to_string(), powershell_args(&path)),
        ("cmd.exe".to_string(), cmd_args(&path)),
        ("explorer.exe".to_string(), vec![path.clone()]),
        (POWERSHELL_ABS.to_string(), powershell_args(&path)),
        (PWSH_ABS.to_string(), powershell_args(&path)),
        (CMD_ABS.to_string(), cmd_args(&path)),
        (EXPLORER_ABS.to_string(), vec![path]),
    ]
}

fn powershell_args(path: &str) -> Vec<String> {
    vec![
        "-NoProfile".to_string(),
        "-Command".to_string(),
        "Start-Process".to_string(),
        path.to_string(),
    ]
}

fn cmd_args(path: &str) -> Vec<String> {
    vec![
        "/C".to_string(),
        "start".to_string(),
        String::new(),
        path.to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powershell_args_keep_path_as_single_token() {
        let args = powershell_args(r"C:\Users\me\my notes\a.html");
        assert_eq!(args.len(), 4);
        assert_eq!(args[1], "-Command");
        assert_eq!(args[3], r"C:\Users\me\my notes\a.html");
    }

    #[test]
    fn wsl_attempts_cover_interop_and_absolute_fallbacks() {
        let attempts = wsl_attempts(r"C:\tmp\a.html");
        let names: Vec<&str> = attempts.iter().map(|(name, _)| name.as_str()).collect();

        assert!(names.contains(&"wslview"));
        assert!(names.contains(&"powershell.exe"));
        assert!(names.contains(&POWERSHELL_ABS));
        assert!(names.contains(&EXPLORER_ABS));
    }

    #[test]
    fn target_labels_are_localized() {
        assert_eq!(OpenTarget::Browser.label(Language::Zh), "浏览器");
        assert_eq!(OpenTarget::Browser.label(Language::En), "browser");
    }
}

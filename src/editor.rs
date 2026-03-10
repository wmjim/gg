use std::env;
use std::io;
use std::path::Path;
use std::process::Command;

pub fn open_in_editor(path: &Path) -> io::Result<()> {
    if let Some(editor) = editor_command() {
        return run_editor_command(&editor, path);
    }

    open_with_default_editor(path)
}

fn editor_command() -> Option<String> {
    env::var("GG_EDITOR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| env::var("VISUAL").ok().filter(|value| !value.trim().is_empty()))
        .or_else(|| env::var("EDITOR").ok().filter(|value| !value.trim().is_empty()))
}

fn run_editor_command(command: &str, path: &Path) -> io::Result<()> {
    let path_str = path.to_string_lossy();

    #[cfg(target_os = "windows")]
    {
        let cmd = format!("{} \"{}\"", command, path_str);
        Command::new("cmd").args(["/C", &cmd]).spawn()?;
        return Ok(());
    }

    #[cfg(not(target_os = "windows"))]
    {
        let cmd = format!("{} \"{}\"", command, path_str);
        Command::new("sh").args(["-c", &cmd]).spawn()?;
        return Ok(());
    }
}

fn open_with_default_editor(path: &Path) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        Command::new("notepad.exe").arg(path).spawn()?;
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open").args(["-e", &path.to_string_lossy()]).spawn()?;
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        // Prefer terminal editors when available
        let cli_editors = ["nvim", "vim", "vi", "hx", "helix", "nano"];
        for editor in cli_editors {
            let status = Command::new(editor).arg(path).status();
            if let Ok(status) = status {
                if status.success() {
                    return Ok(());
                }
            }
        }

        // Check if running in WSL
        let is_wsl = std::env::var("WSL_DISTRO_NAME").is_ok()
            || std::fs::read_to_string("/proc/version")
                .map(|v| v.to_lowercase().contains("microsoft"))
                .unwrap_or(false);

        if is_wsl {
            // Convert path to Windows format first
            let windows_path = Command::new("wslpath")
                .arg("-w")
                .arg(path)
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_else(|_| path.to_string_lossy().to_string());

            // Try wslview first if available (wslu)
            if let Ok(status) = Command::new("wslview").arg(&windows_path).status() {
                if status.success() {
                    return Ok(());
                }
            }

            // Prefer Windows PowerShell Start-Process
            if let Ok(status) = Command::new("powershell.exe")
                .args(["-NoProfile", "-Command", "Start-Process", &windows_path])
                .status()
            {
                if status.success() {
                    return Ok(());
                }
            }

            // PowerShell 7 fallback
            if let Ok(status) = Command::new("pwsh.exe")
                .args(["-NoProfile", "-Command", "Start-Process", &windows_path])
                .status()
            {
                if status.success() {
                    return Ok(());
                }
            }

            // cmd.exe fallback
            if let Ok(status) = Command::new("cmd.exe")
                .args(["/C", "start", "", &windows_path])
                .status()
            {
                if status.success() {
                    return Ok(());
                }
            }

            // Absolute Windows paths if PATH interop is missing
            let powershell_path = "/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe";
            if let Ok(status) = Command::new(powershell_path)
                .args(["-NoProfile", "-Command", "Start-Process", &windows_path])
                .status()
            {
                if status.success() {
                    return Ok(());
                }
            }

            let cmd_path = "/mnt/c/Windows/System32/cmd.exe";
            if let Ok(status) = Command::new(cmd_path)
                .args(["/C", "start", "", &windows_path])
                .status()
            {
                if status.success() {
                    return Ok(());
                }
            }

            let explorer_path = "/mnt/c/Windows/explorer.exe";
            if let Ok(status) = Command::new(explorer_path)
                .arg(&windows_path)
                .status()
            {
                if status.success() {
                    return Ok(());
                }
            }
        }

        let candidates = ["xdg-open", "gio", "gnome-open", "kde-open"];
        for candidate in candidates {
            if Command::new(candidate).arg(path).spawn().is_ok() {
                return Ok(());
            }
        }
        return Err(io::Error::other(
            "未找到可用的编辑器。请设置 GG_EDITOR/EDITOR/VISUAL，或安装 vim/nvim/helix，或在 WSL 中安装 wslu (wslview)。",
        ));
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        return Err(io::Error::other("不支持的操作系统"));
    }
}



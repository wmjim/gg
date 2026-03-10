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
        let candidates = ["xdg-open", "gio", "gnome-open", "kde-open"];
        for candidate in candidates {
            if Command::new(candidate).arg(path).spawn().is_ok() {
                return Ok(());
            }
        }
        return Err(io::Error::other(
            "未找到可用的编辑器。请设置 EDITOR 或 VISUAL 环境变量。",
        ));
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        return Err(io::Error::other("不支持的操作系统"));
    }
}

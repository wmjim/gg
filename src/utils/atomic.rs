//! 原子替换文件内容。
//!
//! 直接用 `fs::write` 会先把文件截断成 0 字节再写（`File::create` + `write_all`），
//! 进程在这两步之间被杀（Ctrl-C、OOM、关机）就会留下半截文件。对**配置**来说这会让
//! `gg` 完全无法启动（解析是严格的），对**笔记**来说会丢掉刚生成的内容。
//!
//! 同目录内的 `rename` 是原子的：目标只会是「旧内容」或「新内容」，不存在中间态。
//!
//! 放在这里而不是各模块各写一份：这套流程里有几处容易漏的细节（临时文件必须与目标
//! 同目录、重命名前先 `sync_all`），两份实现必然会漂移 —— `utils::platform` 就是
//! 因为浏览器与编辑器各自维护一份 WSL 探测、开始对不上而收敛出来的。

use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;

/// 原子替换 `path` 的内容。
///
/// `temp_prefix` 只影响临时文件名，建议用前导点开头（Unix 下 `ls` 默认不显示），
/// 并且**不要**让临时文件带上目标的扩展名 —— 笔记目录里的临时文件正是靠扩展名
/// 不是 `.md` 才不会被 `scan_commands` 当作笔记。
///
/// `describe_failure` 由调用方提供，把每一步失败包装成本地化文案（配置写失败与
/// 笔记写失败要说不同的话）；底层 IO 错误仍留在上下文链里，`{err:#}` 或
/// `GG_DEBUG=1` 可以看到真正的原因。
///
/// 副作用：`tempfile` 以 0600 创建临时文件，重命名后目标文件的权限比直接
/// `fs::write`（受 umask 影响，通常为 0644）更严。不含密钥的内容收紧无害；
/// 若将来要保留原权限，需要在重命名前把目标文件已有的权限设给临时文件。
pub fn replace_file(
    path: &Path,
    content: &str,
    temp_prefix: &str,
    describe_failure: &dyn Fn() -> String,
) -> Result<()> {
    // 目标可能是相对路径且没有父目录（例如 `config.toml`），此时临时文件落在当前
    // 目录里 —— 仍然与目标同目录，`rename` 不会跨文件系统。
    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let mut file = tempfile::Builder::new()
        .prefix(temp_prefix)
        .suffix(".tmp")
        .tempfile_in(dir)
        .with_context(describe_failure)?;
    file.write_all(content.as_bytes())
        .with_context(describe_failure)?;
    // 先让数据落盘再重命名：否则重命名后掉电可能得到一个空文件。
    file.as_file().sync_all().with_context(describe_failure)?;
    file.persist(path).with_context(describe_failure)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn failure() -> String {
        "写入失败".to_string()
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("读目录")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn writes_new_content_and_leaves_no_temp_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("note.md");

        replace_file(&path, "# ls\n", ".gg-note-", &failure).expect("写入成功");

        assert_eq!(fs::read_to_string(&path).expect("读取"), "# ls\n");
        assert_eq!(names_in(temp.path()), vec!["note.md"]);
    }

    #[test]
    fn replaces_existing_content_completely() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("note.md");
        fs::write(&path, "旧内容，比新内容长得多\n").expect("预置旧内容");

        replace_file(&path, "新内容\n", ".gg-note-", &failure).expect("写入成功");

        assert_eq!(fs::read_to_string(&path).expect("读取"), "新内容\n");
        assert_eq!(names_in(temp.path()), vec!["note.md"]);
    }

    /// 临时文件不得占用目标的扩展名，否则会被 `scan_commands` 当成笔记。
    #[test]
    fn temp_file_does_not_look_like_the_target() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("note.md");
        let seen = std::cell::RefCell::new(Vec::new());

        // 在临时文件存在期间偷看一眼目录。
        let spy = |name: &str| {
            let dir = temp.path().to_path_buf();
            seen.borrow_mut().push((name.to_string(), names_in(&dir)));
        };
        let mut file = tempfile::Builder::new()
            .prefix(".gg-note-")
            .suffix(".tmp")
            .tempfile_in(temp.path())
            .expect("建临时文件");
        file.write_all(b"x").expect("写");
        spy("during");
        drop(file);

        let during = &seen.borrow()[0].1;
        let temp_name = during
            .iter()
            .find(|name| name.starts_with(".gg-note-"))
            .expect("应看到临时文件");
        assert!(
            temp_name.ends_with(".tmp"),
            "临时文件应带 .tmp 后缀，实际 {temp_name}"
        );
        assert!(
            !temp_name.ends_with(".md"),
            "临时文件不能以 .md 结尾，否则会被当作笔记"
        );

        replace_file(&path, "内容\n", ".gg-note-", &failure).expect("写入成功");
        assert_eq!(names_in(temp.path()), vec!["note.md"]);
    }

    /// 目标位于子目录、且父目录不存在时直接报错，由调用方负责先建目录并给出
    /// 贴合场景的文案。
    #[test]
    fn missing_parent_directory_is_reported() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("nope").join("note.md");

        let err = replace_file(&path, "x", ".gg-note-", &failure).expect_err("必须失败");
        assert!(format!("{err:#}").contains("写入失败"), "{err:#}");
        assert!(!path.exists(), "失败时不得留下目标文件");
    }
}

//! 端到端 CLI 测试。
//!
//! 假 claude 脚本必须按平台生成：早期版本只写 `.cmd` 且未设置可执行位，
//! 导致所有非 Windows 环境的 `cargo test` 直接失败。

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn command_for(temp: &TempDir) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("gg"));

    let appdata = temp.path().join("appdata");
    let home = temp.path().join("home");
    fs::create_dir_all(&appdata).expect("创建 appdata");
    fs::create_dir_all(&home).expect("创建 home");

    cmd.env("APPDATA", &appdata)
        .env("XDG_CONFIG_HOME", &appdata)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("NO_COLOR", "1")
        // 测试必须与宿主机是否安装 glow / claude 解耦。
        .env("GG_GLOW_BIN", "__missing_glow_binary__")
        .env_remove("GG_NOTES_DIR")
        .env_remove("GG_EDITOR")
        .env_remove("VISUAL")
        .env_remove("EDITOR");

    cmd
}

fn write_note(notes_dir: &Path, command: &str, markdown: &str) {
    fs::create_dir_all(notes_dir).expect("创建笔记目录");
    fs::write(notes_dir.join(format!("{command}.md")), markdown).expect("写入笔记");
}

#[cfg(unix)]
fn create_fake_claude(temp: &TempDir) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    // `--version` 探测不写标记；标记存在即代表真的走到了生成路径。
    let path = temp.path().join("fake-claude");
    let script = "#!/bin/sh\n\
                  if [ \"$1\" = \"--version\" ]; then echo 'claude 0.0.1'; exit 0; fi\n\
                  if [ -n \"$GG_TEST_MARKER\" ]; then echo called >> \"$GG_TEST_MARKER\"; fi\n\
                  echo '# AI Note'\n\
                  echo 'generated for testing'\n";
    fs::write(&path, script).expect("写入假 claude 脚本");

    let mut perms = fs::metadata(&path).expect("stat 假 claude").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("设置可执行位");
    path
}

#[cfg(windows)]
fn create_fake_claude(temp: &TempDir) -> PathBuf {
    let path = temp.path().join("fake-claude.cmd");
    let script = "@echo off\r\n\
                  if \"%1\"==\"--version\" (\r\n\
                    echo claude 0.0.1\r\n\
                    exit /b 0\r\n\
                  )\r\n\
                  if not \"%GG_TEST_MARKER%\"==\"\" echo called >> \"%GG_TEST_MARKER%\"\r\n\
                  echo # AI Note\r\n\
                  echo generated for testing\r\n";
    fs::write(&path, script).expect("写入假 claude 脚本");
    path
}

// ---------------------------------------------------------------- 基础查询

#[test]
fn query_existing_note_outputs_markdown() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    write_note(&notes_dir, "ls", "# ls\n列出目录内容\n");

    let mut cmd = command_for(&temp);
    cmd.args(["--notes-dir", notes_dir.to_str().expect("utf8"), "ls"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("# ls"))
        .stdout(predicate::str::contains("列出目录内容"));
}

#[test]
fn list_outputs_sorted_command_names() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    write_note(&notes_dir, "b", "# b\n");
    write_note(&notes_dir, "a", "# a\n");
    fs::write(notes_dir.join("ignore.txt"), "noop").expect("写入非 md 文件");

    let mut cmd = command_for(&temp);
    cmd.args(["--notes-dir", notes_dir.to_str().expect("utf8"), "list"]);

    cmd.assert().success().stdout("a\nb\n");
}

#[test]
fn search_matches_file_names_only() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    write_note(&notes_dir, "grep", "# grep\n正文包含 ls 关键字\n");
    write_note(&notes_dir, "ls", "# ls\n");

    let mut cmd = command_for(&temp);
    cmd.args([
        "--notes-dir",
        notes_dir.to_str().expect("utf8"),
        "search",
        "gr",
    ]);

    cmd.assert().success().stdout("grep\n");
}

// ---------------------------------------------------------------- 未命中路径

#[test]
fn missing_note_without_claude_returns_suggestions() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    write_note(&notes_dir, "ls", "# ls\n");

    let mut cmd = command_for(&temp);
    cmd.env("GG_CLAUDE_BIN", "__missing_claude_binary__");
    cmd.args(["--notes-dir", notes_dir.to_str().expect("utf8"), "lz"]);

    cmd.assert()
        .failure()
        .code(3)
        .stderr(predicate::str::contains("未找到命令 `lz` 的笔记"))
        .stderr(predicate::str::contains("ls"))
        .stderr(predicate::str::contains("已跳过 AI 回退"));
}

/// P0 回归：管道/重定向场景（stdout 非终端）绝不能调用 AI，也不能落盘。
#[test]
fn missing_note_in_piped_output_skips_ai_and_writes_nothing() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    fs::create_dir_all(&notes_dir).expect("创建笔记目录");
    let fake_claude = create_fake_claude(&temp);
    let marker = temp.path().join("claude-was-called");

    let mut cmd = command_for(&temp);
    cmd.env("GG_CLAUDE_BIN", fake_claude);
    cmd.env("GG_TEST_MARKER", &marker);
    cmd.args(["--notes-dir", notes_dir.to_str().expect("utf8"), "foo"]);

    cmd.assert()
        .failure()
        .code(3)
        .stderr(predicate::str::contains("非交互终端"));

    assert!(!marker.exists(), "非交互场景不得调用 claude 生成内容");
    assert!(
        !notes_dir.join("foo.md").exists(),
        "非交互场景不得写入笔记文件"
    );
}

// ---------------------------------------------------------------- AI 回退

/// 显式关闭确认（`ask_before_ai = false`）后，非交互场景也应允许生成。
#[test]
fn ai_opt_out_allows_non_interactive_generation() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    fs::create_dir_all(&notes_dir).expect("创建笔记目录");

    let config_dir = temp.path().join("appdata").join("gg");
    fs::create_dir_all(&config_dir).expect("创建配置目录");
    fs::write(
        config_dir.join("config.toml"),
        "language = \"zh\"\nask_before_ai = false\nauto_save_ai = true\nask_before_save = false\n",
    )
    .expect("写入配置");

    let fake_claude = create_fake_claude(&temp);
    let marker = temp.path().join("claude-was-called");

    let mut cmd = command_for(&temp);
    cmd.env("GG_CLAUDE_BIN", fake_claude);
    cmd.env("GG_TEST_MARKER", &marker);
    cmd.args(["--notes-dir", notes_dir.to_str().expect("utf8"), "foo"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("# AI Note"))
        .stderr(predicate::str::contains("已保存笔记"));

    assert!(marker.exists(), "显式选择后应当调用 claude");
    let saved = fs::read_to_string(notes_dir.join("foo.md")).expect("笔记应已保存");
    assert!(saved.contains("# AI Note"));
}

// ---------------------------------------------------------------- 配置

#[test]
fn lang_option_persists_and_localizes_help() {
    let temp = TempDir::new().expect("tempdir");

    let mut cmd = command_for(&temp);
    cmd.args(["--lang", "en"]);
    cmd.assert()
        .success()
        .stderr(predicate::str::contains("Language configuration saved"));

    // 第二次调用应读取已保存的语言，输出英文帮助。
    let mut cmd = command_for(&temp);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains(
            "Query your own command notes like man",
        ))
        .stdout(predicate::str::contains("Usage:"))
        .stdout(predicate::str::contains("Commands:"));
}

#[test]
fn lang_option_from_argv_localizes_help_immediately() {
    let temp = TempDir::new().expect("tempdir");

    let mut cmd = command_for(&temp);
    cmd.args(["--lang", "en", "--help"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains(
            "Query your own command notes like man",
        ))
        .stdout(predicate::str::contains("Usage:"));
}

#[test]
fn invalid_lang_option_fails_loudly() {
    let temp = TempDir::new().expect("tempdir");

    let mut cmd = command_for(&temp);
    cmd.args(["--lang", "jp"]);

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("jp"));
}

#[test]
fn set_editor_persists_to_config() {
    let temp = TempDir::new().expect("tempdir");

    let mut cmd = command_for(&temp);
    cmd.args(["--set-editor", "hx"]);
    cmd.assert().success();

    let config_dir = temp.path().join("appdata").join("gg");
    let raw = fs::read_to_string(config_dir.join("config.toml")).expect("配置文件应存在");
    assert!(raw.contains("editor = \"hx\""), "实际配置:\n{raw}");
}

#[test]
fn invalid_config_reports_the_offending_file() {
    let temp = TempDir::new().expect("tempdir");
    let config_dir = temp.path().join("appdata").join("gg");
    fs::create_dir_all(&config_dir).expect("创建配置目录");
    fs::write(config_dir.join("config.toml"), "language = \"jp\"\n").expect("写入非法配置");

    let mut cmd = command_for(&temp);
    cmd.arg("list");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("无法解析配置文件"))
        .stderr(predicate::str::contains("config.toml"));
}

#[test]
fn default_notes_dir_is_created_on_demand_not_on_list() {
    let temp = TempDir::new().expect("tempdir");

    let mut cmd = command_for(&temp);
    cmd.arg("list");

    cmd.assert().success();
    assert!(
        !temp
            .path()
            .join("appdata")
            .join("gg")
            .join("notes")
            .exists(),
        "只读操作不应创建笔记目录"
    );
}

#[test]
fn notes_dir_precedence_prefers_cli_flag_over_env() {
    let temp = TempDir::new().expect("tempdir");
    let cli_dir = temp.path().join("cli-notes");
    let env_dir = temp.path().join("env-notes");
    write_note(&cli_dir, "fromcli", "# fromcli\n");
    write_note(&env_dir, "fromenv", "# fromenv\n");

    let mut cmd = command_for(&temp);
    cmd.env("GG_NOTES_DIR", &env_dir);
    cmd.args(["--notes-dir", cli_dir.to_str().expect("utf8"), "list"]);

    cmd.assert().success().stdout("fromcli\n");
}

#[test]
fn path_traversal_is_rejected() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    fs::create_dir_all(&notes_dir).expect("创建笔记目录");

    let mut cmd = command_for(&temp);
    cmd.args([
        "--notes-dir",
        notes_dir.to_str().expect("utf8"),
        "../etc/passwd",
    ]);

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("unsupported path characters"));
}

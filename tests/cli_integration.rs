use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn command_for(temp: &TempDir) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("gg"));

    let appdata = temp.path().join("appdata");
    let home = temp.path().join("home");
    fs::create_dir_all(&appdata).expect("create appdata");
    fs::create_dir_all(&home).expect("create home");

    cmd.env("APPDATA", &appdata)
        .env("XDG_CONFIG_HOME", &appdata)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("NO_COLOR", "1");

    cmd
}

fn write_note(notes_dir: &Path, command: &str, markdown: &str) {
    fs::create_dir_all(notes_dir).expect("create notes directory");
    fs::write(notes_dir.join(format!("{command}.md")), markdown).expect("write note");
}

fn create_fake_claude(temp: &TempDir) -> std::path::PathBuf {
    let script_path = temp.path().join("fake-claude.cmd");
    let script = r#"@echo off
if "%1"=="--version" (
  echo claude 0.0.1
  exit /b 0
)
echo # AI Note
echo generated for testing
exit /b 0
"#;
    fs::write(&script_path, script).expect("write fake claude script");
    script_path
}

#[test]
fn query_existing_note_outputs_markdown() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    write_note(&notes_dir, "ls", "# ls\n列出目录内容\n");

    let mut cmd = command_for(&temp);
    cmd.args(["--notes-dir", notes_dir.to_str().expect("utf8 path"), "ls"]);

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
    fs::write(notes_dir.join("ignore.txt"), "noop").expect("write non-md file");

    let mut cmd = command_for(&temp);
    cmd.args([
        "--notes-dir",
        notes_dir.to_str().expect("utf8 path"),
        "list",
    ]);

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
        notes_dir.to_str().expect("utf8 path"),
        "search",
        "gr",
    ]);

    cmd.assert().success().stdout("grep\n");
}

#[test]
fn missing_note_without_claude_returns_suggestions() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    write_note(&notes_dir, "ls", "# ls\n");

    let mut cmd = command_for(&temp);
    cmd.env("GG_CLAUDE_BIN", "__missing_claude_binary__");
    cmd.args(["--notes-dir", notes_dir.to_str().expect("utf8 path"), "lz"]);

    cmd.assert()
        .success()
        .stderr(predicate::str::contains("未找到命令 `lz` 的笔记"))
        .stderr(predicate::str::contains("ls"))
        .stderr(predicate::str::contains("已跳过 AI 回退"));
}

#[test]
fn missing_note_with_claude_generates_and_saves() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    fs::create_dir_all(&notes_dir).expect("create notes dir");

    let fake_claude = create_fake_claude(&temp);

    let mut cmd = command_for(&temp);
    cmd.env("GG_CLAUDE_BIN", fake_claude);
    cmd.args(["--notes-dir", notes_dir.to_str().expect("utf8 path"), "foo"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("# AI Note"))
        .stderr(predicate::str::contains("已保存笔记"));

    let saved = fs::read_to_string(notes_dir.join("foo.md")).expect("saved note should exist");
    assert!(saved.contains("# AI Note"));
}

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
    // `GG_TEST_HANG` 用于验证ai_timeout_seconds 能终止不返回的子进程。
    let path = temp.path().join("fake-claude");
    // 模拟真实模型输出：以人手会写坏习惯的 `# 标题` 开头，用来验证会被剥离。
    let script = "#!/bin/sh\n\
                  if [ \"$1\" = \"--version\" ]; then echo 'claude 0.0.1'; exit 0; fi\n\
                  if [ -n \"$GG_TEST_HANG\" ]; then sleep 30; exit 0; fi\n\
                  if [ -n \"$GG_TEST_MARKER\" ]; then echo called >> \"$GG_TEST_MARKER\"; fi\n\
                  echo '# foo 命令速查'\n\
                  echo ''\n\
                  echo '`foo`：测试用命令。'\n\
                  echo ''\n\
                  echo '- `-x`：测试选项'\n";
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
                  if not \"%GG_TEST_HANG%\"==\"\" (\r\n\
                    ping -n 30 127.0.0.1 >nul\r\n\
                    exit /b 0\r\n\
                  )\r\n\
                  if not \"%GG_TEST_MARKER%\"==\"\" echo called >> \"%GG_TEST_MARKER%\"\r\n\
                  echo # foo 命令速查\r\n\
                  echo.\r\n\
                  echo `foo`：测试用命令。\r\n\
                  echo.\r\n\
                  echo - `-x`：测试选项\r\n";
    fs::write(&path, script).expect("写入假 claude 脚本");
    path
}

fn write_config(temp: &TempDir, body: &str) {
    let config_dir = temp.path().join("appdata").join("gg");
    fs::create_dir_all(&config_dir).expect("创建配置目录");
    fs::write(config_dir.join("config.toml"), body).expect("写入配置");
}

/// 创建一个会记录自己 argv 的假 AI 工具，返回 (可执行文件, argv 记录文件)。
fn fake_ai_tool(temp: &TempDir, name: &str) -> (PathBuf, PathBuf) {
    let record = temp.path().join(format!("{name}-argv.txt"));
    let bin = temp.path().join(name);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // 先记录 argv，再输出一段合法笔记正文，避免被判为「AI 返回空内容」。
        let body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {}\necho '`foo`：测试用命令。'\n",
            record.display()
        );
        fs::write(&bin, body).expect("写假 AI 工具");
        let mut perms = fs::metadata(&bin).expect("stat").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&bin, perms).expect("chmod");
    }
    #[cfg(windows)]
    {
        let body = format!(
            "@echo off\r\necho %* >> \"{}\"\r\necho `foo`：测试用命令。\r\n",
            record.display()
        );
        fs::write(&bin, body).expect("写假 AI 工具");
    }

    (bin, record)
}

/// 把目录插到子进程 PATH 最前面，用于让预设（claude/codex/gemini）命中假工具。
fn prepend_path(cmd: &mut assert_cmd::Command, dir: &Path) {
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let joined = std::env::join_paths(std::env::split_paths(&existing).collect::<Vec<_>>())
        .and_then(|paths| {
            std::env::join_paths(
                std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(&paths)),
            )
        })
        .expect("拼接 PATH");
    cmd.env("PATH", joined);
}

fn read_record(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
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

/// 默认只匹配文件名：搜索正文里的词不应命中。
#[test]
fn search_ignores_note_bodies_by_default() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    write_note(&notes_dir, "grep", "# grep\n正文包含关键字\n");

    let mut cmd = command_for(&temp);
    cmd.args([
        "--notes-dir",
        notes_dir.to_str().expect("utf8"),
        "search",
        "关键字",
    ]);

    cmd.assert().success().stdout("");
}

#[test]
fn search_content_flag_reports_file_line_and_text() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    write_note(&notes_dir, "grep", "# grep\n\n递归搜索目录\n递归时要小心\n");
    write_note(&notes_dir, "ls", "# ls\n列出目录\n");

    let mut cmd = command_for(&temp);
    cmd.args([
        "--notes-dir",
        notes_dir.to_str().expect("utf8"),
        "search",
        "-c",
        "递归",
    ]);

    cmd.assert()
        .success()
        .stdout("grep:3: 递归搜索目录\ngrep:4: 递归时要小心\n");
}

#[test]
fn search_content_flag_accepts_long_form() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    write_note(&notes_dir, "aws", "AWS CLI 用法\n");

    let mut cmd = command_for(&temp);
    cmd.args([
        "--notes-dir",
        notes_dir.to_str().expect("utf8"),
        "search",
        "--content",
        "aws",
    ]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("aws:1: AWS CLI 用法"));
}

// ---------------------------------------------------------------- 编辑笔记

#[test]
fn edit_notifies_when_it_creates_an_empty_note() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    fs::create_dir_all(&notes_dir).expect("创建笔记目录");

    // 用假编辑器，避开真实编辑器的交互阻塞。
    let editor = temp.path().join("noop-editor");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::write(&editor, "#!/bin/sh\nexit 0\n").expect("写假编辑器");
        let mut perms = fs::metadata(&editor).expect("stat").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&editor, perms).expect("chmod");
    }
    #[cfg(windows)]
    {
        fs::write(&editor, "@echo off\r\nexit /b 0\r\n").expect("写假编辑器");
    }

    let mut cmd = command_for(&temp);
    cmd.env("GG_EDITOR", &editor);
    cmd.args([
        "--notes-dir",
        notes_dir.to_str().expect("utf8"),
        "--edit",
        "newcmd",
    ]);

    cmd.assert()
        .success()
        .stderr(predicate::str::contains("已新建空笔记"));
    assert!(notes_dir.join("newcmd.md").exists());
}

#[test]
fn edit_does_not_claim_creation_for_an_existing_note() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    write_note(&notes_dir, "ls", "# ls\n");

    let editor = temp.path().join("noop-editor");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::write(&editor, "#!/bin/sh\nexit 0\n").expect("写假编辑器");
        let mut perms = fs::metadata(&editor).expect("stat").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&editor, perms).expect("chmod");
    }
    #[cfg(windows)]
    {
        fs::write(&editor, "@echo off\r\nexit /b 0\r\n").expect("写假编辑器");
    }

    let mut cmd = command_for(&temp);
    cmd.env("GG_EDITOR", &editor);
    cmd.args([
        "--notes-dir",
        notes_dir.to_str().expect("utf8"),
        "--edit",
        "ls",
    ]);

    cmd.assert()
        .success()
        .stderr(predicate::str::contains("已新建空笔记").not());
    let content = fs::read_to_string(notes_dir.join("ls.md")).expect("读取笔记");
    assert_eq!(content, "# ls\n", "已有笔记内容不得被清空");
}

/// 回归：README 声明的是一条优先级链
/// `config.editor > GG_EDITOR > VISUAL > EDITOR > 终端编辑器`。
/// 配置里的编辑器只是没装时，必须继续往下试环境变量，
/// 而不是直接跳到 nvim。
#[test]
fn missing_configured_editor_falls_through_to_env_editor() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    write_note(&notes_dir, "ls", "# ls\n");

    let marker = temp.path().join("env-editor-ran");
    let env_editor = temp.path().join("env-editor");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let body = format!("#!/bin/sh\necho \"$1\" > {}\n", marker.display());
        fs::write(&env_editor, body).expect("写假编辑器");
        let mut perms = fs::metadata(&env_editor).expect("stat").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&env_editor, perms).expect("chmod");
    }
    #[cfg(windows)]
    {
        let body = format!("@echo off\r\necho %1 > \"{}\"\r\n", marker.display());
        fs::write(&env_editor, body).expect("写假编辑器");
    }

    write_config(
        &temp,
        "language = \"zh\"\neditor = \"__gg_no_such_editor__\"\n",
    );

    let mut cmd = command_for(&temp);
    cmd.env("GG_EDITOR", &env_editor);
    cmd.args([
        "--notes-dir",
        notes_dir.to_str().expect("utf8"),
        "--edit",
        "ls",
    ]);

    cmd.assert()
        .success()
        .stderr(predicate::str::contains("__gg_no_such_editor__"));

    let recorded =
        fs::read_to_string(&marker).expect("GG_EDITOR 指定的编辑器应被调用，而不是直接跳去开 nvim");
    assert!(
        recorded.contains("ls.md"),
        "应把笔记路径传给环境变量编辑器，实际: {recorded}"
    );
}

/// 编辑器执行失败必须报错，而不是默默换一个编辑器。
#[test]
fn failing_editor_is_reported_instead_of_silently_falling_back() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    write_note(&notes_dir, "ls", "# ls\n");

    let editor = temp.path().join("failing-editor");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::write(&editor, "#!/bin/sh\nexit 3\n").expect("写假编辑器");
        let mut perms = fs::metadata(&editor).expect("stat").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&editor, perms).expect("chmod");
    }
    #[cfg(windows)]
    {
        fs::write(&editor, "@echo off\r\nexit /b 3\r\n").expect("写假编辑器");
    }

    let mut cmd = command_for(&temp);
    cmd.env("GG_EDITOR", &editor);
    cmd.args([
        "--notes-dir",
        notes_dir.to_str().expect("utf8"),
        "--edit",
        "ls",
    ]);

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("执行失败"))
        .stderr(predicate::str::contains("退出码"));
}

/// `--yes` 应能在非交互场景授权生成，且不用改配置。
#[test]
fn yes_flag_authorizes_non_interactive_generation() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    fs::create_dir_all(&notes_dir).expect("创建笔记目录");
    // 保持默认的「必须先问我」策略，仅靠 --yes 授权
    write_config(
        &temp,
        "language = \"zh\"\nask_before_ai = true\nauto_save_ai = true\nask_before_save = true\n",
    );

    let fake_claude = create_fake_claude(&temp);
    let marker = temp.path().join("claude-was-called");

    let mut cmd = command_for(&temp);
    cmd.env("GG_CLAUDE_BIN", fake_claude);
    cmd.env("GG_TEST_MARKER", &marker);
    cmd.args([
        "--notes-dir",
        notes_dir.to_str().expect("utf8"),
        "--yes",
        "foo",
    ]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("`foo`：测试用命令。"))
        .stderr(predicate::str::contains("已保存笔记"));

    assert!(marker.exists(), "--yes 应当授权调用 claude");
    assert!(notes_dir.join("foo.md").exists());
}

#[test]
fn yes_flag_is_documented_in_help() {
    for (lang, flag, keyword) in [
        ("zh", "-y, --yes", "对所有询问自动回答"),
        ("en", "-y, --yes", "Answer yes to every prompt"),
    ] {
        let mut cmd = command_for(&TempDir::new().expect("tempdir"));
        cmd.args(["--lang", lang, "--help"]);
        cmd.assert()
            .success()
            .stdout(predicate::str::contains(flag))
            .stdout(predicate::str::contains(keyword));
    }
}

/// 单个条目读不了时，列表必须仍然输出，并在 stderr 告知结果可能不完整。
#[cfg(unix)]
#[test]
fn list_survives_unreadable_entry_and_warns() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    write_note(&notes_dir, "ls", "# ls\n");
    fs::write(notes_dir.join(OsStr::from_bytes(b"\xff\xfe.md")), "# bad\n").expect("写非法文件名");

    let mut cmd = command_for(&temp);
    cmd.args(["--notes-dir", notes_dir.to_str().expect("utf8"), "list"]);

    cmd.assert()
        .success()
        .stdout("ls\n")
        .stderr(predicate::str::contains("无法读取"));
}

// ---------------------------------------------------------------- AI 后端选择

#[test]
fn ai_provider_none_disables_fallback() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    fs::create_dir_all(&notes_dir).expect("创建笔记目录");
    write_config(&temp, "language = \"zh\"\nai_provider = \"none\"\n");

    let mut cmd = command_for(&temp);
    // 即便有可用的 claude，也不应被调用
    let (tool, record) = fake_ai_tool(&temp, "claude");
    cmd.env("GG_AI_BIN", &tool);
    cmd.args(["--notes-dir", notes_dir.to_str().expect("utf8"), "foo"]);

    cmd.assert()
        .failure()
        .code(3)
        .stderr(predicate::str::contains("AI 回退已关闭"));

    assert!(
        !record.exists(),
        "ai_provider = none 时不应调用任何 AI 工具"
    );
}

/// `ai_command` 优先级高于 `ai_provider` 预设，包括 none。
#[test]
fn custom_ai_command_takes_precedence_over_preset() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    fs::create_dir_all(&notes_dir).expect("创建笔记目录");
    let (tool, record) = fake_ai_tool(&temp, "mytool");

    write_config(
        &temp,
        &format!(
            "language = \"zh\"\nai_provider = \"none\"\nai_command = \"{} --flag\"\nask_before_ai = false\nauto_save_ai = false\n",
            tool.display()
        ),
    );

    let mut cmd = command_for(&temp);
    cmd.args(["--notes-dir", notes_dir.to_str().expect("utf8"), "foo"]);
    cmd.assert().success();

    let argv = read_record(&record);
    assert!(
        argv.contains("--flag"),
        "自定义命令的参数应被传递，实际 argv:\n{argv}"
    );
    // 提示词作为最后一个参数追加，且必须是优化后的新提示词
    assert!(
        argv.contains("10~25 行"),
        "应使用简洁版提示词，实际 argv:\n{argv}"
    );
    assert!(
        argv.contains("foo"),
        "提示词应指明目标命令，实际 argv:\n{argv}"
    );
}

/// 预设 provider 应当拼出各自的非交互调用形式。
#[test]
fn provider_presets_use_their_own_arguments() {
    for (provider, bin_name, expected_arg) in [
        ("claude", "claude", "--output-format"),
        ("codex", "codex", "exec"),
        ("gemini", "gemini", "-p"),
    ] {
        let temp = TempDir::new().expect("tempdir");
        let notes_dir = temp.path().join("notes");
        fs::create_dir_all(&notes_dir).expect("创建笔记目录");
        let bin_dir = temp.path().join("bin");
        fs::create_dir_all(&bin_dir).expect("创建 bin 目录");

        let (tool, record) = fake_ai_tool(&temp, bin_name);
        fs::rename(&tool, bin_dir.join(bin_name)).expect("移动假工具到 bin");

        write_config(
            &temp,
            &format!(
                "language = \"zh\"\nai_provider = \"{provider}\"\nask_before_ai = false\nauto_save_ai = false\n"
            ),
        );

        let mut cmd = command_for(&temp);
        prepend_path(&mut cmd, &bin_dir);
        cmd.args(["--notes-dir", notes_dir.to_str().expect("utf8"), "foo"]);
        cmd.assert().success();

        let argv = read_record(&record);
        assert!(
            argv.contains(expected_arg),
            "{provider} 预设应包含 `{expected_arg}`，实际 argv:\n{argv}"
        );
    }
}

/// `GG_AI_BIN` 只替换可执行文件，保留预设参数。
#[test]
fn gg_ai_bin_overrides_preset_binary_only() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    fs::create_dir_all(&notes_dir).expect("创建笔记目录");
    let (tool, record) = fake_ai_tool(&temp, "claude");

    write_config(
        &temp,
        "language = \"zh\"\nai_provider = \"claude\"\nask_before_ai = false\nauto_save_ai = false\n",
    );

    let mut cmd = command_for(&temp);
    cmd.env("GG_AI_BIN", &tool);
    cmd.args(["--notes-dir", notes_dir.to_str().expect("utf8"), "foo"]);
    cmd.assert().success();

    let argv = read_record(&record);
    assert!(
        argv.contains("--output-format"),
        "GG_AI_BIN 只该换二进制，预设参数应保留，实际 argv:\n{argv}"
    );
}

/// 非终端（管道）下不显示转圈，但必须有静态提示，否则用户会以为卡死。
#[test]
fn generation_prints_progress_when_not_a_terminal() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    fs::create_dir_all(&notes_dir).expect("创建笔记目录");
    let fake_claude = create_fake_claude(&temp);

    let mut cmd = command_for(&temp);
    cmd.env("GG_CLAUDE_BIN", fake_claude);
    cmd.args([
        "--notes-dir",
        notes_dir.to_str().expect("utf8"),
        "-y",
        "foo",
    ]);

    cmd.assert()
        .success()
        .stderr(predicate::str::contains("正在生成 `foo` 的笔记"));
}

// ---------------------------------------------------------------- 列布局

/// 管道场景必须保持每行一条，否则 `gg list | grep x` 之类的脚本会失效。
#[test]
fn list_keeps_one_entry_per_line_when_piped() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    for name in ["alias", "apt", "awk", "bear", "cat", "chmod"] {
        write_note(&notes_dir, name, "# x\n");
    }

    let mut cmd = command_for(&temp);
    cmd.args(["--notes-dir", notes_dir.to_str().expect("utf8"), "list"]);

    cmd.assert()
        .success()
        .stdout("alias\napt\nawk\nbear\ncat\nchmod\n");
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
        .stdout(predicate::str::contains("`foo`：测试用命令。"))
        .stderr(predicate::str::contains("已保存笔记"));

    assert!(marker.exists(), "显式选择后应当调用 claude");
    let saved = fs::read_to_string(notes_dir.join("foo.md")).expect("笔记应已保存");
    assert!(
        saved.contains("`foo`：测试用命令。"),
        "实际:
{saved}"
    );
    assert!(
        !saved.contains("# foo 命令速查"),
        "模型加的首行标题应被剥离，实际:
{saved}"
    );
}

/// 不返回的 claude 必须被超时终止，而不是让 `gg` 永久挂住。
#[test]
fn hanging_claude_is_terminated_by_timeout() {
    let temp = TempDir::new().expect("tempdir");
    let notes_dir = temp.path().join("notes");
    fs::create_dir_all(&notes_dir).expect("创建笔记目录");
    write_config(
        &temp,
        "language = \"zh\"\nask_before_ai = false\nauto_save_ai = true\nai_timeout_seconds = 1\n",
    );

    let fake_claude = create_fake_claude(&temp);

    let mut cmd = command_for(&temp);
    cmd.env("GG_CLAUDE_BIN", fake_claude);
    cmd.env("GG_TEST_HANG", "1");
    cmd.args(["--notes-dir", notes_dir.to_str().expect("utf8"), "foo"]);

    let started = std::time::Instant::now();
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("未返回"));
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(20),
        "应在超时后立即返回，实际耗时 {elapsed:?}"
    );
    assert!(!notes_dir.join("foo.md").exists(), "生成失败时不应写入笔记");
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

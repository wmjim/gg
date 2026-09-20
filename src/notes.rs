use crate::error;
use crate::i18n::Language;
use anyhow::{Context, Result};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub fn note_path(notes_dir: &Path, command: &str) -> PathBuf {
    notes_dir.join(format!("{command}.md"))
}

/// Windows 文件名非法字符。`/` 与 `\` 同时也是路径分隔符；这里在所有平台
/// 一律拒绝 —— 真实命令名几乎不会包含它们，而笔记目录可能被同步到 Windows。
const INVALID_NAME_CHARS: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// Windows 保留设备名（大小写不敏感，带扩展名同样保留）。
const RESERVED_DEVICE_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// 校验命令名。
///
/// 命令名直接来自命令行，所以校验失败属于**用法错误**（退出码 2），不是
/// 运行时错误：命令行本身就用错了。
pub fn validate_command_name(command: &str, lang: Language) -> Result<()> {
    if command.is_empty() {
        return Err(error::usage(lang.note_command_empty()));
    }
    if command.chars().any(char::is_whitespace) {
        return Err(error::usage(lang.note_command_has_whitespace()));
    }
    if command.chars().any(|c| INVALID_NAME_CHARS.contains(&c)) {
        return Err(error::usage(lang.note_command_has_path_chars()));
    }
    if is_reserved_device_name(command) {
        return Err(error::usage(lang.note_command_is_reserved(command)));
    }
    Ok(())
}

/// Windows 上 `CON`、`NUL`、`COM1` 这类设备名不能作为文档名，`CON.md` 也算。
/// 命令名里已经排除了路径分隔符，这里只比较扩展名之前的部分。
fn is_reserved_device_name(command: &str) -> bool {
    let stem = command.split('.').next().unwrap_or(command);
    RESERVED_DEVICE_NAMES
        .iter()
        .any(|name| stem.eq_ignore_ascii_case(name))
}

/// [`ensure_note_file`] 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsuredNote {
    pub path: PathBuf,
    /// 本次是否新建了空文件（用于提示用户，避免默默多出一个笔记）。
    pub created: bool,
}

/// 扫描笔记目录时被跳过的条目。
///
/// 单个条目的权限问题或竞态（扫描期间被删除）不应让整个列表失败，
/// 但也不能静默忽略——用户需要知道结果可能不完整。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub path: PathBuf,
    pub reason: String,
}

/// 一次扫描或搜索的结果。
///
/// `skipped` 记录因错误被跳过的条目；调用方必须把它告知用户，
/// 否则列表看起来「完整」却在骗人。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Found<T> {
    pub items: Vec<T>,
    pub skipped: Vec<Skipped>,
}

impl<T> Found<T> {
    fn none() -> Self {
        Self {
            items: Vec::new(),
            skipped: Vec::new(),
        }
    }
}

/// 正文搜索命中的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentMatch {
    pub command: String,
    /// 1 起始的行号。
    pub line_number: usize,
    pub line: String,
}

impl ContentMatch {
    /// grep 风格输出：`<command>:<行号>: <内容>`。
    ///
    /// `line` 是原文整行（含行首空白与行尾空白），因此输出可直接与源文件对拍。
    pub fn render(&self) -> String {
        format!("{}:{}: {}", self.command, self.line_number, self.line)
    }
}

pub fn read_note(notes_dir: &Path, command: &str, lang: Language) -> Result<Option<String>> {
    let path = note_path(notes_dir, command);
    // 同名目录不是笔记：按「没有笔记」处理，而不是把它当文件读而报 EISDIR，
    // 也不是把目录交给编辑器。这里与 `remove_note` 的判断保持一致。
    if !path.is_file() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path)
        .with_context(|| lang.note_read_failed(&path.display().to_string()))?;
    Ok(Some(content))
}

pub fn write_note(
    notes_dir: &Path,
    command: &str,
    content: &str,
    lang: Language,
) -> Result<PathBuf> {
    fs::create_dir_all(notes_dir)
        .with_context(|| lang.notes_dir_create_failed(&notes_dir.display().to_string()))?;

    let path = note_path(notes_dir, command);
    let target = path.display().to_string();
    // 原子写：AI 生成的内容可能很长，中途被杀不能留下半截笔记。
    // 临时文件名带 `.tmp` 后缀（而非 `.md`），因此即使崩溃残留也不会被
    // `scan_commands` 当成一条笔记。
    crate::utils::atomic::replace_file(&path, content, ".gg-note-", &|| {
        lang.note_write_failed(&target)
    })?;
    Ok(path)
}

/// 确保笔记文件存在，供 `--edit` 打开。
///
/// 用 `create_new`（`O_CREAT | O_EXCL`）而不「先 `is_file()` 再 `fs::write`」：
/// 后者在检查与写入之间存在窗口，并发的另一次 `gg -e`、或恰好落盘的 AI 笔记，
/// 都可能被 `fs::write` 截断成空文件。「不存在才创建」正是 `O_EXCL` 的语义，
/// 它无法覆盖任何已有内容。顺带也不再跟随悬空符号链接去目录外建文件。
pub fn ensure_note_file(notes_dir: &Path, command: &str, lang: Language) -> Result<EnsuredNote> {
    fs::create_dir_all(notes_dir)
        .with_context(|| lang.notes_dir_create_failed(&notes_dir.display().to_string()))?;

    let path = note_path(notes_dir, command);
    let target = path.display().to_string();

    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(_) => Ok(EnsuredNote {
            path,
            created: true,
        }),
        // 已存在：可能是笔记，也可能是同名目录（或悬空符号链接）。
        // 只有真实文件才交回给调用方打开 —— 把目录交给编辑器是另一类错误。
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            if path.is_file() {
                Ok(EnsuredNote {
                    path,
                    created: false,
                })
            } else {
                Err(err).with_context(|| lang.note_create_failed(&target))
            }
        }
        Err(err) => Err(err).with_context(|| lang.note_create_failed(&target)),
    }
}

/// 删除笔记文件。返回 `false` 表示该命令本来就没有笔记。
pub fn remove_note(notes_dir: &Path, command: &str, lang: Language) -> Result<bool> {
    let path = note_path(notes_dir, command);
    if !path.is_file() {
        return Ok(false);
    }

    fs::remove_file(&path).with_context(|| lang.note_remove_failed(&path.display().to_string()))?;
    Ok(true)
}

/// 列出笔记命令，同时返回无法读取而被跳过的条目。
pub fn scan_commands(notes_dir: &Path, lang: Language) -> Result<Found<String>> {
    if !notes_dir.exists() {
        return Ok(Found::none());
    }

    let mut commands = Vec::new();
    let mut skipped = Vec::new();

    let entries = fs::read_dir(notes_dir)
        .with_context(|| lang.notes_dir_read_failed(&notes_dir.display().to_string()))?;

    for entry in entries {
        // 单条 readdir 失败（权限、竞态删除）只跳过该条，不放弃整个目录。
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                skipped.push(Skipped {
                    path: notes_dir.to_path_buf(),
                    reason: err.to_string(),
                });
                continue;
            }
        };

        let path = entry.path();
        match entry.file_type() {
            Ok(file_type) if file_type.is_file() => {}
            // 目录、符号链接等不是笔记文件，静默忽略。
            Ok(_) => continue,
            Err(err) => {
                skipped.push(Skipped {
                    path,
                    reason: err.to_string(),
                });
                continue;
            }
        }

        let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        if !ext.eq_ignore_ascii_case("md") {
            continue;
        }

        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            skipped.push(Skipped {
                path,
                reason: "file name is not valid UTF-8".to_string(),
            });
            continue;
        };
        commands.push(stem.to_string());
    }

    commands.sort();
    Ok(Found {
        items: commands,
        skipped,
    })
}

pub fn search_commands_by_name(
    notes_dir: &Path,
    keyword: &str,
    lang: Language,
) -> Result<Found<String>> {
    let keyword = keyword.to_ascii_lowercase();
    let found = scan_commands(notes_dir, lang)?;
    let mut items: Vec<String> = found
        .items
        .into_iter()
        .filter(|command| command.to_ascii_lowercase().contains(&keyword))
        .collect();
    items.sort();
    Ok(Found {
        items,
        skipped: found.skipped,
    })
}

/// 按正文内容搜索笔记，返回 grep 风格的命中行。
///
/// 刻意不做 Markdown 语法剥离，也**不 trim 行首空白**：用户能直接看到原文上下文，
/// 行为可预测。行首空白承载着信息（代码块缩进、列表层级），抹掉它就看不到匹配
/// 所在的上下文了 —— 与 `grep` / `ripgrep` 的取行方式保持一致。
pub fn search_notes_by_content(
    notes_dir: &Path,
    keyword: &str,
    lang: Language,
) -> Result<Found<ContentMatch>> {
    let needle = keyword.to_ascii_lowercase();
    if needle.is_empty() {
        return Ok(Found::none());
    }

    let found = scan_commands(notes_dir, lang)?;
    let mut skipped = found.skipped;
    let mut items = Vec::new();

    for command in found.items {
        let content = match read_note(notes_dir, &command, lang) {
            Ok(Some(content)) => content,
            Ok(None) => continue,
            // 单个笔记读不了不应中断整次搜索。
            Err(err) => {
                skipped.push(Skipped {
                    path: note_path(notes_dir, &command),
                    reason: format!("{err:#}"),
                });
                continue;
            }
        };

        for (index, line) in content.lines().enumerate() {
            if line.to_ascii_lowercase().contains(&needle) {
                items.push(ContentMatch {
                    command: command.clone(),
                    line_number: index + 1,
                    // 逐字保留：`lines()` 已处理 `\r\n`，无需再靠 trim 清理行尾。
                    line: line.to_string(),
                });
            }
        }
    }

    Ok(Found { items, skipped })
}

pub fn suggest_commands(query: &str, commands: &[String], limit: usize) -> Vec<String> {
    let query = query.to_ascii_lowercase();
    let mut scored: Vec<(f64, &String)> = commands
        .iter()
        .map(|command| {
            let candidate = command.to_ascii_lowercase();
            let mut score = if candidate.contains(&query) {
                2.0
            } else {
                normalized_similarity(&candidate, &query)
            };
            if query.starts_with(&candidate) || candidate.starts_with(&query) {
                score += 1.0;
            }
            (score, command)
        })
        .collect();

    scored.sort_by(|(score_a, name_a), (score_b, name_b)| {
        score_b.total_cmp(score_a).then_with(|| name_a.cmp(name_b))
    });

    scored
        .into_iter()
        .filter(|(score, _)| *score > 0.0)
        .take(limit)
        .map(|(_, command)| command.clone())
        .collect()
}

fn normalized_similarity(candidate: &str, query: &str) -> f64 {
    let max_len = candidate.len().max(query.len()) as f64;
    if max_len == 0.0 {
        return 0.0;
    }

    let distance = strsim::levenshtein(candidate, query) as f64;
    (1.0 - (distance / max_len)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_name_validation() {
        assert!(validate_command_name("ls", Language::Zh).is_ok());
        assert!(validate_command_name("git-status", Language::Zh).is_ok());
        assert!(validate_command_name("docker run", Language::Zh).is_err());
        assert!(validate_command_name("../ls", Language::Zh).is_err());
        assert!(validate_command_name("C:ls", Language::Zh).is_err());
    }

    /// 校验失败的原因必须跟着界面语言走，不能泄漏 `notes.rs` 内部的英文。
    #[test]
    fn command_name_errors_are_localized() {
        let zh = validate_command_name("docker run", Language::Zh).expect_err("含空格应被拒");
        assert!(format!("{zh:#}").contains("不能含空格"), "{zh:#}");
        assert!(
            format!("{zh:#}").contains("git-log.md"),
            "多词查询应给出可操作的命名提示: {zh:#}"
        );

        let en = validate_command_name("docker run", Language::En).expect_err("含空格应被拒");
        assert!(
            format!("{en:#}").contains("cannot contain spaces"),
            "{en:#}"
        );

        let traversal = validate_command_name("../ls", Language::En).expect_err("路径穿越应被拒");
        assert!(
            format!("{traversal:#}").contains("unsupported path characters"),
            "{traversal:#}"
        );

        let reserved = validate_command_name("nul", Language::En).expect_err("保留设备名应被拒");
        assert!(format!("{reserved:#}").contains("nul"), "{reserved:#}");
    }

    /// Windows 下这些字符与名字不能做文件名。跨平台一律拒绝，免得笔记目录
    /// 同步到 Windows 之后才炸。
    #[test]
    fn windows_illegal_names_are_rejected_everywhere() {
        for name in ["a?b", "a*b", "a\"b", "a<b", "a>b", "a|b"] {
            assert!(
                validate_command_name(name, Language::Zh).is_err(),
                "`{name}` 含 Windows 非法字符，应被拒绝"
            );
        }

        for name in ["con", "CON", "nul", "aux", "com1", "COM9", "lpt9", "con.md"] {
            assert!(
                validate_command_name(name, Language::Zh).is_err(),
                "`{name}` 是保留设备名，应被拒绝"
            );
        }

        // 近似名字不能误伤
        for name in ["console", "com10", "nulify", "lpt", "ls"] {
            assert!(
                validate_command_name(name, Language::Zh).is_ok(),
                "`{name}` 应当允许"
            );
        }
    }

    #[test]
    fn suggestions_are_sorted_by_similarity() {
        let commands = vec![
            "ls".to_string(),
            "grep".to_string(),
            "lsof".to_string(),
            "less".to_string(),
        ];

        let suggestions = suggest_commands("lss", &commands, 3);
        assert_eq!(suggestions, vec!["ls", "less", "lsof"]);
    }

    #[test]
    fn ensure_note_file_reports_whether_it_created_the_file() {
        let temp = tempfile::tempdir().expect("tempdir");

        let first = ensure_note_file(temp.path(), "ls", Language::Zh).expect("新建笔记");
        assert!(first.created, "首次调用应报告已创建");
        assert!(first.path.exists());

        fs::write(&first.path, "# ls\n").expect("写入内容");
        let second = ensure_note_file(temp.path(), "ls", Language::Zh).expect("已有笔记");
        assert!(!second.created, "已存在时不得报告创建");
        let content = fs::read_to_string(&second.path).expect("读取笔记");
        assert_eq!(content, "# ls\n", "已存在的笔记内容不得被清空");
    }

    /// `write_note` 负责按需建目录：AI 生成时笔记目录可能还不存在。
    #[test]
    fn write_note_creates_the_notes_directory_on_demand() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nested = temp.path().join("a").join("b");

        let path = write_note(&nested, "ls", "# ls\n", Language::Zh).expect("按需建目录并写入");

        assert_eq!(path, nested.join("ls.md"));
        assert_eq!(fs::read_to_string(&path).expect("读取"), "# ls\n");
    }

    /// 写入是「原子替换」：旧内容完整消失，且不留临时文件。
    #[test]
    fn write_note_replaces_content_without_leaving_temp_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_note(
            temp.path(),
            "ls",
            "# 旧内容，比新内容长很多\n",
            Language::Zh,
        )
        .expect("首次写入");
        write_note(temp.path(), "ls", "# 新\n", Language::Zh).expect("覆盖写入");

        assert_eq!(
            fs::read_to_string(temp.path().join("ls.md")).expect("读取"),
            "# 新\n"
        );
        assert_eq!(
            note_file_names(temp.path()),
            vec!["ls.md"],
            "不得留下临时文件"
        );
    }

    /// 崩溃残留的临时文件不能冒充笔记。
    ///
    /// 原子写用「同目录临时文件 + rename」，只在写与 rename 之间被杀时才留残留。
    /// 残留之所以无害，靠的是它带 `.tmp` 而不是 `.md` 后缀 —— 这条不变量一旦
    /// 破了，`gg list` 就会多出一条以随机名命名的“笔记”。
    #[test]
    fn scan_commands_ignores_leftover_write_temp_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_note(temp.path(), "ls", "# ls\n", Language::Zh).expect("写笔记");
        fs::write(temp.path().join(".gg-note-AbC123.tmp"), "半截内容").expect("模拟崩溃残留");

        let found = scan_commands(temp.path(), Language::Zh).expect("扫描不应整体失败");

        assert_eq!(found.items, vec!["ls".to_string()], "残留文件不得冒充笔记");
        assert!(found.skipped.is_empty(), "残留文件不是错误，不该上报");
    }

    /// 目录下所有条目的文件名（排序），用于断言「没留下垃圾」。
    #[cfg(test)]
    fn note_file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("读目录")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn content_search_reports_line_numbers_and_skips_missing_keywords() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(
            temp.path().join("grep.md"),
            "# grep\n\n递归搜索目录\n递归时要小心\n",
        )
        .expect("写笔记");
        fs::write(temp.path().join("ls.md"), "# ls\n列出目录\n").expect("写笔记");
        fs::write(temp.path().join("ignore.txt"), "递归").expect("写非 md 文件");

        let found = search_notes_by_content(temp.path(), "递归", Language::Zh).expect("搜索正文");
        let rendered: Vec<String> = found.items.iter().map(ContentMatch::render).collect();

        assert_eq!(
            rendered,
            vec!["grep:3: 递归搜索目录", "grep:4: 递归时要小心"]
        );
        assert!(found.skipped.is_empty(), "不该有被跳过的条目");

        let none =
            search_notes_by_content(temp.path(), "不存在的词", Language::Zh).expect("搜索正文");
        assert!(none.items.is_empty());
    }

    #[test]
    fn remove_note_deletes_the_file_and_reports_absence() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_note(temp.path(), "ls", "# ls\n", Language::Zh).expect("写笔记");
        write_note(temp.path(), "grep", "# grep\n", Language::Zh).expect("写笔记");

        assert!(
            remove_note(temp.path(), "ls", Language::Zh).expect("删除成功"),
            "存在则应删除"
        );
        assert!(!temp.path().join("ls.md").exists());
        assert!(temp.path().join("grep.md").exists(), "不应误删其它笔记");

        assert!(
            !remove_note(temp.path(), "ls", Language::Zh).expect("再次删除不报错"),
            "已不存在应返回 false"
        );
    }

    /// 同名目录不是笔记：查询时按「没有笔记」处理，而不是把目录当文件读。
    #[test]
    fn read_note_ignores_a_directory_with_the_same_name() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(temp.path().join("somedir.md")).expect("建同名目录");

        let content = read_note(temp.path(), "somedir", Language::Zh).expect("目录不算读取错误");
        assert_eq!(content, None, "目录不应该被当成笔记内容读出来");
    }

    /// `gg -e` 走 `ensure_note_file`：同名目录不能被当成已有笔记，否则编辑器
    /// 会被交去打开一个目录。
    #[test]
    fn ensure_note_file_rejects_a_directory_with_the_same_name() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(temp.path().join("somedir.md")).expect("建同名目录");

        let err = ensure_note_file(temp.path(), "somedir", Language::Zh)
            .expect_err("同名目录必须报错，而不是把目录交给编辑器");
        assert!(format!("{err:#}").contains("无法新建笔记"), "{err:#}");
        assert!(temp.path().join("somedir.md").is_dir(), "目录必须还在");
    }

    /// 悬空符号链接必须被拒绝，而不是跟着它到笔记目录**之外**创建文件。
    ///
    /// `create_new`（`O_EXCL`）对符号链接一律返回 EEXIST，因此不会跟踪链接去建
    /// 目标；若沿用「先 `is_file()` 再 `fs::write`」的写法，`is_file()` 会因目标
    /// 不存在而返回 false，`fs::write` 则会顺着链接在目录外建出文件。
    #[cfg(unix)]
    #[test]
    fn ensure_note_file_refuses_a_dangling_symlink() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let outside = temp.path().join("outside.md");
        symlink(&outside, temp.path().join("ls.md")).expect("造悬空符号链接");

        let err = ensure_note_file(temp.path(), "ls", Language::Zh).expect_err("必须拒绝");

        assert!(format!("{err:#}").contains("无法新建笔记"), "{err:#}");
        assert!(!outside.exists(), "不得跟着符号链接在笔记目录外建文件");
    }

    /// 指向真实文件的符号链接仍照常工作，内容不被清空。
    #[cfg(unix)]
    #[test]
    fn ensure_note_file_accepts_an_existing_symlinked_note() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let real = temp.path().join("real.md");
        fs::write(&real, "# ls\n").expect("写真笔记");
        symlink(&real, temp.path().join("ls.md")).expect("造符号链接");

        let ensured = ensure_note_file(temp.path(), "ls", Language::Zh).expect("已有笔记应被接受");

        assert!(!ensured.created);
        assert_eq!(
            fs::read_to_string(&real).expect("读取"),
            "# ls\n",
            "内容不得被清空"
        );
    }

    #[test]
    fn remove_note_does_not_follow_a_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(temp.path().join("somedir.md")).expect("建同名目录");

        assert!(
            !remove_note(temp.path(), "somedir", Language::Zh).expect("目录不当文件删"),
            "同名目录不应被当作笔记删除"
        );
        assert!(temp.path().join("somedir.md").is_dir());
    }

    #[test]
    fn scan_commands_skips_unreadable_entries_and_reports_them() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("ls.md"), "# ls\n").expect("写笔记");

        let Some(_bad) = write_non_utf8_note(temp.path()) else {
            eprintln!("[跳过] 当前文件系统不接受非 UTF-8 文件名");
            return;
        };

        let found = scan_commands(temp.path(), Language::Zh).expect("扫描目录不应整体失败");
        assert_eq!(found.items, vec!["ls".to_string()]);
        assert_eq!(found.skipped.len(), 1, "非法文件名应被记录并跳过");
    }

    /// 造一个「文件名不是合法 UTF-8」的笔记。
    ///
    /// 并非所有文件系统都接受这种名字 —— macOS 的 APFS 直接返回 `EILSEQ`，
    /// Windows 的文件名本就是 UTF-16。返回 `None` 表示当前平台造不出来，
    /// 由调用方跳过该场景，而不是让测试失败。
    #[cfg(unix)]
    fn write_non_utf8_note(dir: &Path) -> Option<PathBuf> {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let path = dir.join(OsStr::from_bytes(b"\xff\xfe.md"));
        fs::write(&path, "# bad\n").ok().map(|()| path)
    }

    #[cfg(not(unix))]
    fn write_non_utf8_note(_dir: &Path) -> Option<PathBuf> {
        None
    }

    #[test]
    fn scan_commands_on_missing_directory_is_empty_not_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let missing = temp.path().join("nope");

        let found = scan_commands(&missing, Language::Zh).expect("目录不存在不是错误");
        assert!(found.items.is_empty());
        assert!(found.skipped.is_empty());
    }

    #[test]
    fn content_search_reports_lines_verbatim() {
        let temp = tempfile::tempdir().expect("tempdir");
        // 代码块缩进（行首 4 空格）与行尾空白都是原文的一部分。
        fs::write(
            temp.path().join("grep.md"),
            "# grep\n\n```bash\n    grep -rn TODO ./src \n```\n",
        )
        .expect("写笔记");

        let found =
            search_notes_by_content(temp.path(), "grep -rn", Language::Zh).expect("搜索正文");

        assert_eq!(found.items.len(), 1);
        // 逐个字段断言而不是比对渲染后的字符串：行尾空格在字面量里看不见。
        assert_eq!(found.items[0].line_number, 4);
        assert_eq!(found.items[0].line, "    grep -rn TODO ./src ");
        assert_eq!(found.items[0].render(), "grep:4:     grep -rn TODO ./src ");
    }

    /// 行首空白是有效信息（代码块缩进、列表层级），不能被抹掉 —— 这与 `grep`
    /// 的取行方式一致，也让输出可以直接与源文件对拍。
    #[test]
    fn content_search_preserves_indentation() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(
            temp.path().join("awk.md"),
            "# awk\n\n- 常用写法\n  - 按列求和：`awk '{s+=$1} END {print s}'`\n",
        )
        .expect("写笔记");

        let found =
            search_notes_by_content(temp.path(), "按列求和", Language::Zh).expect("搜索正文");
        let rendered: Vec<String> = found.items.iter().map(ContentMatch::render).collect();

        assert_eq!(
            rendered,
            vec!["awk:4:   - 按列求和：`awk '{s+=$1} END {print s}'`"]
        );
    }

    #[test]
    fn content_search_is_case_insensitive() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("aws.md"), "AWS CLI 用法\n").expect("写笔记");

        let found = search_notes_by_content(temp.path(), "aws", Language::Zh).expect("搜索正文");
        assert_eq!(found.items.len(), 1, "搜索应忽略大小写");
    }
}

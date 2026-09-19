use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub fn note_path(notes_dir: &Path, command: &str) -> PathBuf {
    notes_dir.join(format!("{command}.md"))
}

pub fn validate_command_name(command: &str) -> Result<()> {
    anyhow::ensure!(!command.is_empty(), "Command name cannot be empty");
    anyhow::ensure!(
        !command.chars().any(char::is_whitespace),
        "Command name must be a single token without spaces"
    );
    anyhow::ensure!(
        !command.chars().any(|c| c == '/' || c == '\\' || c == ':'),
        "Command name contains unsupported path characters"
    );
    Ok(())
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
    pub fn render(&self) -> String {
        format!("{}:{}: {}", self.command, self.line_number, self.line)
    }
}

pub fn read_note(notes_dir: &Path, command: &str) -> Result<Option<String>> {
    let path = note_path(notes_dir, command);
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read note file: {}", path.display()))?;
    Ok(Some(content))
}

pub fn write_note(notes_dir: &Path, command: &str, content: &str) -> Result<PathBuf> {
    fs::create_dir_all(notes_dir)
        .with_context(|| format!("Failed to create notes directory: {}", notes_dir.display()))?;

    let path = note_path(notes_dir, command);
    fs::write(&path, content)
        .with_context(|| format!("Failed to write note: {}", path.display()))?;
    Ok(path)
}
pub fn ensure_note_file(notes_dir: &Path, command: &str) -> Result<EnsuredNote> {
    fs::create_dir_all(notes_dir)
        .with_context(|| format!("Failed to create notes directory: {}", notes_dir.display()))?;

    let path = note_path(notes_dir, command);
    if path.exists() {
        return Ok(EnsuredNote {
            path,
            created: false,
        });
    }

    fs::write(&path, "").with_context(|| format!("Failed to create note: {}", path.display()))?;
    Ok(EnsuredNote {
        path,
        created: true,
    })
}

/// 列出笔记命令，同时返回无法读取而被跳过的条目。
pub fn scan_commands(notes_dir: &Path) -> Result<Found<String>> {
    if !notes_dir.exists() {
        return Ok(Found::none());
    }

    let mut commands = Vec::new();
    let mut skipped = Vec::new();

    let entries = fs::read_dir(notes_dir)
        .with_context(|| format!("Failed to read notes directory: {}", notes_dir.display()))?;

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

pub fn search_commands_by_name(notes_dir: &Path, keyword: &str) -> Result<Found<String>> {
    let keyword = keyword.to_ascii_lowercase();
    let found = scan_commands(notes_dir)?;
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
/// 刻意不做 Markdown 语法营剥：用户能直接看到原文上下文，行为可预测。
pub fn search_notes_by_content(notes_dir: &Path, keyword: &str) -> Result<Found<ContentMatch>> {
    let needle = keyword.to_ascii_lowercase();
    if needle.is_empty() {
        return Ok(Found::none());
    }

    let found = scan_commands(notes_dir)?;
    let mut skipped = found.skipped;
    let mut items = Vec::new();

    for command in found.items {
        let content = match read_note(notes_dir, &command) {
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
                    line: line.trim().to_string(),
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
        assert!(validate_command_name("ls").is_ok());
        assert!(validate_command_name("git-status").is_ok());
        assert!(validate_command_name("docker run").is_err());
        assert!(validate_command_name("../ls").is_err());
        assert!(validate_command_name("C:ls").is_err());
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

        let first = ensure_note_file(temp.path(), "ls").expect("新建笔记");
        assert!(first.created, "首次调用应报告已创建");
        assert!(first.path.exists());

        fs::write(&first.path, "# ls\n").expect("写入内容");
        let second = ensure_note_file(temp.path(), "ls").expect("已有笔记");
        assert!(!second.created, "已存在时不得报告创建");
        let content = fs::read_to_string(&second.path).expect("读取笔记");
        assert_eq!(content, "# ls\n", "已存在的笔记内容不得被清空");
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

        let found = search_notes_by_content(temp.path(), "递归").expect("搜索正文");
        let rendered: Vec<String> = found.items.iter().map(ContentMatch::render).collect();

        assert_eq!(
            rendered,
            vec!["grep:3: 递归搜索目录", "grep:4: 递归时要小心"]
        );
        assert!(found.skipped.is_empty(), "不该有被跳过的条目");

        let none = search_notes_by_content(temp.path(), "不存在的词").expect("搜索正文");
        assert!(none.items.is_empty());
    }

    #[test]
    fn scan_commands_skips_unreadable_entries_and_reports_them() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("ls.md"), "# ls\n").expect("写笔记");
        // 非 UTF-8 文件名在 Unix 上可构造；否则该分支由 read_dir 失败覆盖。
        #[cfg(unix)]
        {
            use std::ffi::OsStr;
            use std::os::unix::ffi::OsStrExt;
            let bad = OsStr::from_bytes(b"\xff\xfe.md");
            fs::write(temp.path().join(bad), "# bad\n").expect("写非法文件名");
        }

        let found = scan_commands(temp.path()).expect("扫描目录不应整体失败");
        assert_eq!(found.items, vec!["ls".to_string()]);
        #[cfg(unix)]
        assert_eq!(found.skipped.len(), 1, "非法文件名应被记录并跳过");
    }

    #[test]
    fn scan_commands_on_missing_directory_is_empty_not_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let missing = temp.path().join("nope");

        let found = scan_commands(&missing).expect("目录不存在不是错误");
        assert!(found.items.is_empty());
        assert!(found.skipped.is_empty());
    }

    #[test]
    fn content_search_is_case_insensitive() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("aws.md"), "AWS CLI 用法\n").expect("写笔记");

        let found = search_notes_by_content(temp.path(), "aws").expect("搜索正文");
        assert_eq!(found.items.len(), 1, "搜索应忽略大小写");
    }
}

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

pub fn list_commands(notes_dir: &Path) -> Result<Vec<String>> {
    if !notes_dir.exists() {
        return Ok(Vec::new());
    }

    let mut commands = Vec::new();
    for entry in fs::read_dir(notes_dir)
        .with_context(|| format!("Failed to read notes directory: {}", notes_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        if !ext.eq_ignore_ascii_case("md") {
            continue;
        }

        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        commands.push(stem.to_string());
    }

    commands.sort();
    Ok(commands)
}

pub fn search_commands_by_name(notes_dir: &Path, keyword: &str) -> Result<Vec<String>> {
    let keyword = keyword.to_ascii_lowercase();
    let mut results: Vec<String> = list_commands(notes_dir)?
        .into_iter()
        .filter(|command| command.to_ascii_lowercase().contains(&keyword))
        .collect();
    results.sort();
    Ok(results)
}

/// 按正文内容搜索笔记，返回 grep 风格的命中行。
///
/// 刻意不做 Markdown 语法营剥：用户能直接看到原文上下文，行为可预测。
pub fn search_notes_by_content(notes_dir: &Path, keyword: &str) -> Result<Vec<ContentMatch>> {
    let needle = keyword.to_ascii_lowercase();
    if needle.is_empty() {
        return Ok(Vec::new());
    }

    let mut matches = Vec::new();
    for command in list_commands(notes_dir)? {
        let Some(content) = read_note(notes_dir, &command)? else {
            continue;
        };

        for (index, line) in content.lines().enumerate() {
            if line.to_ascii_lowercase().contains(&needle) {
                matches.push(ContentMatch {
                    command: command.clone(),
                    line_number: index + 1,
                    line: line.trim().to_string(),
                });
            }
        }
    }

    Ok(matches)
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

        let matches = search_notes_by_content(temp.path(), "递归").expect("搜索正文");
        let rendered: Vec<String> = matches.iter().map(ContentMatch::render).collect();

        assert_eq!(
            rendered,
            vec!["grep:3: 递归搜索目录", "grep:4: 递归时要小心"]
        );

        let none = search_notes_by_content(temp.path(), "不存在的词").expect("搜索正文");
        assert!(none.is_empty());
    }

    #[test]
    fn content_search_is_case_insensitive() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("aws.md"), "AWS CLI 用法\n").expect("写笔记");

        let matches = search_notes_by_content(temp.path(), "aws").expect("搜索正文");
        assert_eq!(matches.len(), 1, "搜索应忽略大小写");
    }
}

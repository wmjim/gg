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
}

//! 交互提示端口。

use crate::i18n::Language;
use anyhow::Result;
use std::io::{self, IsTerminal, Write};

/// 提示端口。抽象出来是为了让「非交互场景不得触发 AI / 落盘」这条规则可测。
pub trait Prompter {
    /// 是否具备交互能力。
    ///
    /// 只以 stdin 为准：`gg ls | less` 时 stdout 不是终端，但用户仍在交互中。
    fn is_interactive(&self) -> bool;

    fn confirm(&self, question: &str, default_yes: bool) -> Result<bool>;

    /// 从候选里选一个，返回选中的候选项原文。
    fn choose(&self, prompt: &str, options: &[&str], retry_message: &str) -> Result<String>;
}

#[derive(Debug, Clone, Copy)]
pub struct ConsolePrompter {
    lang: Language,
}

impl ConsolePrompter {
    pub fn new(lang: Language) -> Self {
        Self { lang }
    }
}

impl Default for ConsolePrompter {
    fn default() -> Self {
        Self::new(Language::default())
    }
}

impl Prompter for ConsolePrompter {
    fn is_interactive(&self) -> bool {
        io::stdin().is_terminal()
    }

    fn confirm(&self, question: &str, default_yes: bool) -> Result<bool> {
        let mut stderr = io::stderr();
        let default_hint = if default_yes { "Y/n" } else { "y/N" };

        loop {
            write!(stderr, "{question} [{default_hint}]: ")?;
            stderr.flush()?;

            let mut input = String::new();
            let read = io::stdin().read_line(&mut input)?;
            if read == 0 {
                // 输入流被关闭（例如管道提前结束），退回默认值，避免死循环。
                return Ok(default_yes);
            }

            match input.trim().to_ascii_lowercase().as_str() {
                "" => return Ok(default_yes),
                "y" | "yes" => return Ok(true),
                "n" | "no" => return Ok(false),
                _ => writeln!(stderr, "{}", self.lang.yes_no_retry())?,
            }
        }
    }

    fn choose(&self, prompt: &str, options: &[&str], retry_message: &str) -> Result<String> {
        let mut stderr = io::stderr();
        loop {
            write!(stderr, "{prompt}")?;
            stderr.flush()?;

            let mut input = String::new();
            let read = io::stdin().read_line(&mut input)?;
            let answer = input.trim();

            if read == 0 || answer.is_empty() {
                return Ok(options[0].to_string());
            }

            if let Ok(number) = answer.parse::<usize>() {
                if number >= 1 && number <= options.len() {
                    return Ok(options[number - 1].to_string());
                }
            }

            if let Some(hit) = options
                .iter()
                .find(|option| answer.eq_ignore_ascii_case(option))
            {
                return Ok((*hit).to_string());
            }

            writeln!(stderr, "{retry_message}")?;
        }
    }
}

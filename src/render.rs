use std::env;
use std::io::IsTerminal;

use termimad::{Alignment, MadSkin, crossterm::style::Attribute, gray};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalTheme {
    Dark,
    Light,
}

pub fn render_markdown(markdown: &str) {
    if std::io::stdout().is_terminal() {
        let skin = build_skin();
        skin.print_text(markdown);
        return;
    }

    print!("{markdown}");
    if !markdown.ends_with('\n') {
        println!();
    }
}

fn build_skin() -> MadSkin {
    if env::var_os("NO_COLOR").is_some() {
        return MadSkin::no_style();
    }

    let theme = detect_terminal_theme();
    let mut skin = match theme {
        TerminalTheme::Dark => MadSkin::default_dark(),
        TerminalTheme::Light => MadSkin::default_light(),
    };

    // Make headings visually clear: bold, left-aligned, no underline clutter.
    for header in &mut skin.headers {
        header.add_attr(Attribute::Bold);
        header.compound_style.remove_attr(Attribute::Underlined);
        header.align = Alignment::Unspecified;
    }

    match theme {
        TerminalTheme::Dark => {
            skin.headers[0].set_fg(gray(23));
            skin.headers[1].set_fg(gray(21));
            skin.headers[2].set_fg(gray(20));
            skin.inline_code.set_fgbg(gray(23), gray(5));
            skin.code_block.set_fgbg(gray(23), gray(4));
        }
        TerminalTheme::Light => {
            skin.headers[0].set_fg(gray(1));
            skin.headers[1].set_fg(gray(3));
            skin.headers[2].set_fg(gray(5));
            skin.inline_code.set_fgbg(gray(1), gray(20));
            skin.code_block.set_fgbg(gray(2), gray(21));
        }
    }

    skin.code_block.left_margin = 2;
    skin
}

fn detect_terminal_theme() -> TerminalTheme {
    let forced = env::var("GG_TERM_THEME").ok();
    let colorfgbg = env::var("COLORFGBG").ok();
    detect_terminal_theme_from_vars(forced.as_deref(), colorfgbg.as_deref())
}

fn detect_terminal_theme_from_vars(
    forced_theme: Option<&str>,
    colorfgbg: Option<&str>,
) -> TerminalTheme {
    if let Some(theme) = forced_theme.and_then(parse_theme_name) {
        return theme;
    }

    if let Some(theme) = colorfgbg.and_then(parse_colorfgbg_theme) {
        return theme;
    }

    TerminalTheme::Dark
}

fn parse_theme_name(value: &str) -> Option<TerminalTheme> {
    match value.trim().to_ascii_lowercase().as_str() {
        "dark" => Some(TerminalTheme::Dark),
        "light" => Some(TerminalTheme::Light),
        _ => None,
    }
}

fn parse_colorfgbg_theme(value: &str) -> Option<TerminalTheme> {
    let bg = value.split(';').next_back()?.trim().parse::<u8>().ok()?;
    if bg <= 6 || bg == 8 {
        Some(TerminalTheme::Dark)
    } else {
        Some(TerminalTheme::Light)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forced_theme_overrides_detection() {
        let theme = detect_terminal_theme_from_vars(Some("light"), Some("0;0"));
        assert_eq!(theme, TerminalTheme::Light);
    }

    #[test]
    fn colorfgbg_dark_background_detected() {
        let theme = detect_terminal_theme_from_vars(None, Some("15;0"));
        assert_eq!(theme, TerminalTheme::Dark);
    }

    #[test]
    fn colorfgbg_light_background_detected() {
        let theme = detect_terminal_theme_from_vars(None, Some("0;15"));
        assert_eq!(theme, TerminalTheme::Light);
    }

    #[test]
    fn fallback_theme_is_dark() {
        let theme = detect_terminal_theme_from_vars(None, None);
        assert_eq!(theme, TerminalTheme::Dark);
    }
}

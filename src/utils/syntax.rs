//! shell 代码块的语法着色。
//!
//! 刻意不引入 `syntect` 这类完整高亮库：它自带一堆 Sublime 语法转储，会把
//! 二进制从 1.5 MB 抬到 4 MB 上下，而本工具的笔记几乎全是 bash 单行命令。
//! 这里是一个**保守**的极简分词器 —— 拿不准的地方一律按纯文本输出，
//! 宁可少着色，也不出错色。
//!
//! 已知取舍（不做的事）：不识别 heredoc 与多行续行，`$'...'` 与嵌套
//! `${...}` 只按最外层切分。这些在速查笔记里几乎不出现，代价是错误着色
//! 的可能性更低。

/// 支持着色的 fence 语言标签。
pub fn is_supported(lang: &str) -> bool {
    matches!(
        lang.trim().to_ascii_lowercase().as_str(),
        "bash" | "sh" | "shell" | "zsh" | "ksh" | "console" | "shell-session"
    )
}

/// 把一个代码块渲染成带 `<span>` 的高亮 HTML（内容已转义）。
///
/// 不支持的语言返回转义后的纯文本，调用方无需再转义。
pub fn highlight(code: &str, lang: &str) -> String {
    if !is_supported(lang) {
        return escape_html(code);
    }

    let mut html = String::with_capacity(code.len() * 3);
    for (kind, chunk) in merged(code) {
        match kind.class() {
            Some(class) => {
                html.push_str("<span class=\"");
                html.push_str(class);
                html.push_str("\">");
                html.push_str(&escape_html(&chunk));
                html.push_str("</span>");
            }
            None => html.push_str(&escape_html(&chunk)),
        }
    }
    html
}

/// 分词并合并相邻同类 token。
///
/// 引号会切成「左引号 / 内容 / 右引号」三段，中间还可能跨行；不合并就会
/// 生成一串碎片 span。`highlight` 与测试都基于这个函数，避免两边对
/// 「一个 token」的理解分叉。
fn merged(code: &str) -> Vec<(Kind, String)> {
    let mut out: Vec<(Kind, String)> = Vec::new();
    for (kind, text) in tokenize(code) {
        match out.last_mut() {
            Some((last_kind, last_text)) if *last_kind == kind => last_text.push_str(text),
            _ => out.push((kind, text.to_string())),
        }
    }
    out
}

pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Plain,
    Prompt,
    Comment,
    Str,
    Variable,
    Option,
    Command,
    Keyword,
    Number,
    Operator,
}

impl Kind {
    fn class(self) -> Option<&'static str> {
        Some(match self {
            Kind::Plain => return None,
            Kind::Prompt => "tok-prompt",
            Kind::Comment => "tok-comment",
            Kind::Str => "tok-str",
            Kind::Variable => "tok-var",
            Kind::Option => "tok-opt",
            Kind::Command => "tok-cmd",
            Kind::Keyword => "tok-kw",
            Kind::Number => "tok-num",
            Kind::Operator => "tok-op",
        })
    }
}

/// 跨行延续的引号状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Quote {
    #[default]
    None,
    Single,
    Double,
}

fn tokenize(code: &str) -> Vec<(Kind, &str)> {
    let mut tokens = Vec::new();
    let mut quote = Quote::None;
    for line in code.split_inclusive('\n') {
        scan_line(line, &mut quote, &mut tokens);
    }
    tokens
}

fn scan_line<'a>(line: &'a str, quote: &mut Quote, out: &mut Vec<(Kind, &'a str)>) {
    let len = line.len();
    let mut pos = 0usize;
    let mut at_command = true;
    let mut at_line_start = true;

    while pos < len {
        // 续接上一行未闭合的引号
        if *quote != Quote::None {
            let terminator = if *quote == Quote::Single { '\'' } else { '"' };
            let (end, closed) = quote_end(line, pos, terminator, *quote);
            let width = terminator.len_utf8();
            if closed {
                push(out, Kind::Str, &line[pos..end + width]);
                *quote = Quote::None;
                pos = end + width;
            } else {
                push(out, Kind::Str, &line[pos..]);
                pos = len;
            }
            continue;
        }

        let rest = &line[pos..];
        let ch = rest.chars().next().expect("pos 在字符边界内");

        // 行首 `$ ` 提示符： shell-session 风格的笔记均以此书写
        if at_line_start && ch == '$' && rest[1..].starts_with(' ') {
            push(out, Kind::Prompt, &line[pos..pos + 1]);
            pos += 1;
            at_line_start = false;
            at_command = true;
            continue;
        }
        at_line_start = false;

        match ch {
            '#' if at_word_boundary(line, pos) => {
                push(out, Kind::Comment, &line[pos..]);
                pos = len;
            }
            '\'' | '"' => {
                let next = if ch == '\'' {
                    Quote::Single
                } else {
                    Quote::Double
                };
                push(out, Kind::Str, &line[pos..pos + 1]);
                *quote = next;
                pos += 1;
            }
            '$' if rest.starts_with("$(") => {
                push(out, Kind::Operator, &line[pos..pos + 2]);
                pos += 2;
                at_command = true;
            }
            '$' => {
                let width = variable_width(rest);
                if width == 1 {
                    push(out, Kind::Plain, &line[pos..pos + 1]);
                } else {
                    push(out, Kind::Variable, &line[pos..pos + width]);
                }
                pos += width;
            }
            c if c.is_ascii_digit() => {
                let digits = count_while(rest, |c| c.is_ascii_digit());
                // `2>` / `2>>` 这类带 fd 的重定向整体算操作符
                if rest[digits..].starts_with(['<', '>']) {
                    let width = digits + operator_width(&rest[digits..]);
                    push(out, Kind::Operator, &line[pos..pos + width]);
                    pos += width;
                    at_command = true;
                } else {
                    push(out, Kind::Number, &line[pos..pos + digits]);
                    pos += digits;
                }
            }
            c if is_operator_char(c) => {
                let width = operator_width(rest);
                push(out, Kind::Operator, &line[pos..pos + width]);
                at_command = is_command_separator(&rest[..width]);
                pos += width;
            }
            c if is_word_char(c) => {
                let width = count_while(rest, is_word_char);
                let word = &line[pos..pos + width];
                let kind = classify_word(word, at_command);
                push(out, kind, word);
                pos += width;
                // `do` / `then` / `else` 后面接的仍是命令，不能清掉命令位置
                at_command = kind == Kind::Keyword && introduces_command(word);
            }
            _ => {
                // 其它字符（含中文、换行）按纯文本走，原样保留
                let width = count_while(rest, |c| {
                    !is_word_char(c) && !is_operator_char(c) && !matches!(c, '\'' | '"' | '$' | '#')
                })
                .max(1);
                push(out, Kind::Plain, &line[pos..pos + width]);
                pos += width;
            }
        }
    }
}

fn push<'a>(out: &mut Vec<(Kind, &'a str)>, kind: Kind, text: &'a str) {
    if !text.is_empty() {
        out.push((kind, text));
    }
}

/// 这些关键字之后仍应处于命令位置。
fn introduces_command(word: &str) -> bool {
    matches!(word, "do" | "then" | "else")
}

fn classify_word(word: &str, at_command: bool) -> Kind {
    if word.chars().all(|ch| ch == '-') {
        return Kind::Plain;
    }
    if is_keyword(word) {
        Kind::Keyword
    } else if is_option(word) {
        Kind::Option
    } else if at_command {
        Kind::Command
    } else {
        Kind::Plain
    }
}

fn is_keyword(word: &str) -> bool {
    matches!(
        word,
        "if" | "then"
            | "else"
            | "elif"
            | "fi"
            | "for"
            | "while"
            | "until"
            | "do"
            | "done"
            | "case"
            | "esac"
            | "in"
            | "function"
            | "return"
            | "select"
    )
}

/// 形如 `-r`、`-rn`、`--flag` 的选项（`-` 与 `--` 本身不算）。
fn is_option(word: &str) -> bool {
    let rest = word.trim_start_matches('-');
    word.starts_with('-')
        && word.len() > rest.len()
        && rest
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
}

/// 词内字符，全部为 ASCII —— 保证按字节计数即按字符计数。
fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
        || matches!(
            ch,
            '_' | '-' | '.' | '/' | '~' | '*' | '?' | '=' | ':' | '@' | '%' | '+' | ','
        )
}

fn is_operator_char(ch: char) -> bool {
    matches!(ch, '|' | '&' | ';' | '<' | '>' | '(' | ')')
}

/// 该操作符之后是否处于「命令位置」。
fn is_command_separator(op: &str) -> bool {
    op.chars().any(|c| matches!(c, '|' | '&' | ';' | '('))
}

fn at_word_boundary(line: &str, pos: usize) -> bool {
    pos == 0
        || line[..pos]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_whitespace() || is_operator_char(c))
}

fn count_while(rest: &str, predicate: impl Fn(char) -> bool) -> usize {
    rest.char_indices()
        .take_while(|(_, ch)| predicate(*ch))
        .map(|(index, ch)| index + ch.len_utf8())
        .last()
        .unwrap_or(0)
}

fn variable_width(rest: &str) -> usize {
    let mut chars = rest.chars();
    chars.next(); // 跳过 `$`
    match chars.next() {
        Some('{') => rest.find('}').map_or(1, |end| end + 1),
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {
            1 + count_while(&rest[1..], |c| c.is_ascii_alphanumeric() || c == '_')
        }
        Some(c) if c.is_ascii_digit() => 1 + count_while(&rest[1..], |c| c.is_ascii_digit()),
        Some('?' | '@' | '*' | '#' | '$' | '!' | '-') => 2,
        _ => 1,
    }
}

fn operator_width(rest: &str) -> usize {
    count_while(rest, is_operator_char).max(1)
}

/// 找引号结束位置，返回 (结束字节位置, 是否找到)。
fn quote_end(line: &str, from: usize, terminator: char, quote: Quote) -> (usize, bool) {
    let mut escaped = false;
    for (offset, ch) in line[from..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quote == Quote::Double && ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == terminator {
            return (from + offset, true);
        }
    }
    (line.len(), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把一行渲染成 `类名:文本` 序列，便于断言；空白 token 丢弃。
    fn kinds(code: &str) -> Vec<(Option<&'static str>, String)> {
        merged(code)
            .into_iter()
            .filter(|(_, text)| !text.trim().is_empty())
            .map(|(kind, text)| (kind.class(), text.trim().to_string()))
            .collect()
    }

    #[test]
    fn unsupported_language_is_escaped_plain_text() {
        assert_eq!(highlight("a < b", "python"), "a &lt; b");
        assert!(!highlight("a < b", "python").contains("span"));
    }

    #[test]
    fn shell_languages_are_supported() {
        for lang in ["bash", "sh", "zsh", "BASH", " shell ", "console"] {
            assert!(is_supported(lang), "{lang} 应被支持");
        }
        for lang in ["python", "json", "", "go"] {
            assert!(!is_supported(lang), "{lang} 不应被支持");
        }
    }

    #[test]
    fn prompt_comment_command_and_option() {
        let tokens = kinds("$ grep -rn");
        assert_eq!(
            tokens,
            vec![
                (Some("tok-prompt"), "$".into()),
                (Some("tok-cmd"), "grep".into()),
                (Some("tok-opt"), "-rn".into()),
            ]
        );
    }

    #[test]
    fn comment_runs_to_end_of_line() {
        let tokens = kinds("# 递归搜索\n");
        assert_eq!(tokens, vec![(Some("tok-comment"), "# 递归搜索".into())]);
    }

    #[test]
    fn hash_inside_a_word_is_not_a_comment() {
        let tokens = kinds("echo a#b\n");
        assert!(
            !tokens
                .iter()
                .any(|(class, _)| *class == Some("tok-comment")),
            "词中的 # 不是注释: {tokens:?}"
        );
    }

    #[test]
    fn long_options_and_bare_dash() {
        assert_eq!(
            kinds("--dry-run"),
            vec![(Some("tok-opt"), "--dry-run".into())]
        );
        assert_eq!(kinds("-"), vec![(None, "-".into())], "单独的 - 不算选项");
        assert_eq!(kinds("--"), vec![(None, "--".into())]);
    }

    #[test]
    fn strings_and_variables() {
        assert_eq!(
            kinds(r#"grep "TODO" ./src"#),
            vec![
                (Some("tok-cmd"), "grep".into()),
                (Some("tok-str"), r#""TODO""#.into()),
                (None, "./src".into()),
            ]
        );
        assert_eq!(
            kinds("echo $HOME ${PWD} $1"),
            vec![
                (Some("tok-cmd"), "echo".into()),
                (Some("tok-var"), "$HOME".into()),
                (Some("tok-var"), "${PWD}".into()),
                (Some("tok-var"), "$1".into()),
            ]
        );
    }

    #[test]
    fn lone_dollar_stays_plain() {
        assert_eq!(
            kinds("echo $"),
            vec![(Some("tok-cmd"), "echo".into()), (None, "$".into())]
        );
    }

    #[test]
    fn keywords_are_recognised_anywhere() {
        let tokens = kinds("for f in *.md; do echo \"$f\"; done\n");
        let classes: Vec<_> = tokens
            .iter()
            .map(|(class, text)| (*class, text.as_str()))
            .collect();
        assert!(classes.contains(&(Some("tok-kw"), "for")), "{classes:?}");
        assert!(classes.contains(&(Some("tok-kw"), "do")), "{classes:?}");
        assert!(classes.contains(&(Some("tok-kw"), "done")), "{classes:?}");
        assert!(
            classes.contains(&(Some("tok-str"), "\"$f\"")),
            "{classes:?}"
        );
    }

    #[test]
    fn operators_reset_command_position() {
        let tokens = kinds("test -f x && echo $HOME || exit 1\n");
        let classes: Vec<_> = tokens.iter().map(|(c, t)| (*c, t.as_str())).collect();
        assert!(classes.contains(&(Some("tok-op"), "&&")), "{classes:?}");
        assert!(classes.contains(&(Some("tok-op"), "||")), "{classes:?}");
        assert!(classes.contains(&(Some("tok-cmd"), "echo")), "{classes:?}");
        assert!(classes.contains(&(Some("tok-cmd"), "exit")), "{classes:?}");
        assert!(classes.contains(&(Some("tok-num"), "1")), "{classes:?}");
    }

    #[test]
    fn file_descriptor_redirection_is_one_operator() {
        let tokens = kinds("cmd 2>> /tmp/log\n");
        let classes: Vec<_> = tokens.iter().map(|(c, t)| (*c, t.as_str())).collect();
        assert!(classes.contains(&(Some("tok-op"), "2>>")), "{classes:?}");
    }

    #[test]
    fn command_substitution_marks_command_position() {
        let tokens = kinds("$(which git) --version\n");
        let classes: Vec<_> = tokens.iter().map(|(c, t)| (*c, t.as_str())).collect();
        assert!(classes.contains(&(Some("tok-cmd"), "which")), "{classes:?}");
    }

    #[test]
    fn unterminated_quote_continues_to_next_line() {
        let tokens = kinds("echo \"第一行\n第二行\"\n");
        let strings: Vec<&str> = tokens
            .iter()
            .filter(|(class, _)| *class == Some("tok-str"))
            .map(|(_, text)| text.as_str())
            .collect();
        assert_eq!(strings.len(), 1, "跨行字符串应合并为一个 token: {tokens:?}");
        assert!(strings[0].contains("第一行") && strings[0].contains("第二行"));
    }

    #[test]
    fn escaped_quote_does_not_end_the_string() {
        let tokens = kinds("echo \"a\\\"b\"\n");
        let strings: Vec<&str> = tokens
            .iter()
            .filter(|(class, _)| *class == Some("tok-str"))
            .map(|(_, text)| text.as_str())
            .collect();
        assert_eq!(strings, vec![r#""a\"b""#], "{tokens:?}");
    }

    #[test]
    fn highlight_escapes_content_and_never_leaks_markup() {
        let html = highlight("echo \"<script>&\"\n", "bash");
        assert!(!html.contains("<script>"), "内容未转义: {html}");
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&amp;"));
    }

    #[test]
    fn adjacent_same_kind_tokens_merge_into_one_span() {
        let html = highlight(r#"echo "a b""#, "bash");
        assert_eq!(html.matches(r#"class="tok-str""#).count(), 1, "{html}");
    }

    /// 分词只需保证「拼回去等于原文」—— 这是所有着色正确性的前提。
    #[test]
    fn tokenizing_is_lossless() {
        let samples = [
            "$ grep -rn \"TODO\" ./src\n",
            "# 注释\n",
            "for f in *.md; do echo \"$f\"; done\n",
            "cmd 2>> log || exit 1\n",
            "$(which git) --version\n",
            "echo \"未闭合\n第二行\n",
            "if [ -f x ]; then echo ok; fi\n",
            "echo a#b $ \" \n",
        ];
        for sample in samples {
            let joined: String = tokenize(sample).into_iter().map(|(_, text)| text).collect();
            assert_eq!(joined, sample, "分词有损: {sample:?}");
        }
    }
}

//! Markdown 渲染：终端（优先 glow）与浏览器两条路径。

use crate::i18n::Language;
use crate::utils::debug_log;
use crate::utils::platform::{self, OpenTarget};
use crate::utils::process::resolve_program;
use crate::utils::syntax;
use anyhow::{Context, Result, bail};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd, html};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};

/// 浏览器渲染临时文件的最长保留时长。
const STALE_RENDER_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// 输出目标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputTarget {
    Terminal,
    Browser,
}

/// 渲染端口，便于测试时替换为假实现。
pub trait Renderer {
    /// `command` 是笔记对应的命令名，浏览器模式下用于页面标题与书脊栏。
    fn render(&self, markdown: &str, target: OutputTarget, command: &str) -> Result<()>;
}

pub struct MarkdownRenderer {
    lang: Language,
}

impl MarkdownRenderer {
    pub fn new(lang: Language) -> Self {
        Self { lang }
    }
}

impl Renderer for MarkdownRenderer {
    fn render(&self, markdown: &str, target: OutputTarget, command: &str) -> Result<()> {
        match target {
            OutputTarget::Terminal => {
                self.render_to_terminal(markdown);
                Ok(())
            }
            OutputTarget::Browser => self.render_to_browser(markdown, command),
        }
    }
}

impl MarkdownRenderer {
    /// 终端渲染失败时降级为原始 Markdown，保证内容始终可见。
    fn render_to_terminal(&self, markdown: &str) {
        if let Err(err) = self.render_with_glow(markdown) {
            eprintln!("{}", self.lang.glow_failed(&err.to_string()));
            print_raw(markdown, self.lang);
        }
    }

    fn render_to_browser(&self, markdown: &str, command: &str) -> Result<()> {
        let html = convert_md_to_html(markdown, self.lang, command);
        let dir = browser_cache_dir()?;
        prune_stale_render_files(&dir, STALE_RENDER_MAX_AGE);

        let mut file = tempfile::Builder::new()
            .prefix("gg-render-")
            .suffix(".html")
            .tempfile_in(&dir)
            .with_context(|| format!("无法在 {} 创建临时 HTML 文件", dir.display()))?;
        file.write_all(html.as_bytes())
            .context("无法写入临时 HTML 文件")?;
        file.flush().context("无法刷新临时 HTML 文件")?;

        // 浏览器异步读取，临时文件不能在这里删除，交给后续的定期清理回收。
        let (_file, path) = file.keep().context("无法保留临时 HTML 文件")?;
        debug_log!(
            "render::render_to_browser: markdown {} 字节 -> {}",
            markdown.len(),
            path.display()
        );

        platform::open_path(&path, OpenTarget::Browser, self.lang).map_err(|err| {
            anyhow::anyhow!(
                "{}",
                self.lang
                    .browser_open_failed(&path.display().to_string(), &format!("{err:#}"))
            )
        })
    }

    fn render_with_glow(&self, markdown: &str) -> Result<()> {
        let spec = glow_bin();
        let program =
            resolve_program(&spec, self.lang).with_context(|| self.lang.glow_not_found(&spec))?;

        match run_glow_via_stdin(&program, markdown) {
            Ok(()) => Ok(()),
            Err(stdin_err) => {
                debug_log!("render::render_with_glow: stdin 模式失败: {stdin_err}, 回退文件模式");
                run_glow_via_file(&program, markdown).map_err(|file_err| {
                    anyhow::anyhow!("stdin 模式失败：{stdin_err}; 文件模式失败：{file_err}")
                })
            }
        }
    }
}

fn run_glow_via_stdin(program: &crate::utils::process::Program, markdown: &str) -> Result<()> {
    let mut child = program
        .command()
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("无法启动 `{}`", program.describe()))?;

    // glow 可能在读完输入前就退出（例如参数错误），此时 write_all 会得到 EPIPE。
    // 必须无论如何都等到退出并回收，否则要么留下僵尸进程，
    // 要么因为写失败而误判渲染失败、白白回退成原始 Markdown。
    // 同时，ChildStdin 在该分支结束时被 drop，glow 才能看到 EOF。
    let write_result = match child.stdin.take() {
        Some(mut stdin) => stdin.write_all(markdown.as_bytes()),
        None => Ok(()),
    };
    let status = child.wait().context("无法等待 glow 退出")?;

    if let Err(err) = &write_result {
        debug_log!("render::run_glow_via_stdin: 写入 stdin 失败: {err}");
    }

    if status.success() {
        return Ok(());
    }

    let detail = match write_result {
        Ok(()) => String::new(),
        Err(err) => format!("（写入 stdin 失败: {err}）"),
    };
    bail!("glow 退出码 {status}（参数: -）{detail}")
}

fn run_glow_via_file(program: &crate::utils::process::Program, markdown: &str) -> Result<()> {
    let mut file = tempfile::Builder::new()
        .prefix("gg-render-")
        .suffix(".md")
        .tempfile()
        .context("无法创建临时 Markdown 文件")?;
    file.write_all(markdown.as_bytes())
        .context("无法写入临时 Markdown 文件")?;
    file.flush().context("无法刷新临时 Markdown 文件")?;

    // NamedTempFile 在作用域结束时自动删除，无需手工 remove_file。
    let status = program
        .command()
        .arg(file.path())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("无法启动 `{}`", program.describe()))?;

    anyhow::ensure!(
        status.success(),
        "glow 退出码 {status}（文件: {}）",
        file.path().display()
    );
    Ok(())
}

fn glow_bin() -> String {
    std::env::var("GG_GLOW_BIN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "glow".to_string())
}

/// 浏览器渲染产物集中目录；不再把临时文件散落在系统临时目录根部。
fn browser_cache_dir() -> Result<PathBuf> {
    let dir = std::env::temp_dir().join("gg");
    fs::create_dir_all(&dir).with_context(|| format!("无法创建临时目录 {}", dir.display()))?;
    Ok(dir)
}

/// 浏览器是异步读取文件的，渲染结束后不能立即删除，只能事后回收。
/// 早期版本只删临时 `.md`，`.html` 会永久堆积在临时目录。
fn prune_stale_render_files(dir: &Path, max_age: Duration) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let is_render_artifact = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.starts_with("gg-render-") && name.ends_with(".html"))
            .unwrap_or(false);
        if !is_render_artifact {
            continue;
        }

        let age = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok());

        if age.is_some_and(|age| age > max_age) {
            debug_log!("render::prune_stale_render_files: 回收 {}", path.display());
            let _ = fs::remove_file(&path);
        }
    }
}

const TEMPLATE: &str = include_str!("assets/note_template.html");

/// 把 Markdown 与元信息填进模板。
///
/// `{{body}}` 必须最后替换：否则笔记正文里若出现 `{{lang}}` 之类的字面量
/// 会被当成占位符二次展开。
fn convert_md_to_html(markdown: &str, lang: Language, command: &str) -> String {
    TEMPLATE
        .replace("{{lang}}", lang.code())
        .replace("{{command}}", &html_escape(command))
        .replace("{{body}}", &markdown_to_html_body(markdown))
}

/// 命令名会进 `<title>` 与 HTML 文本，需要转义，否则 `gg '<x>'` 之类
/// 的命令名会破坏文档结构。
use crate::utils::syntax::escape_html as html_escape;

fn markdown_to_html_body(markdown: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_HEADING_ATTRIBUTES);
    options.insert(Options::ENABLE_SMART_PUNCTUATION);

    // 原始 HTML 必须在代码块高亮**之前**降级为纯文本：highlight_code_blocks
    // 会把自己产出的、可信的 `Event::Html` 塞进事件流，顺序反了会连它们
    // 一起转义掉。
    let events: Vec<Event> = Parser::new_ext(markdown, options)
        .map(escape_raw_html)
        .collect();
    let mut html_output = String::new();
    html::push_html(&mut html_output, highlight_code_blocks(events).into_iter());
    html_output
}

/// 把笔记里的原始 HTML 降级为纯文本。
///
/// pulldown-cmark 遵循 CommonMark，会原样透传 HTML 块与行内标签。产物是写进
/// 文件、再用浏览器打开的 `.html`，所以笔记里的 `<script>` 或 `onerror=`
/// 会在 `file://` 源上执行 —— 而 AI 生成的笔记属于不可信输入，必须一并挡住。
/// 转成 `Event::Text` 后由 `push_html` 统一转义；代价是原始 HTML 以字面量
/// 显示，这与语法高亮「宁可少着色，也不出错色」是同一种取舍。
fn escape_raw_html(event: Event<'_>) -> Event<'_> {
    match event {
        Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
        other => other,
    }
}

/// 把代码块事件替换成带高亮 span 的 HTML。
///
/// pulldown-cmark 只产出纯文本的 `<pre><code>`，不做着色。这里在**事件流**
/// 上把整个代码块收拢成一个 `Event::Html`（内容由我们自己转义），而不是
/// 先生成 HTML 再回头解析改写 —— 后者要处理二次转义，容易出漏洞。
fn highlight_code_blocks(events: Vec<Event<'_>>) -> Vec<Event<'_>> {
    let mut out: Vec<Event> = Vec::with_capacity(events.len());
    let mut index = 0usize;

    while index < events.len() {
        let Event::Start(Tag::CodeBlock(kind)) = &events[index] else {
            out.push(events[index].clone());
            index += 1;
            continue;
        };

        let lang = match kind {
            CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_string(),
            CodeBlockKind::Indented => String::new(),
        };

        let mut code = String::new();
        let mut cursor = index + 1;
        while let Some(Event::Text(text)) = events.get(cursor) {
            code.push_str(text);
            cursor += 1;
        }

        // 事件流符合预期才改写，否则原样透传，避免把内容弄丢
        if !matches!(events.get(cursor), Some(Event::End(TagEnd::CodeBlock))) {
            out.extend(events[index..cursor].iter().cloned());
            index = cursor;
            continue;
        }

        let code = code.strip_suffix('\n').unwrap_or(&code);
        out.push(Event::Html(render_code_block(code, &lang).into()));
        index = cursor + 1;
    }

    out
}

fn render_code_block(code: &str, lang: &str) -> String {
    // 语言标签会进 class 属性，只保留安全字符
    let class: String = lang
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '+'))
        .take(24)
        .collect();

    let class_attr = if class.is_empty() {
        String::new()
    } else {
        format!(" class=\"language-{class}\"")
    };

    format!(
        "<pre><code{class_attr}>{}</code></pre>\n",
        syntax::highlight(code, lang)
    )
}

fn print_raw(markdown: &str, lang: Language) {
    let text = if markdown.ends_with('\n') {
        markdown.to_string()
    } else {
        format!("{markdown}\n")
    };

    if let Err(err) = crate::utils::output::write_text(io::stdout().lock(), &text) {
        eprintln!("{}", lang.stdout_write_failed(&format!("{err:#}")));
    }
}

/// 供集成测试确认模板占位符全部被替换。
#[cfg(test)]
pub(crate) fn render_html_for_test(markdown: &str) -> String {
    convert_md_to_html(markdown, Language::Zh, "demo")
}

#[cfg(test)]
pub(crate) fn render_html_with_command_for_test(markdown: &str, command: &str) -> String {
    convert_md_to_html(markdown, Language::Zh, command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_escapes_are_left_to_pulldown_cmark() {
        let html = render_html_for_test("# hi\n\n`<b>`\n");
        assert!(html.contains("<h1>hi</h1>"));
        assert!(html.contains("&lt;b&gt;"));
    }

    /// `gg -b` 的产物会被浏览器当成页面执行：笔记里的原始 HTML 必须降级为
    /// 纯文本，否则 `<script>` / `onerror=` 会在 file:// 源上跑起来。
    /// AI 生成的内容属于不可信输入，回归时不要把这条守卫删掉。
    #[test]
    fn raw_html_in_notes_is_escaped_instead_of_executed() {
        let markdown = "\
<script>document.title = 'pwned'</script>\n\
\n\
<img src=x onerror=\"document.title='pwned'\">\n\
\n\
行内 <b>粗体</b> 与 <a href=\"javascript:alert(1)\">链接</a>\n";

        let html = render_html_for_test(markdown);

        for raw in ["<script", "<img", "<b>", "<a href"] {
            assert!(!html.contains(raw), "原始标签 `{raw}` 被透传:\n{html}");
        }
        assert!(
            html.contains("&lt;script&gt;"),
            "原始 HTML 应转义为纯文本:\n{html}"
        );
        assert!(
            html.contains("&lt;b&gt;粗体&lt;/b&gt;"),
            "行内 HTML 应转义为纯文本:\n{html}"
        );
    }

    #[test]
    fn template_placeholders_are_all_replaced() {
        let html = render_html_for_test("body text");
        assert!(!html.contains("{{"), "模板占位符未完全替换: {html}");
        assert!(html.contains("body text"));
        assert!(html.contains("lang=\"zh\""));
    }

    #[test]
    fn tables_and_task_lists_are_enabled() {
        let html = render_html_for_test("| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert!(html.contains("<table>"));
    }

    /// 升级 pulldown-cmark 跨了两个大版本，而我们开了 6 个非默认选项。
    /// 这里逐个盯住每项的实际产出，避免升级后某个选项静默失效。
    #[test]
    fn every_enabled_option_still_takes_effect() {
        let markdown = "\
# 标题 {#custom-id}\n\
\n\
~~删除~~\n\
\n\
- [ ] 待办\n\
- [x] 完成\n\
\n\
引号 \"测试\" 与 -- 破折号\n\
\n\
脚注[^1]\n\
\n\
[^1]: 脚注内容\n\
\n\
| a | b |\n\
|---|---|\n\
| 1 | 2 |\n";

        let html = render_html_for_test(markdown);

        // ENABLE_HEADING_ATTRIBUTES
        assert!(
            html.contains(r#"<h1 id="custom-id">"#),
            "标题属性失效:\n{html}"
        );
        // ENABLE_STRIKETHROUGH
        assert!(html.contains("<del>删除</del>"), "删除线失效:\n{html}");
        // ENABLE_TASKLISTS
        assert!(html.contains(r#"type="checkbox""#), "任务列表失效:\n{html}");
        assert!(html.contains("checked"), "勾选状态丢失:\n{html}");
        // ENABLE_SMART_PUNCTUATION
        assert!(
            html.contains('“') && html.contains('”'),
            "智能引号失效:\n{html}"
        );
        assert!(
            html.contains('–') || html.contains('—'),
            "破折号替换失效:\n{html}"
        );
        // ENABLE_FOOTNOTES
        assert!(
            html.contains(r#"class="footnote-reference""#),
            "脚注引用失效:\n{html}"
        );
        assert!(
            html.contains(r#"class="footnote-definition""#),
            "脚注定义失效:\n{html}"
        );
        // ENABLE_TABLES
        assert!(html.contains("<table>"), "表格失效:\n{html}");
    }

    /// 任务列表样式必须与实际产出对齐。
    ///
    /// 模板里曾按 `.task-list-item` 写样式，但 pulldown-cmark 从不输出该类名
    /// （0.11 与 0.13 的源码均无），那段 CSS 一直是死代码，浏览器里会同时
    /// 出现项目符号与复选框。这个用例把「选择器 ↔ 实际 HTML」的关系钉住，
    /// 以后换版本若产出结构变了会直接失败提醒。
    #[test]
    fn task_list_css_matches_the_markup_we_emit() {
        let html = render_html_for_test("- [ ] 待办\n");

        assert!(
            html.contains("<li><input") && html.contains(r#"type="checkbox""#),
            "任务列表的产出结构变了，需同步核对模板里的选择器:\n{html}"
        );
        assert!(
            !html.contains("task-list-item"),
            "渲染器已开始输出类名，可以改回按类名选了:\n{html}"
        );

        assert!(
            !TEMPLATE.contains("task-list-item"),
            "模板不应再引用永远不会出现的类名"
        );
        assert!(
            TEMPLATE.contains(r#"li:has(input[type="checkbox"])"#),
            "模板缺少与实际产出对齐的任务列表选择器"
        );
    }

    // ---------- 模板不变量守护 ----------
    //
    // 这个模板历史上踩过两类坑，都是「模板与渲染器/主题脱节」且没有任何测试能发现：
    //   1. 死选择器：`.task-list-item`、`.command` 是抄来的类名，pulldown-cmark
    //      从不输出，规则静默失效；
    //   2. 未主题化颜色：表格背景硬编码 `#ffffff`，深色模式下文字对比度掉到
    //      1.54:1，表格整块不可读。
    // 下面三条用例把这两类问题钉死在 CI 上。

    /// 覆盖全部语法，用于让所有产物类名都出现。
    const SAMPLE: &str = "\
# 标题\n\
\n\
定义 `grep` 与 [链接](https://example.com)。\n\
\n\
- 无序项\n\
- [ ] 待办\n\
\n\
1. 有序项\n\
\n\
```bash\n\
# 递归搜索并显示行号\n\
$ grep -rn \"TODO\" ./src\n\
$ for f in *.md; do echo \"$f\"; done\n\
$ test -f /tmp/x && echo $HOME || exit 1\n\
```\n\
\n\
| 参数 | 说明 |\n\
|---|---|\n\
| `-i` | 忽略大小写 |\n\
\n\
> 旁注\n\
\n\
---\n\
\n\
脚注[^1]\n\
\n\
[^1]: 脚注内容\n";

    /// 去掉 CSS 注释 —— 注释里出现的 `.rs`、`#abc` 不该被当成选择器或颜色。
    fn strip_css_comments(css: &str) -> String {
        let mut out = String::with_capacity(css.len());
        let mut rest = css;
        while let Some(start) = rest.find("/*") {
            out.push_str(&rest[..start]);
            match rest[start + 2..].find("*/") {
                Some(end) => rest = &rest[start + 2 + end + 2..],
                None => return out,
            }
        }
        out.push_str(rest);
        out
    }

    fn template_css() -> &'static str {
        TEMPLATE
            .split_once("<style>")
            .and_then(|(_, rest)| rest.split_once("</style>"))
            .map(|(css, _)| css)
            .expect("模板内应有 <style> 块")
    }

    /// 挖掉所有 `:root` 变量块（含 `@media` 内嵌的），只留内容样式。
    fn strip_root_blocks(css: &str) -> String {
        let chars: Vec<char> = css.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        while i < chars.len() {
            if chars[i..].starts_with(&[':', 'r', 'o', 'o', 't']) {
                let Some(open) = chars[i..].iter().position(|c| *c == '{') else {
                    break;
                };
                let mut depth = 0usize;
                let mut j = i + open;
                while j < chars.len() {
                    match chars[j] {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                i = j + 1;
                continue;
            }
            out.push(chars[i]);
            i += 1;
        }
        out
    }

    /// CSS 里出现的类选择器名。`[a-z]` 开头的要求自动排除 `1.5rem` 这类小数。
    fn css_classes(css: &str) -> std::collections::BTreeSet<String> {
        let chars: Vec<char> = css.chars().collect();
        let mut found = std::collections::BTreeSet::new();
        for (i, ch) in chars.iter().enumerate() {
            if *ch != '.' {
                continue;
            }
            let name: String = chars[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_alphanumeric() || **c == '-' || **c == '_')
                .collect();
            if name.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
                found.insert(name);
            }
        }
        found
    }

    fn emitted_classes(html: &str) -> std::collections::BTreeSet<String> {
        html.split("class=\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .flat_map(|value| value.split_whitespace().map(str::to_string))
            .collect()
    }

    #[test]
    fn every_css_class_selector_matches_the_rendered_markup() {
        let html = render_html_for_test(SAMPLE);
        let emitted = emitted_classes(&html);
        let dead: Vec<String> = css_classes(&strip_css_comments(template_css()))
            .into_iter()
            .filter(|name| !emitted.contains(name))
            .collect();

        assert!(
            dead.is_empty(),
            "CSS 里这些类选择器在渲染产物中不存在，规则会静默失效: {dead:?}\n产物类名: {emitted:?}"
        );
    }

    #[test]
    fn colors_are_defined_only_in_theme_blocks() {
        let content = strip_root_blocks(&strip_css_comments(template_css()));
        let chars: Vec<char> = content.chars().collect();

        let mut offenders: Vec<String> = Vec::new();
        for (i, ch) in chars.iter().enumerate() {
            if *ch != '#' {
                continue;
            }
            let hex: String = chars[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_hexdigit())
                .collect();
            if matches!(hex.len(), 3 | 6) {
                offenders.push(format!("#{hex}"));
            }
        }
        for func in ["rgb(", "rgba(", "hsl(", "hsla("] {
            if content.contains(func) {
                offenders.push(func.to_string());
            }
        }

        assert!(
            offenders.is_empty(),
            "这些字面量颜色不在 :root 里，不会随深浅色切换，深色模式下会变成亮斑: {offenders:?}"
        );
    }

    /// 取出 `needle` 之后第一个花括号块的内容。
    fn braced_block_after(text: &str, needle: &str) -> String {
        let start = text.find(needle).expect("能找到选择器") + needle.len();
        let chars: Vec<char> = text[start..].chars().collect();
        let open = chars.iter().position(|c| *c == '{').expect("块开始");
        let mut depth = 0usize;
        let mut out = String::new();
        for ch in &chars[open..] {
            match ch {
                '{' => {
                    depth += 1;
                    if depth == 1 {
                        continue;
                    }
                }
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            out.push(*ch);
        }
        out
    }

    #[derive(Debug, Clone, Copy)]
    struct Rgba(u8, u8, u8, f64);

    impl Rgba {
        fn parse(raw: &str) -> Option<Self> {
            let raw = raw.trim();
            if let Some(hex) = raw.strip_prefix('#') {
                let hex = match hex.len() {
                    3 => hex.chars().flat_map(|c| [c, c]).collect::<String>(),
                    6 => hex.to_string(),
                    _ => return None,
                };
                let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
                return Some(Self(byte(0)?, byte(2)?, byte(4)?, 1.0));
            }

            let inner = raw
                .strip_prefix("rgb(")
                .or_else(|| raw.strip_prefix("rgba("))?
                .strip_suffix(')')?;
            let parts: Vec<f64> = inner
                .split(',')
                .filter_map(|part| part.trim().parse().ok())
                .collect();
            match parts.as_slice() {
                [r, g, b] => Some(Self(*r as u8, *g as u8, *b as u8, 1.0)),
                [r, g, b, a] => Some(Self(*r as u8, *g as u8, *b as u8, *a)),
                _ => None,
            }
        }

        /// 把自身（可能半透明）叠到底色上，得到实际显示色。
        fn over(self, backdrop: Self) -> Self {
            let mix = |top: u8, bottom: u8| {
                (f64::from(top) * self.3 + f64::from(bottom) * (1.0 - self.3)).round() as u8
            };
            Self(
                mix(self.0, backdrop.0),
                mix(self.1, backdrop.1),
                mix(self.2, backdrop.2),
                1.0,
            )
        }

        fn luminance(self) -> f64 {
            let channel = |value: u8| {
                let value = f64::from(value) / 255.0;
                if value <= 0.039_28 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(self.0) + 0.7152 * channel(self.1) + 0.0722 * channel(self.2)
        }

        /// WCAG 对比度：`self` 作为前景，`backdrop` 作为不透明底色。
        fn contrast_over(self, backdrop: Self) -> f64 {
            let front = self.over(backdrop).luminance();
            let back = backdrop.luminance();
            let (hi, lo) = if front > back {
                (front, back)
            } else {
                (back, front)
            };
            (hi + 0.05) / (lo + 0.05)
        }
    }

    fn theme_vars(block: &str) -> std::collections::BTreeMap<String, Rgba> {
        let mut vars = std::collections::BTreeMap::new();
        for line in block.lines() {
            let line = line.trim().trim_end_matches(';');
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            let name = name.trim();
            if !name.starts_with("--") {
                continue;
            }
            if let Some(color) = Rgba::parse(value) {
                vars.insert(name.to_string(), color);
            }
        }
        vars
    }

    /// 主题对比度守护。上一版的做法是表格背景硬编码 `#ffffff`，深色模式下
    /// 表格正文掉到 1.54:1（WCAG AA 需 4.5:1），整块读不了。
    #[test]
    fn theme_contrast_meets_wcag_aa_in_both_modes() {
        let css = template_css();
        let dark = theme_vars(&braced_block_after(css, ":root"));
        let light_vars = theme_vars(&braced_block_after(
            &braced_block_after(css, "@media (prefers-color-scheme: light)"),
            ":root",
        ));

        // (说明, 前景变量, 背景变量, 最低对比度)
        // 半透明背景（--zebra/--hover）会先叠到 --bg 上再算。
        let requirements = [
            ("正文", "--fg", "--bg", 4.5),
            ("次级文字", "--fg-muted", "--bg", 4.5),
            ("链接", "--link", "--bg", 4.5),
            ("行内代码", "--accent", "--surface-2", 4.5),
            ("代码块正文", "--fg", "--surface", 4.5),
            ("表格表头", "--fg-muted", "--surface-2", 4.5),
            ("表格斑马行", "--fg", "--zebra", 4.5),
            ("表格悬停行", "--fg", "--hover", 4.5),
            ("引用旁注", "--fg-muted", "--surface", 4.5),
            ("装饰标记", "--accent-dim", "--bg", 3.0),
            // 代码高亮各 token（底色都是代码块的 --surface）
            ("高亮·提示符/选项", "--accent", "--surface", 4.5),
            ("高亮·命令", "--fg", "--surface", 4.5),
            ("高亮·字符串", "--tok-str", "--surface", 4.5),
            ("高亮·变量", "--tok-var", "--surface", 4.5),
            ("高亮·关键字", "--tok-kw", "--surface", 4.5),
            ("高亮·数字", "--tok-num", "--surface", 4.5),
            ("高亮·注释/操作符", "--fg-muted", "--surface", 4.5),
        ];

        for (mode, vars) in [("深色（默认）", &dark), ("浅色", &light_vars)] {
            let page = *vars.get("--bg").expect("--bg 必须定义");
            for (label, fg, bg, minimum) in requirements {
                let fg = *vars.get(fg).unwrap_or_else(|| panic!("{fg} 未定义"));
                let bg = *vars.get(bg).unwrap_or_else(|| panic!("{bg} 未定义"));
                let ratio = fg.contrast_over(bg.over(page));
                assert!(
                    ratio >= minimum,
                    "{mode} 的「{label}」对比度只有 {ratio:.2}:1，低于要求的 {minimum}:1"
                );
            }
        }
    }

    // ---------- 命令名透传 ----------

    #[test]
    fn title_and_rail_carry_the_command_name() {
        let html = render_html_with_command_for_test("正文", "systemctl");

        assert!(
            html.contains("<title>systemctl · gg</title>"),
            "页面标题应带上命令名，否则多开标签页无法区分"
        );
        assert!(
            html.contains(r#"<div class="rail__cmd">systemctl</div>"#),
            "书脊栏应显示命令名"
        );
    }

    /// 命令名可以合法地包含 `<`、`&` 等字符（只禁止空白与路径分隔符），
    /// 直接拼进 HTML 会破坏文档结构。
    #[test]
    fn command_name_is_escaped() {
        let html = render_html_with_command_for_test("正文", "<b>&\"'");

        assert!(!html.contains("<b>&"), "命令名未转义: {html}");
        assert!(html.contains("&lt;b&gt;&amp;&quot;&#39;"), "转义结果不对");
    }

    #[test]
    fn stale_render_files_are_pruned_but_other_files_are_kept() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stale = dir.path().join("gg-render-old.html");
        let unrelated = dir.path().join("keep-me.txt");
        fs::write(&stale, "old").expect("写旧文件");
        fs::write(&unrelated, "keep").expect("写无关文件");

        // max_age 为 0 时，所有渲染产物都视为过期。
        prune_stale_render_files(dir.path(), Duration::ZERO);

        assert!(!stale.exists(), "过期的渲染产物应被回收");
        assert!(unrelated.exists(), "无关文件不应被误删");
    }

    #[test]
    fn fresh_render_files_are_kept() {
        let dir = tempfile::tempdir().expect("tempdir");
        let fresh = dir.path().join("gg-render-new.html");
        fs::write(&fresh, "new").expect("写新文件");

        prune_stale_render_files(dir.path(), STALE_RENDER_MAX_AGE);
        assert!(fresh.exists(), "未过期的渲染产物必须保留给浏览器读取");
    }
}

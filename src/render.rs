//! Markdown 渲染：终端（优先 glow）与浏览器两条路径。

use crate::i18n::Language;
use crate::utils::debug_log;
use crate::utils::platform::{self, OpenTarget};
use crate::utils::process::resolve_program;
use anyhow::{Context, Result};
use pulldown_cmark::{Options, Parser, html};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
    fn render(&self, markdown: &str, target: OutputTarget) -> Result<()>;
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
    fn render(&self, markdown: &str, target: OutputTarget) -> Result<()> {
        match target {
            OutputTarget::Terminal => {
                self.render_to_terminal(markdown);
                Ok(())
            }
            OutputTarget::Browser => self.render_to_browser(markdown),
        }
    }
}

impl MarkdownRenderer {
    /// 终端渲染失败时降级为原始 Markdown，保证内容始终可见。
    fn render_to_terminal(&self, markdown: &str) {
        if let Err(err) = self.render_with_glow(markdown) {
            eprintln!("{}", self.lang.glow_failed(&err.to_string()));
            print_raw(markdown);
        }
    }

    fn render_to_browser(&self, markdown: &str) -> Result<()> {
        let html = convert_md_to_html(markdown, self.lang);
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
        let program = resolve_program(&spec).with_context(|| self.lang.glow_not_found(&spec))?;

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
    let mut child = Command::new(&program.bin)
        .args(&program.args)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("无法启动 `{}`", program.describe()))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(markdown.as_bytes())
            .context("无法向 glow 写入 Markdown")?;
    }

    let status = child.wait().context("无法等待 glow 退出")?;
    anyhow::ensure!(status.success(), "glow 退出码 {status}（参数: -）");
    Ok(())
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

    // glow 需要按路径读取，写入后立刻关闭句柄，避免 Windows 上的共享冲突。
    let (_file, path) = file.keep().context("无法保留临时 Markdown 文件")?;
    let result = Command::new(&program.bin)
        .args(&program.args)
        .arg(&path)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("无法启动 `{}`", program.describe()));

    let _ = fs::remove_file(&path);

    let status = result?;
    anyhow::ensure!(
        status.success(),
        "glow 退出码 {status}（文件: {}）",
        path.display()
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

fn convert_md_to_html(markdown: &str, lang: Language) -> String {
    TEMPLATE
        .replace("{{lang}}", lang.code())
        .replace("{{title}}", "gg notes")
        .replace("{{body}}", &markdown_to_html_body(markdown))
}

fn markdown_to_html_body(markdown: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_HEADING_ATTRIBUTES);
    options.insert(Options::ENABLE_SMART_PUNCTUATION);

    let parser = Parser::new_ext(markdown, options);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    html_output
}

fn print_raw(markdown: &str) {
    let text = if markdown.ends_with('\n') {
        markdown.to_string()
    } else {
        format!("{markdown}\n")
    };

    if let Err(err) = crate::utils::output::write_text(io::stdout().lock(), &text) {
        eprintln!("无法写入标准输出: {err:#}");
    }
}

/// 供集成测试确认模板占位符全部被替换。
#[cfg(test)]
pub(crate) fn render_html_for_test(markdown: &str) -> String {
    convert_md_to_html(markdown, Language::Zh)
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

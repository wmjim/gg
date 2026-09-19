//! 用户可见文案的唯一来源。
//!
//! 改造前 `app.rs` 里有 10 处 `if lang == "zh" { .. } else { .. }`、33 处中文
//! 字面量，加第三种语言要改遍业务逻辑。这里把所有文案收敛到一处，并通过
//! `catalog!` 保证「中英成对声明」——漏写一门语言会直接编译失败。

use serde::{Deserialize, Serialize};

/// 界面语言。
///
/// 刻意用强类型而非 `String`：配置文件里写 `language = "jp"` 会在反序列化
/// 阶段直接报错，而不是静默降级成英文。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Zh,
    En,
}

impl Language {
    pub const ALL: [Language; 2] = [Language::Zh, Language::En];

    /// 宽松解析，用于命令行 `--lang` 输入。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "zh" | "zh-cn" | "cn" | "chinese" => Some(Language::Zh),
            "en" | "en-us" | "english" => Some(Language::En),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Language::Zh => "zh",
            Language::En => "en",
        }
    }
}

/// 声明成对的中英文案：`名称(参数) => { zh: "..{参数}..", en: "..{参数}.." }`。
///
/// `format!` 的占位符在编译期校验，参数名写错会直接编译失败。
macro_rules! catalog {
    ($($name:ident($($arg:ident: $ty:ty),*) => { zh: $zh:literal, en: $en:literal $(,)? }),* $(,)?) => {
        $(
            pub fn $name(self $(, $arg: $ty)*) -> String {
                match self {
                    Language::Zh => format!($zh $(, $arg = $arg)*),
                    Language::En => format!($en $(, $arg = $arg)*),
                }
            }
        )*
    };
}

impl Language {
    catalog! {
        // ---------- 启动 / 配置 ----------
        saved_editor_config() => {
            zh: "已保存编辑器配置。",
            en: "Editor configuration saved.",
        },
        saved_language_config() => {
            zh: "已保存语言配置。",
            en: "Language configuration saved.",
        },
        invalid_language(raw: &str) => {
            zh: "无效的语言选项: {raw}，请使用 zh 或 en",
            en: "Invalid language option: {raw}. Use zh or en.",
        },
        first_run_header() => {
            zh: "首次使用 gg，请选择显示语言 (Choose display language):",
            en: "Welcome to gg. Choose a display language:",
        },
        first_run_option_zh() => {
            zh: "  1) 中文 (zh)",
            en: "  1) 中文 (zh)",
        },
        first_run_option_en() => {
            zh: "  2) English (en)",
            en: "  2) English (en)",
        },
        first_run_prompt() => {
            zh: "请输入选项数字 [1]: ",
            en: "Enter option number [1]: ",
        },
        first_run_default_note() => {
            zh: "(已默认设置语言为中文，如需更改请使用 --lang 选项)",
            en: "(Defaulted to Chinese; run `gg --lang en` to change)",
        },
        invalid_input() => {
            zh: "无效输入，请重新输入。",
            en: "Invalid input, please try again.",
        },
        yes_no_retry() => {
            zh: "请输入 y 或 n。",
            en: "Please answer y or n.",
        },

        // ---------- 查询 ----------
        note_not_found(command: &str) => {
            zh: "未找到命令 `{command}` 的笔记。",
            en: "No notes found for command `{command}`.",
        },
        did_you_mean(list: &str) => {
            zh: "你可能想查: {list}",
            en: "Did you mean: {list}",
        },
        note_saved(path: &str) => {
            zh: "已保存笔记: {path}",
            en: "Notes saved: {path}",
        },
        notes_skipped(count: &str) => {
            zh: "有 {count} 个条目无法读取，已跳过（用 GG_DEBUG=1 查看详情）。",
            en: "{count} entries could not be read and were skipped (set GG_DEBUG=1 for details).",
        },
        save_skipped() => {
            zh: "已跳过保存。",
            en: "Save skipped.",
        },

        // ---------- AI 回退 ----------
        ai_disabled() => {
            zh: "已配置 ai_provider = \"none\"，AI 回退已关闭。",
            en: "ai_provider = \"none\": AI fallback is disabled.",
        },
        ai_skipped_non_interactive() => {
            zh: "非交互终端，已跳过 AI 回退（设置 ask_before_ai = false 可强制启用）。",
            en: "Non-interactive terminal: skipped AI fallback (set ask_before_ai = false to force it).",
        },
        ai_progress(command: &str) => {
            zh: "正在生成 `{command}` 的笔记",
            en: "Generating notes for `{command}`",
        },
        ai_timeout(seconds: u64) => {
            zh: "AI 在 {seconds} 秒内未返回，已终止进程（可调整 ai_timeout_seconds，设为 0 表示不限制）。",
            en: "AI did not return within {seconds}s and was terminated (tune ai_timeout_seconds; 0 disables the limit).",
        },
        ai_tool_missing(tool: &str) => {
            zh: "未找到 AI 命令行工具 `{tool}`，已跳过 AI 回退（可用 ai_provider 或 ai_command 配置）。",
            en: "AI command `{tool}` not found, skipped AI fallback (configure ai_provider or ai_command).",
        },
        ask_generate_note() => {
            zh: "可尝试生成该笔记。是否继续查询？",
            en: "Generate this note?",
        },
        ai_cancelled() => {
            zh: "已取消 AI 查询。",
            en: "AI query cancelled.",
        },
        ask_save_note() => {
            zh: "是否保存这份 AI 生成笔记到本地？",
            en: "Save the AI-generated note locally?",
        },
        ai_failed(detail: &str) => {
            zh: "AI 调用失败: {detail}",
            en: "AI invocation failed: {detail}",
        },
        ai_empty_output() => {
            zh: "AI 返回了空内容。",
            en: "AI returned empty content.",
        },
        ai_output_not_utf8() => {
            zh: "AI 输出不是合法的 UTF-8。",
            en: "AI output is not valid UTF-8.",
        },

        // ---------- 渲染 ----------
        glow_failed(err: &str) => {
            zh: "glow 渲染失败（{err}），已回退为原始 Markdown 输出。",
            en: "glow rendering failed ({err}); falling back to raw Markdown.",
        },
        glow_not_found(bin: &str) => {
            zh: "未找到 glow 可执行文件 `{bin}`，请先安装 glow 或设置 GG_GLOW_BIN",
            en: "glow binary `{bin}` not found; install glow or set GG_GLOW_BIN",
        },
        browser_open_failed(path: &str, err: &str) => {
            zh: "无法用浏览器打开 {path}: {err}",
            en: "Failed to open {path} in a browser: {err}",
        },

        // ---------- 外部程序 ----------
        opener_target_browser() => {
            zh: "浏览器",
            en: "browser",
        },
        opener_target_editor() => {
            zh: "编辑器",
            en: "editor",
        },
        no_opener_found(target: &str, tried: &str) => {
            zh: "未找到可用的{target}程序。已尝试: {tried}。请安装 xdg-utils 或在配置中指定。",
            en: "No usable {target} found. Tried: {tried}. Install xdg-utils or set an explicit program.",
        },
        opener_launch_failed(target: &str, path: &str) => {
            zh: "启动{target}失败: {path}",
            en: "Failed to launch the {target} for {path}",
        },
        wsl_opener_failed(target: &str, tried: &str) => {
            zh: "WSL 无法打开 Windows {target}。已尝试: {tried}。\n建议: 1) 确认 WSL 互操作已开启; 2) 将 /mnt/c/Windows/System32 加入 PATH; 3) 安装 wslu 并确保 wslview 可用。",
            en: "WSL could not open a Windows {target}. Tried: {tried}.\nSuggestions: 1) enable WSL interop; 2) add /mnt/c/Windows/System32 to PATH; 3) install wslu so wslview is available.",
        },
        configured_editor_missing(editor: &str) => {
            zh: "配置的编辑器 `{editor}` 不存在，尝试系统默认编辑器。",
            en: "Configured editor `{editor}` not found; falling back to the system default.",
        },
        env_editor_missing(editor: &str) => {
            zh: "环境变量指定的编辑器 `{editor}` 不存在，尝试系统默认编辑器。",
            en: "Editor `{editor}` from environment not found; falling back to the system default.",
        },
        editor_launch_failed(editor: &str) => {
            zh: "编辑器 `{editor}` 执行失败，未回退到其他编辑器",
            en: "editor `{editor}` failed; not falling back to another editor",
        },
        no_editor_found() => {
            zh: "未找到可用的编辑器。请设置 GG_EDITOR/EDITOR/VISUAL，或安装 nvim/vim/helix/nano。",
            en: "No usable editor found. Set GG_EDITOR/EDITOR/VISUAL or install nvim/vim/helix/nano.",
        },

        // ---------- CLI 帮助 ----------
        cli_about() => {
            zh: "像 man 一样查询你自己的命令笔记",
            en: "Query your own command notes like man",
        },
        cli_usage_heading() => {
            zh: "用法",
            en: "Usage",
        },
        cli_heading_commands() => {
            zh: "命令",
            en: "Commands",
        },
        cli_heading_arguments() => {
            zh: "参数",
            en: "Arguments",
        },
        cli_heading_options() => {
            zh: "选项",
            en: "Options",
        },
        cli_arg_notes_dir() => {
            zh: "指定笔记目录（优先级高于 GG_NOTES_DIR）",
            en: "Override the notes directory (takes precedence over GG_NOTES_DIR)",
        },
        cli_arg_browser() => {
            zh: "在浏览器中打开 Markdown 而非终端",
            en: "Open the Markdown in a browser instead of the terminal",
        },
        cli_arg_edit() => {
            zh: "在默认编辑器中打开该笔记进行编辑",
            en: "Open the note in your editor",
        },
        cli_arg_set_editor() => {
            zh: "设置默认编辑器并保存到配置",
            en: "Set the default editor and save it to the config",
        },
        cli_arg_yes() => {
            zh: "对所有询问自动回答「是」（会跳过 AI 生成与保存的确认）",
            en: "Answer yes to every prompt (skips the AI generation and save confirmations)",
        },
        cli_arg_lang() => {
            zh: "设置显示语言 (zh/en) 并保存到配置",
            en: "Set the display language (zh/en) and save it to the config",
        },
        cli_arg_help() => {
            zh: "打印帮助信息",
            en: "Print help",
        },
        cli_arg_version() => {
            zh: "打印版本",
            en: "Print version",
        },
        cli_cmd_list() => {
            zh: "列出所有笔记命令",
            en: "List all note commands",
        },
        cli_cmd_search() => {
            zh: "按文件名搜索笔记命令",
            en: "Search note commands by file name",
        },
        cli_cmd_search_keyword() => {
            zh: "搜索关键词",
            en: "Keyword to search for",
        },
        cli_arg_search_content() => {
            zh: "搜索笔记正文而非文件名",
            en: "Search note bodies instead of file names",
        },
        note_created(path: &str) => {
            zh: "已新建空笔记: {path}",
            en: "Created an empty note: {path}",
        },
        cli_cmd_help() => {
            zh: "打印此帮助信息或指定子命令的帮助",
            en: "Print this message or the help of the given subcommand(s)",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_aliases() {
        assert_eq!(Language::parse("zh"), Some(Language::Zh));
        assert_eq!(Language::parse(" ZH-CN "), Some(Language::Zh));
        assert_eq!(Language::parse("en"), Some(Language::En));
        assert_eq!(Language::parse("English"), Some(Language::En));
        assert_eq!(Language::parse("jp"), None);
    }

    #[test]
    fn config_accepts_known_language_and_rejects_unknown() {
        #[derive(Debug, Deserialize)]
        struct Wrapper {
            language: Language,
        }

        let parsed: Wrapper = toml::from_str("language = \"en\"").expect("合法语言");
        assert_eq!(parsed.language, Language::En);

        let err = toml::from_str::<Wrapper>("language = \"jp\"")
            .expect_err("未知语言必须报错，而不是静默降级为英文");
        assert!(format!("{err}").contains("jp"), "实际信息: {err}");
    }

    #[test]
    fn catalog_interpolates_arguments() {
        assert_eq!(
            Language::Zh.note_not_found("lz"),
            "未找到命令 `lz` 的笔记。"
        );
        assert_eq!(
            Language::En.note_not_found("lz"),
            "No notes found for command `lz`."
        );
    }

    #[test]
    fn catalog_covers_every_language() {
        for lang in Language::ALL {
            assert!(!lang.cli_about().is_empty());
            assert!(!lang.save_skipped().is_empty());
        }
    }
}

# gg

> 让 man 文档为你而生长

`gg` 是一个像 `man` 一样的个人命令笔记查询工具。

把 Linux 命令笔记按 Markdown 存在本地目录，用 `gg <命令>` 直接在终端渲染；
查不到时可以交给 AI 生成一份并保存。

## 功能特点

| 功能 | 说明 |
|---|---|
| **查询** | `gg <命令>` 读取并渲染对应笔记 |
| **列表 / 搜索** | `gg list` 列出全部；`gg search <词>` 按文件名搜，`-c` 按正文搜 |
| **终端渲染** | 优先调用 `glow`，不可用时回退原始 Markdown |
| **浏览器渲染** | `-b` 打开排版好的页面：终端手册风格、代码高亮、深浅色自适应 |
| **编辑 / 删除** | `-e` 用编辑器打开；`gg rm <命令>` 删除（会先确认） |
| **AI 生成** | 未命中时生成笔记，支持 claude / codex / gemini 或自定义命令，也可关闭 |
| **相近建议** | 未命中时给出拼写相近的命令名 |
| **中英双语** | `--lang zh\|en`，帮助信息与全部提示均本地化 |

## 安装

```bash
# 安装到系统（推荐）
cargo install --path . --force

# 或只构建发布二进制
cargo build --release      # target/release/gg
```

## 快速开始

```bash
# 1) 建立笔记目录
mkdir -p ~/.config/gg/notes

# 2) 写一份笔记：文件名即命令名
cat > ~/.config/gg/notes/ls.md <<'EOF'
`ls`：显示目录中的文件及其属性信息。

- `-a`, `--all`：显示所有文件，包括隐藏文件
- `-l`：以长格式显示详细信息
EOF

# 3) 查询
gg ls
```

笔记目录里的 `ls.md` 对应 `gg ls`。

## 常用命令

```bash
gg ls                    # 查询 ls 的笔记
gg list                  # 列出全部笔记（终端下多列显示）
gg search grep           # 按文件名搜索
gg search -c 递归        # 按正文搜索，输出 `文件:行号: 内容`
gg -b ls                 # 在浏览器中打开
gg -e ls                 # 用编辑器打开
gg rm ls                 # 删除（默认否定，回车 = 不删）
gg rm ls grep            # 一次删多个
gg show rm               # 显式查询（用于被同名子命令占用的命令名）
```

### 选项

| 选项 | 说明 |
|---|---|
| `--notes-dir <DIR>` | 指定笔记目录 |
| `-b`, `--browser` | 在浏览器中打开 Markdown |
| `-e`, `--edit` | 在编辑器中打开笔记 |
| `-y`, `--yes` | 对所有询问自动回答「是」（含 AI 生成、保存与删除） |
| `--set-editor <EDITOR>` | 设置默认编辑器并写入配置 |
| `--lang <LANG>` | 设置显示语言 `zh` / `en` 并写入配置 |

## 笔记目录

优先级：`--notes-dir` > `GG_NOTES_DIR` > 系统配置目录下 `gg/notes`。

| 平台 | 默认位置 |
|---|---|
| Linux | `~/.config/gg/notes` |
| macOS | `~/Library/Application Support/gg/notes` |
| Windows | `%APPDATA%\gg\notes` |

## 配置

配置文件：Linux/macOS 为 `~/.config/gg/config.toml`，Windows 为 `%APPDATA%\gg\config.toml`。

```toml
language = "zh"           # 显示语言
editor = "hx"             # 默认编辑器，支持带参数，如 "code -w"

ask_before_ai = true      # 调用 AI 前询问
auto_save_ai = true       # AI 生成后落盘
ask_before_save = false   # 落盘前再问一次
ai_provider = "claude"    # claude / codex / gemini / none
# ai_command = "llm -m gpt-4o"   # 自定义 AI 命令行，优先级高于 ai_provider
ai_note_language = "zh-CN"
ai_timeout_seconds = 180  # AI 最长等待秒数，0 表示不限制
```

取值写错会在启动时直接报错并指出问题所在，不会静默降级。

### 环境变量

| 变量 | 说明 |
|---|---|
| `GG_NOTES_DIR` | 笔记目录 |
| `GG_CONFIG_DIR` | 配置根目录（显式覆盖平台默认位置，便于可移植部署） |
| `GG_GLOW_BIN` | `glow` 路径，支持带参数：`"glow -s dark -w 80"` |
| `GG_AI_BIN` | 替换 AI 可执行文件（保留预设参数）；旧名 `GG_CLAUDE_BIN` 仍兼容 |
| `GG_EDITOR` / `VISUAL` / `EDITOR` | 编辑器，优先级依次降低 |
| `GG_DEBUG=1` | 输出调试日志，排查外部命令调用问题 |

## AI 回退

未命中时不直接调用 AI，而是**先检查后端是否可用，再询问**：

```bash
$ gg rsyns
未找到命令 `rsyns`，推荐:
rsync
$ gg rsync
⠹ 正在生成 `rsync` 的笔记 [3s]
已保存笔记: ~/.config/gg/notes/rsync.md
用 `gg rsync` 查看
```

| `ai_provider` | 实际执行 |
|---|---|
| `claude`（默认） | `claude -p --output-format text <提示词>` |
| `codex` | `codex exec <提示词>` |
| `gemini` | `gemini -p <提示词>` |
| `none` | 关闭 AI 回退 |

需要别的形态就用 `ai_command`，提示词默认追加为最后一个参数：

```toml
ai_command = "llm -m gpt-4o"
ai_command = "my-tool --flag {prompt}"    # 或用 {prompt} 指定位置
```

**非交互终端不会静默调用 AI**：`gg foo | less` 或 `gg foo > out.md` 这类场景
没有用户可确认，会跳过 AI 并返回退出码 3。脚本里需要自动化就显式加 `-y`，
或把 `ask_before_ai` 设为 `false`。

## 退出码

| 退出码 | 含义 |
|---|---|
| 0 | 成功 |
| 1 | 运行时错误（配置非法、文件不可写等，stderr 有完整原因） |
| 2 | 命令行用法错误 |
| 3 | 没有产生任何结果：查询未命中，或操作因缺少确认而未执行 |

## 注意事项

- 仅支持「单词命令名」，不能含空格，也不能含 `/`、`\`、`:`
- 仅识别 `.md` 笔记文件
- `list`、`search`、`rm`、`show`、`help` 是子命令名，不能作为普通查询命令名直接使用；
  被占用的名字（典型是 `rm`）可以用 `gg show <命令>` 查询
- `gg rm` 不可逆，非交互终端需显式 `-y`；若笔记目录在 git 仓库内会提示找回方式

## License

MIT，见 [LICENSE](LICENSE)。

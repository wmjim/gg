
# gg

> 让 man 文档为你而生长

`gg` 是一个类似 `man` 的个人命令笔记查询工具：

- 你把 Linux 命令笔记按 Markdown 文件保存到本地目录
- 用 `gg <cmd>` 直接查询并在终端渲染
- 查不到时可回退调用 Claude 生成笔记并保存

## 功能

- 查询：`gg <cmd>`
- 列表：`gg list`
- 搜索：`gg search <keyword>`（仅按文件名匹配）
- 路径优先级：`--notes-dir` > `GG_NOTES_DIR` > 系统配置目录下 `gg/notes`
- Markdown 渲染：TTY 下彩色渲染；非 TTY 下输出原始 Markdown
- 渲染主题：自动检测终端深浅主题，可用 `GG_TERM_THEME=dark|light` 强制指定
- AI 回退：未命中时检测 `claude`，可询问后生成并保存

## 安装与构建

### 方式 1：安装到系统（推荐）

在项目目录执行：

```bash
cargo install --path . --force
```

安装后可直接在任意目录使用：

```bash
gg --version
gg list
```

说明：`cargo install` 会把可执行文件放到 Cargo 的 bin 目录（通常是 `~/.cargo/bin` 或 `%USERPROFILE%\\.cargo\\bin`）。

### 方式 2：仅构建发布二进制

```bash
cargo build --release
```

可执行文件：

- Windows: `target/release/gg.exe`
- Linux/macOS: `target/release/gg`

你可以手动把它复制到 PATH 中的目录。

### PATH 检查

如果执行 `gg` 提示找不到命令，请确认 Cargo bin 在 PATH 中：

- Linux/macOS: `~/.cargo/bin`
- Windows: `%USERPROFILE%\\.cargo\\bin`

## 开发运行

```bash
cargo run -- ls
cargo run -- list
cargo run -- search gre
```

## 笔记目录结构

v1 约定每个命令一个文件：

```text
notes/
  ls.md
  grep.md
  systemctl.md
```

`gg ls` 会读取 `ls.md`。

## 使用说明

### 1) 查询命令笔记

```bash
gg ls
gg grep
```

### 2) 列出全部笔记

```bash
gg list
```

### 3) 按命令名搜索

```bash
gg search ls
gg search gre
```

### 4) 临时指定笔记目录

```bash
gg --notes-dir ~/my-notes ls
```

### 5) 环境变量指定笔记目录

```bash
# Linux/macOS
export GG_NOTES_DIR=~/my-notes

# PowerShell
$env:GG_NOTES_DIR = "D:\\notes"
```

## 配置文件

配置文件位置：

- Linux/macOS: `~/.config/gg/config.toml`
- Windows: `%APPDATA%\\gg\\config.toml`

示例：

```toml
ask_before_ai = true
auto_save_ai = true
ask_before_save = false
ai_note_language = "zh-CN"
ai_provider = "claude"
```

字段说明：

- `ask_before_ai`: 交互终端下，未命中时是否先询问再调用 AI
- `auto_save_ai`: AI 生成后默认是否保存
- `ask_before_save`: 交互终端下，保存前是否再次询问
- `ai_note_language`: AI 生成笔记的目标语言
- `ai_provider`: 当前仅支持 `claude`

## 渲染主题

默认会自动按终端环境推断深色/浅色风格（代码块与行内代码会适配不同主题）。

如果你想手动强制：

```bash
# Linux/macOS
export GG_TERM_THEME=dark
# 或
export GG_TERM_THEME=light

# PowerShell
$env:GG_TERM_THEME = "dark"
```

如果设置了 `NO_COLOR`，则禁用样式输出。

## Claude 回退说明

当 `gg <cmd>` 未找到本地笔记时：

1. 输出未命中提示和相近命令建议
2. 检测 `claude` 是否可用
3. 在交互终端中（且 `ask_before_ai=true`）询问是否调用 AI
4. 生成 Markdown 后输出到终端
5. 根据保存策略保存到 `<notes_dir>/<cmd>.md`

可通过环境变量覆盖 Claude 可执行文件：

```bash
# 示例：自定义 Claude 命令路径
export GG_CLAUDE_BIN=/path/to/claude
```

## 非交互模式行为

当在管道/重定向/脚本环境中运行时，不会弹交互问题：

- AI 查询按默认策略自动执行（若可用）
- 保存行为按 `auto_save_ai` 执行

## 注意事项

- v1 仅支持“单词命令名”（不能含空格）
- 命令名不能包含 `/`、`\\`、`:`
- 仅支持 `.md` 笔记文件
- `list`、`search`、`help` 是子命令名，不能作为普通查询命令名直接使用

## 运行测试

```bash
cargo test
```

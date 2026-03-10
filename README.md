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
- Markdown 渲染：优先调用 `glow`，失败时回退原始 Markdown 输出
- AI 回退：未命中时检测 `claude`，可询问后生成并保存

## 安装与构建

### 方式 1：安装到系统（推荐）

```bash
cargo install --path . --force
```

安装后可在任意目录使用：

```bash
gg --version
gg list
```

### 方式 2：仅构建发布二进制

```bash
cargo build --release
```

- Windows: `target/release/gg.exe`
- Linux/macOS: `target/release/gg`

## glow 渲染

`gg` 优先调用 `glow` 在终端渲染 Markdown。

- 默认执行：`glow -`
- 未安装 `glow` 或调用失败时，自动回退为原始 Markdown 输出
- 可通过 `GG_GLOW_BIN` 指定 `glow` 路径

示例：

```bash
# Linux/macOS
export GG_GLOW_BIN=/usr/local/bin/glow

# PowerShell
$env:GG_GLOW_BIN = "C:\\Tools\\glow.exe"
```

## 默认笔记目录

| 平台 | 默认笔记目录 |
|------|-------------|
| Linux | `~/.config/gg/notes` |
| macOS | `~/Library/Application Support/gg/notes` |
| Windows | `%APPDATA%\gg\notes` |

路径优先级：`--notes-dir` > `GG_NOTES_DIR` > 默认目录。

```bash
# 命令行参数
gg --notes-dir ~/my-notes ls

# 环境变量
export GG_NOTES_DIR=~/my-notes
```

## 笔记目录结构

```text
notes/
  ls.md
  grep.md
  systemctl.md
```

`gg ls` 会读取 `ls.md`。

## 使用说明

```bash
gg ls
gg list
gg search gre
```

## 配置文件

- Linux/macOS: `~/.config/gg/config.toml`
- Windows: `%APPDATA%\gg\config.toml`

```toml
ask_before_ai = true
auto_save_ai = true
ask_before_save = false
ai_note_language = "zh-CN"
ai_provider = "claude"
```

## Claude 回退说明

当 `gg <cmd>` 未找到本地笔记时：

1. 输出未命中提示和相近命令建议
2. 检测 `claude` 是否可用
3. 在交互终端中（且 `ask_before_ai=true`）询问是否调用 AI
4. 生成 Markdown 后输出到终端
5. 根据保存策略保存到 `<notes_dir>/<cmd>.md`

可通过 `GG_CLAUDE_BIN` 指定 Claude 可执行文件路径。

## 注意事项

- v1 仅支持“单词命令名”（不能含空格）
- 命令名不能包含 `/`、`\`、`:`
- 仅支持 `.md` 笔记文件
- `list`、`search`、`help` 是子命令名，不能作为普通查询命令名直接使用

## 测试

```bash
cargo test
```
## 浏览器渲染

`gg` 默认在终端渲染 Markdown（优先调用 `glow`）。如果你想在浏览器中打开并渲染，可以加上 `--browser`：

```bash
gg --browser ls
```

该模式会把 Markdown 转成 HTML 并用系统默认浏览器打开。
## 编辑笔记

如果想直接用默认编辑器打开并修改笔记，可以使用 `--edit`：

```bash
gg --edit ls
```

编辑器优先级：`GG_EDITOR` > `VISUAL` > `EDITOR`。未设置时会尝试系统默认编辑方式。


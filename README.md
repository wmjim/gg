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
- 浏览器渲染：`--browser` 在浏览器中打开 Markdown
- 编辑笔记：`--edit` 用默认编辑器打开笔记
- 界面语言：`zh` / `en`，帮助信息与所有提示均本地化

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

发布构建已启用 `lto` / `codegen-units = 1` / `strip` / `panic = "abort"`。

## glow 渲染

`gg` 优先调用 `glow` 在终端渲染 Markdown。

- 默认执行：`glow -`
- 未安装 `glow` 或调用失败时，自动回退为原始 Markdown 输出
- 可通过 `GG_GLOW_BIN` 指定 `glow` 路径，**支持携带参数**

示例：

```bash
# Linux/macOS
export GG_GLOW_BIN=/usr/local/bin/glow
export GG_GLOW_BIN="glow -s dark -w 80"

# PowerShell
$env:GG_GLOW_BIN = "C:\Tools\glow.exe"
```

`GG_GLOW_BIN` 的解析规则（编辑器配置同理）：

1. 先把整个字符串当作路径解析 —— 保护 `C:\Program Files\glow.exe` 这类含空格的绝对路径
2. 含路径分隔符时按「最长前缀」逐段合并 —— 支持 `/opt/my tools/ed -w` 这种未加引号的写法
3. 最后按 shell 词法切分 —— 支持 `glow -s dark`、`"'/opt/my editor' -p"`

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

## 退出码

| 退出码 | 含义 |
|--------|------|
| 0 | 成功（命中笔记 / `--edit` 打开 / AI 成功生成） |
| 1 | 运行时错误（配置非法、文件不可写等，stderr 有完整原因） |
| 2 | 命令行用法错误（clap 输出） |
| 3 | 未命中笔记且没有可用的替代内容 |

退出码 3 让脚本可以直接判断：

```bash
gg foo > /dev/null 2>&1 || echo "还没写过 foo 的笔记"
```

## 配置文件

- Linux/macOS: `~/.config/gg/config.toml`
- Windows: `%APPDATA%\gg\config.toml`

```toml
ask_before_ai = true      # 调用 AI 前是否询问
auto_save_ai = true       # AI 生成后是否落盘
ask_before_save = false   # 落盘前是否再询问一次
ai_note_language = "zh-CN"
ai_provider = "claude"    # 当前仅支持 claude
editor = "hx"             # 可选：默认编辑器，支持带参数，如 "code -w"
language = "zh"           # 可选：显示语言 (zh/en)
```

配置项类型是强校验的：`language = "jp"`、`ai_provider = "openai"` 会在启动时直接报错并指出问题所在文件，
而不是静默降级成英文或跳过 AI。

## Claude 回退说明

当 `gg <cmd>` 未找到本地笔记时：

1. 输出未命中提示和相近命令建议
2. 检测 `claude` 是否可用
3. 在交互终端中（且 `ask_before_ai=true`）询问是否调用 AI
4. 生成 Markdown 后输出到终端
5. 根据保存策略保存到 `<notes_dir>/<cmd>.md`

可通过 `GG_CLAUDE_BIN` 指定 Claude 可执行文件路径。

### 非交互终端的保护策略

`ask_before_ai = true`（默认）意味着「必须先经我确认」，而确认依赖交互终端。
因此在 `stdout` 被管道或重定向接管时，**不会**静默调用 AI，也不会写入笔记目录：

```bash
gg foo | less        # 提示「非交互终端，已跳过 AI 回退」，退出码 3
gg foo > out.md      # 同上，不会意外改写 ~/.config/gg/notes/
```

如果你确认要在脚本里自动生成，把确认关掉即可显式授权：

```toml
ask_before_ai = false
auto_save_ai = true
```

## 浏览器渲染

`gg` 默认在终端渲染 Markdown（优先调用 `glow`）。如果你想在浏览器中打开并渲染，可以加上 `--browser`：

```bash
gg --browser ls
```

该模式会把 Markdown 转成 HTML 并用系统默认浏览器打开。HTML 模板位于
`src/assets/note_template.html`，可直接编辑而无需改 Rust 代码。

渲染产物写入 `<系统临时目录>/gg/`，每次渲染前会清理其中超过 24 小时的旧文件
（浏览器是异步读取的，渲染结束后不能立刻删除）。

**WSL 支持**：在 WSL 环境下会自动转换路径，并依次尝试 `wslview`、`powershell.exe`、
`pwsh.exe`、`cmd.exe`、`explorer.exe` 以及它们在 `/mnt/c/...` 下的绝对路径。

## 编辑笔记

如果想直接用默认编辑器打开并修改笔记，可以使用 `--edit`：

```bash
gg --edit ls
```

编辑器优先级：`config.editor` > `GG_EDITOR` > `VISUAL` > `EDITOR` > 终端编辑器
（`nvim` > `vim` > `vi` > `hx` > `helix` > `nano`）> 系统默认程序。

可用 `--set-editor` 设置默认编辑器并保存到配置：

```bash
gg --set-editor vim
gg --set-editor "code -w"
```

## 设置默认语言

`--lang` 可设置显示语言（zh/en）并保存到配置：

```bash
gg --lang en
```

`gg --lang en --help` 会立即以英文输出帮助，无需先落盘配置。

## 排查问题

设置 `GG_DEBUG=1` 可输出带时间戳与调用点上下文的调试日志，
用于定位「候选程序一个都没成功」这类多级回退问题：

```bash
GG_DEBUG=1 gg --browser ls
```

```text
[DEBUG] 2026-09-19T04:34:09Z platform::open_native: `gnome-open` 不在 PATH 中, 跳过
[DEBUG] 2026-09-19T04:34:09Z platform::open_native: 已通过 `xdg-open` 打开 /tmp/gg/gg-render-xxx.html (target="浏览器")
```

## 项目结构

```text
src/
  main.rs           进程入口：探测语言 → 解析参数 → 分派
  lib.rs            模块导出
  cli.rs            clap 定义 + 本地化帮助
  app.rs            应用编排层 / QueryService（依赖注入）
  config.rs         配置模型、路径解析（强类型枚举）
  i18n.rs           中英文案唯一来源
  notes.rs          笔记仓储层（读写、列举、模糊建议）
  render.rs         Markdown 渲染（终端 glow / 浏览器）
  editor.rs         编辑器启动与回退链
  ai.rs             Claude 笔记生成
  prompt.rs         交互提示端口
  utils/
    process.rs      可执行命令字符串解析
    platform.rs     跨平台「打开路径」（含 WSL 互操作）
  assets/
    note_template.html   浏览器渲染模板
tests/
  cli_integration.rs  端到端 CLI 测试（跨平台）
```

分层约定：`app.rs` 只做编排，通过 `render` / `editor` / `ai` / `prompt` 四个 trait
注入依赖；业务规则放在 `QueryService`，因此可以脱离真实进程与终端做单测。

## 测试

```bash
cargo test                 # 单元测试 + 集成测试
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
```

CI（`.github/workflows/ci.yml`）在 ubuntu / macos / windows 三个平台跑完整测试矩阵，
并单独一个 job 跑 `fmt` + `clippy -D warnings`。

## 注意事项

- v1 仅支持「单词命令名」（不能含空格）
- 命令名不能包含 `/`、`\`、`:`
- 仅支持 `.md` 笔记文件
- `list`、`search`、`help` 是子命令名，不能作为普通查询命令名直接使用

## License

MIT，见 [LICENSE](LICENSE)。

# gg

> 让 man 文档为你而生长

`gg` 是一个类似 `man` 的个人命令笔记查询工具：

- 你把 Linux 命令笔记按 Markdown 文件保存到本地目录
- 用 `gg <cmd>` 直接查询并在终端渲染
- 查不到时可回退调用 Claude 生成笔记并保存

## 功能

- 查询：`gg <cmd>`
- 列表：`gg list`（终端下按宽度多列显示）
- 搜索：`gg search <keyword>`（默认按文件名）
- 全文搜索：`gg search -c <keyword>`（按笔记正文，grep 风格输出）
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
gg search -c 递归        # 按正文搜索，输出 `文件:行号: 内容`
```

```text
$ gg search -c 递归
grep:12: 递归搜索目录下所有文件
grep:13: 递归时要小心软链接
```

### 列表的列布局

`gg list` / `gg search` 在 **stdout 是终端**时按终端宽度排成多列，行为对齐 `ls`：
列内纵向填充、列间对齐、无行尾空格。管道或重定向时仍为每行一条，
保证 `gg list | grep x` 这类脚本不受影响。

```text
$ gg list
alias  clear    echo    grep          jobs    more    ripgrep  tee      xargs
apt    command  export  head          kill    mv      rm       timeout  zip
aur    cp       fd      helix         less    nohup   sed      tmux     zoxide
```

列宽取环境变量 `COLUMNS`，未设置时按 80 列。

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
ai_provider = "claude"    # claude / codex / gemini / none
ai_command = ""           # 可选：自定义 AI 命令行，优先级高于 ai_provider
ai_timeout_seconds = 180  # AI 生成最长等待秒数，0 表示不限制
editor = "hx"             # 可选：默认编辑器，支持带参数，如 "code -w"
language = "zh"           # 可选：显示语言 (zh/en)
```

配置项类型是强校验的：`language = "jp"`、`ai_provider = "openai"` 会在启动时直接报错并指出问题所在文件，
而不是静默降级成英文或跳过 AI。

## AI 回退说明

当 `gg <cmd>` 未找到本地笔记时：

1. 在同一行输出未命中提示与相近命令建议
2. 检查 AI 后端是否可用
3. 在交互终端中（且 `ask_before_ai=true`）询问是否调用 AI
4. 落到 `<notes_dir>/<cmd>.md`，并告知保存位置与查看命令
5. 若未落盘，则把生成结果直接输出到终端（否则内容就丢了）

```text
$ gg rsyns
未找到命令 `rsyns`，推荐:
rsync
$ gg rsync
正在生成 `rsync` 的笔记
已保存笔记: ~/.config/gg/notes/rsync.md
用 `gg rsync` 查看
```

**不再把生成的整篇笔记刷到终端** —— 笔记已在本地，直接用 `gg <cmd>` 查看或
`gg --edit <cmd>` 修改即可。未落盘（`auto_save_ai=false` 且拒绝保存）时才
当场输出，那种情况下没有「保存位置」可给，不输出就等于内容丢失。

### 后端选择

AI 后端不写死，可用 `ai_provider` 选预设，或用 `ai_command` 完全自定义：

| `ai_provider` | 实际执行 |
|---|---|
| `claude`（默认） | `claude -p --output-format text <提示词>` |
| `codex` | `codex exec <提示词>` |
| `gemini` | `gemini -p <提示词>` |
| `none` | **关闭 AI 回退**，未命中时只给相近命令建议 |

提示词一律追加为最后一个参数 —— `claude -p` / `codex exec` / `gemini -p` /
`llm` / `aichat` / `ollama run <model>` 都符合这个约定。

需要别的形态时用 `ai_command`（优先级高于预设，包括 `none`）：

```toml
ai_command = "llm -m gpt-4o"              # 提示词追加到末尾
ai_command = "ollama run qwen2.5"         # 同上
ai_command = "my-tool --flag {prompt}"    # 用 {prompt} 指定插入位置
```

只想换可执行文件、保留预设参数时用 `GG_AI_BIN`：

```bash
GG_AI_BIN=/opt/claude/bin/claude gg foo    # 仍带 -p --output-format text
```

`GG_CLAUDE_BIN` 作为旧名仍然兼容。

### 提示词与输出风格

提示词的目标是**贴近手写笔记的密度**，而不是生成教程。以本仓库作者手写的
`pwd.md` 为基准（7 行）约束模型：只列 2~4 个最高频选项、每段代码块只放一条
命令、全文 10~25 行，并明确禁止小节标题、表格、emoji、"总之/综上"、
命令历史来源介绍与「强大的/常用的」这类形容。

模型如果仍然在首行加了 `# xxx 命令速查`，`gg` 会在保存前剥掉该标题、
清理行尾空格并折叠多余空行 —— 所以提示词和代码各兜一层，结果稳定。

### 等待反馈

生成期间 `stderr` 会显示转圈动画与已等待秒数：

```text
⠹ 正在生成 `rsync` 的笔记 [7s]
```

只在 `stderr` 是终端且 `TERM != dumb` 时启用；管道或重定向场景退化为一行
静态提示，不会往日志里塞控制字符。超过 `ai_timeout_seconds` 仍未返回时会
终止子进程并报错（默认 180 秒，设 0 表示不限制）。

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

### 设计取向

页面按「终端手册」来做，而不是博客排版 —— 笔记的内容形态是短定义 + 参数列表
+ bash 块，读者的动作是扫一眼就走：

- **等宽作为正文字体**（非只用于代码块），参数列因此天然对齐，接近 man page 的底子
- 左侧**书脊栏**竖排命令名，宽屏吸顶、窄屏折叠成顶部横条
- 代码块渲染为**终端窗口**（圆点标题栏），`h2` 自动编号 `01 / 02`，均为纯 CSS
- **代码块语法着色在服务端完成**（`utils/syntax.rs`），页面里没有一行 JS，
  也不引入高亮库：`syntect` 那类完整方案会把二进制从 1.5 MB 抬到 4 MB 上下，
  而这里几乎全是 bash 单行命令
- 单一琥珀强调色；深浅色双主题，深色为默认
- **不使用任何 webfont**：本地 CLI 打开临时文件，离线可用、零延迟、不外泄请求。
  字体栈全部命中系统自带字体（Maple Mono 自带 CJK 字面，中英混排基线一致）
- 尊重 `prefers-reduced-motion`；带 `@media print` 样式，可直接打印或存 PDF

### 代码高亮的范围与取舍

`utils/syntax.rs` 是一个**保守**的 shell 分词器，只给 fence 标注为
`bash` / `sh` / `zsh` / `ksh` / `console` / `shell-session` 的代码块着色，
其余语言（如 ```python）原样转义输出。

着色的 5 组语义：提示符与选项用琥珀（最醒目，速查时就是找这些）、
命令用正文色加粗、字符串绿、变量青、关键字紫，注释与操作符压到次级灰。

**刻意不做的事**：不识别 heredoc 与多行续行，`$'...'` 与嵌套 `${...}`
只按最外层切分。原则是拿不准就按纯文本输出 —— 宁可少着色，也不出错色。
有一条「分词无损」用例保证切分后拼回去等于原文，这是所有着色正确性的前提。

### 模板不变量

模板有四条不变量由测试强制（`cargo test` 覆盖）：

1. **无死选择器** —— CSS 里每个类选择器都必须在渲染产物中真实出现。
   pulldown-cmark 不做语法高亮、也不给任务列表项加类名，随手写的
   `.comment` / `.task-list-item` 会静默失效。
2. **颜色只在 `:root` 主题块内定义** —— 内容规则里出现字面量颜色就不会随
   深浅色切换，深色模式下变成亮斑。历史上表格背景硬编码 `#ffffff`，
   导致深色下表格正文对比度只有 1.54:1。
3. **对比度达标** —— 两种模式下逐一校验正文/链接/表格/代码高亮各 token 的
   WCAG AA 对比度（正文 4.5:1，装饰元素 3:1），含半透明叠加层的合成。
4. **高亮 token 类名齐全** —— 第 1 条的副产物：测试样本里的 bash 代码块
   覆盖了全部 token 类型，因此分词器少产出任何一种都会被立刻发现。

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

这是一条**严格优先级链**：上一级只是「没装」时会继续往下试，而不是直接跳到
终端编辑器。但如果上一级**装上了却以非 0 退出**（例如 vim 里 `:cq`），
`gg` 会直接报错并保留退出码，不会再开第二个编辑器：

```text
$ gg --edit ls
Error: 编辑器 `failing` 执行失败，未回退到其他编辑器: 编辑器退出码 4
```

`--edit` 一个尚未存在的命令时会新建空笔记并明确提示，不会默默多出文件：

```text
$ gg --edit docker
已新建空笔记: ~/.config/gg/notes/docker.md
```

可用 `--set-editor` 设置默认编辑器并保存到配置：

```bash
gg --set-editor vim
gg --set-editor "code -w"
```

## 跳过交互确认

`-y/--yes` 对所有询问自动回答「是」，相当于在**本次运行内**授权非交互生成
与落盘，无需改配置：

```bash
gg -y foo          # 未命中时直接调用 claude 并按 auto_save_ai 落盘
```

它的边界：

- 只替代询问，**不会**绕过「claude 是否可用」的判断
- 未启用询问（`ask_before_save = false`）时，不会把 `auto_save_ai = false` 变成保存
- 会直接产生费用与磁盘写入，在脚本里使用前请确认清楚

## 设置默认语言

`--lang` 可设置显示语言（zh/en）并保存到配置：

```bash
gg --lang en
```

`gg --lang en --help` 会立即以英文输出帮助，无需先落盘配置。
根命令与所有子命令的帮助（`用法/命令/参数/选项` 标题、参数说明、帮助正文）
均由 clap 的单一来源生成并本地化。

## 排查问题

设置 `GG_DEBUG=1` 可输出带时间戳与调用点上下文的调试日志，
用于定位「候选程序一个都没成功」这类多级回退问题，
以及查看具体是哪个笔记条目被跳过、原因是什么：

```bash
GG_DEBUG=1 gg --browser ls
```

```text
[DEBUG] 2026-09-19T04:34:09Z platform::open_native: `gnome-open` 不在 PATH 中, 跳过
[DEBUG] 2026-09-19T04:34:09Z platform::open_native: 已通过 `xdg-open` 打开 /tmp/gg/gg-render-xxx.html (target="浏览器")
[DEBUG] 2026-09-19T08:56:26Z app::warn_skipped: 跳过 ~/.config/gg/notes/xx.md —— file name is not valid UTF-8
```

## 单个条目损坏时的行为

扫描笔记目录时，单个条目的权限问题、扫描期间被删除、文件名非 UTF-8、
笔记内容读不出来，都**不会**让整个 `gg list` / `gg search` 失败：

```text
$ gg list
有 1 个条目无法读取，已跳过（用 GG_DEBUG=1 查看详情）。
alias
apt
...
```

告警走 stderr，直接接到脚本里不会污染 stdout。

## 项目结构

```text
src/
  main.rs           进程入口：探测语言 → 解析参数 → 分派
  lib.rs            模块导出
  cli.rs            clap 定义 + 本地化帮助
  app.rs            应用编排层 / QueryService（依赖注入）
  config.rs         配置模型、路径解析（强类型枚举）
  i18n.rs           中英文案唯一来源
  notes.rs          笔记仓储层（扫描、读写、正文搜索、模糊建议）
  render.rs         Markdown 渲染（终端 glow / 浏览器）
  editor.rs         编辑器启动与回退链
  ai.rs             AI 笔记生成（多后端 + 提示词 + 输出清洗）
  prompt.rs         交互提示端口
  utils/
    process.rs      可执行命令字符串解析与带超时的子进程等待
    syntax.rs       shell 代码块分词与高亮
    platform.rs     跨平台「打开路径」（含 WSL 互操作）
    layout.rs       终端多列布局（对齐 ls 的列内纵向填充）
    spinner.rs      等待 AI 时的转圈动画
    output.rs       stdout 写入，吸收 EPIPE
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

模板不变量（死选择器、颜色主题化、对比度）的守护用例在 `src/render.rs`
的测试模块里，改模板时若违反会直接失败并指出具体项。

CI（`.github/workflows/ci.yml`）在 ubuntu / macos / windows 三个平台跑完整测试矩阵，
并单独一个 job 跑 `fmt` + `clippy -D warnings`。

## 注意事项

- v1 仅支持「单词命令名」（不能含空格）
- 命令名不能包含 `/`、`\`、`:`
- 仅支持 `.md` 笔记文件
- `list`、`search`、`help` 是子命令名，不能作为普通查询命令名直接使用
- `search` 默认只匹配文件名，需要搜索正文请加 `-c/--content`
- `search -c` 是纯子串匹配，不做 Markdown 语法剥离，行为可预测

## License

MIT，见 [LICENSE](LICENSE)。

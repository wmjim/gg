#!/usr/bin/env python3
"""逐平台编译探针：在没有交叉工具链的机器上验证各平台分支。

本机没有 rustup、也没有 windows/macos 的 std，无法真正交叉编译。但本项目的
平台差异基本只体现为「调哪个可执行文件」「写哪种脚本」，用的都是可移植 API。
于是按平台**逐个**改写 cfg 门控后编译：

- 满足当前平台的 `#[cfg(...)]` → 去掉属性，让该分支参与编译
- 不满足的 → **整项删除**（与真实构建一致；若只置为永假，前面的块会从尾
  表达式降级为普通语句，报出并不存在的类型错误）

为什么必须逐平台而不是一次全开：一次全开时，Linux 分支对某个导入的使用会
掩盖「Windows 上该导入未被使用」这类警告，也掩盖不了别的。逐平台才能复现
真实的编译配置，因此可以配合 `-D warnings` 抓出平台特有的未使用导入/死代码。

局限（两类，都是有意接受的代价）：

1. 若某个平台分支使用了**真正平台专属的 API**（例如 `std::os::windows::*`），
   探针在 Linux 上会误报。本项目当前不依赖这类 API。
2. 探针只做**编译期**检查，查不出运行时语义差异。代码里的 `cfg!(windows)` 在
   探针架构下仍按宿主（Linux）求值，因此「把 POSIX 风格路径交给
   `Path::is_absolute()`」这类断言错误照不出。写用例时避免硬编码绝对路径
   字面量，改用 `tempfile` 之类的平台中立来源。

用法: python3 scripts/cfg_probe.py <源目录> [<目标根目录>]
"""

import pathlib
import re
import shutil
import sys

PROFILES = {
    "linux": {"target_os": "linux", "unix": True, "windows": False},
    "macos": {"target_os": "macos", "unix": True, "windows": False},
    "windows": {"target_os": "windows", "unix": False, "windows": True},
}

# 匹配 `#[cfg(EXPR)]`（只取平台相关的那几种，其余如 cfg(test) 原样保留）
CFG_ATTR = re.compile(r"^([ \t]*)#\[cfg\(([^)]*(?:\([^)]*\))?[^)]*)\)\]", re.MULTILINE)


def split_top_level(expr: str) -> list[str]:
    """按顶层逗号切分，忽略括号内的逗号。"""
    parts, depth, current = [], 0, ""
    for ch in expr:
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
        if ch == "," and depth == 0:
            parts.append(current)
            current = ""
        else:
            current += ch
    parts.append(current)
    return [part.strip() for part in parts if part.strip()]


def evaluate(expr: str, profile: dict) -> bool | None:
    """判定 cfg 表达式；无法判定时返回 None（调用方保留原属性）。"""
    expr = expr.strip()
    try:
        if expr.startswith("all(") and expr.endswith(")"):
            results = [evaluate(part, profile) for part in split_top_level(expr[4:-1])]
            # 任一合取项确定为假 → 整体为假，不必理会未知项。
            # 否则 `all(test, unix)` 这类会因 `test` 未知而放弃判定，
            # 让探针把整块本应删除的代码留在产物里。
            if any(result is False for result in results):
                return False
            return None if None in results else True
        if expr.startswith("any(") and expr.endswith(")"):
            results = [evaluate(part, profile) for part in split_top_level(expr[4:-1])]
            if any(result is True for result in results):
                return True
            return None if None in results else False
        if expr.startswith("not(") and expr.endswith(")"):
            inner = evaluate(expr[4:-1], profile)
            return None if inner is None else not inner
        match = re.fullmatch(r'target_os\s*=\s*"([^"]+)"', expr)
        if match:
            return match.group(1) == profile["target_os"]
        if expr in ("unix", "windows"):
            return bool(profile[expr])
    except Exception:  # noqa: BLE001 - 无法判定一律保留
        return None
    return None


def closing_brace(text: str, open_index: int) -> int:
    depth = 0
    for index in range(open_index, len(text)):
        if text[index] == "{":
            depth += 1
        elif text[index] == "}":
            depth -= 1
            if depth == 0:
                return index
    raise SystemExit("花括号未闭合，探针无法安全改写")


def item_end(text: str, cursor: int) -> int:
    """返回从 `cursor` 开始的那一项的结束索引（含）。"""
    while cursor < len(text) and text[cursor] in " \t\r\n":
        cursor += 1
    if cursor < len(text) and text[cursor] == "{":
        return closing_brace(text, cursor)

    brace = text.find("{", cursor)
    semicolon = text.find(";", cursor)
    if brace != -1 and (semicolon == -1 or brace < semicolon):
        return closing_brace(text, brace)
    if brace != -1 and re.search(r"=\s*$", text[cursor:brace].strip()) is None:
        # 形如 `struct X { .. }` / `fn f() { .. }`
        return closing_brace(text, brace)
    if semicolon != -1:
        return semicolon
    return cursor


def expand_start(text: str, start: int) -> int:
    """把删除起点前移到紧邻的文档注释/属性之前。

    Rust 的 `///` 文档注释等价于 `#[doc]` 属性，属于被门控的那一项；只删
    `#[cfg(...)]` 会把文档注释留在原地，编译时报「expected item after doc
    comment」——那是探针自己造出来的错。
    """
    while start > 0:
        # 取 start 所在行之前的那一行；注意 start 常在行首，
        # 直接 rfind("\n") 会返回同一行的起点。
        end = start - 1 if text[start - 1] == "\n" else start
        line_start = text.rfind("\n", 0, end) + 1
        line = text[line_start:end].strip()
        if line.startswith("///") or line.startswith("//!") or line.startswith("#["):
            start = line_start
            continue
        break
    return start


def rewrite_for_profile(text: str, profile: dict) -> str:
    while True:
        changed = False
        for match in CFG_ATTR.finditer(text):
            verdict = evaluate(match.group(2), profile)
            if verdict is None:
                continue
            if verdict:
                text = text[: match.start(1)] + text[match.end() :]
            else:
                end = item_end(text, match.end())
                start = expand_start(text, match.start())
                text = text[:start] + text[end + 1 :]
            changed = True
            break
        if not changed:
            return text


def build(source: pathlib.Path, root: pathlib.Path) -> list[pathlib.Path]:
    targets = []
    for name, profile in PROFILES.items():
        target = root / name
        if target.exists():
            shutil.rmtree(target)
        target.mkdir(parents=True)
        shutil.copy(source / "Cargo.toml", target / "Cargo.toml")
        shutil.copytree(source / "src", target / "src")
        if (source / "tests").is_dir():
            shutil.copytree(source / "tests", target / "tests")

        for path in list((target / "src").rglob("*.rs")) + list(
            (target / "tests").rglob("*.rs")
        ):
            text = path.read_text(encoding="utf-8")
            if "#[cfg(" in text:
                path.write_text(rewrite_for_profile(text, profile), encoding="utf-8")

        targets.append(target)
        print(f"  生成 {name} 配置 -> {target}")
    return targets


if __name__ == "__main__":
    if len(sys.argv) not in (2, 3):
        raise SystemExit(__doc__)
    source = pathlib.Path(sys.argv[1])
    root = pathlib.Path(sys.argv[2]) if len(sys.argv) == 3 else pathlib.Path("/tmp/cfgprobe")
    for target in build(source, root):
        print(target)

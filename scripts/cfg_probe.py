#!/usr/bin/env python3
"""生成「一次编译所有平台分支」的探针副本。

本机没有 rustup、也没有 windows/macos 的 std，无法真正交叉编译。但本项目的
平台差异基本只体现为「调哪个可执行文件」与「写哪种脚本」，用的都是可移植
API。因此把 `#[cfg(target_os = ...)]` / `#[cfg(unix)]` / `#[cfg(windows)]`
改写后让各平台分支同时参与编译 —— 足以抓出「某个平台分支引用了不存在的
导入或方法」这类只在 CI 才会暴露的错误。

改写规则：
- 正向门控（`#[cfg(unix)]`、`#[cfg(target_os = "windows")]` 等）→ 去掉属性，
  让该分支参与编译；
- 负向门控（`#[cfg(not(...))]`）→ **整项删除**（与真实构建一致）。若只是把
  它们置为永假，前面的块会从「尾表达式」降级成普通语句，探针就会报出并不
  存在的类型错误。
- 去门控后重复定义的函数按平台加后缀，调用点指到 `keep` 那一份。

局限：若某个平台分支使用了**真正平台专属的 API**（例如 `std::os::windows::*`），
探针在 Linux 上会误报。这是它有意接受的代价 —— 本项目当前不依赖这类 API。

用法: python3 scripts/cfg_probe.py <源目录> <目标目录>
"""

import pathlib
import re
import shutil
import sys

# 正向平台门控整行
POSITIVE_GATE = re.compile(
    r'^[ \t]*#\[cfg\((?:any\()?(?:target_os\s*=\s*"[^"]*"|unix|windows)[^\]]*\)\]',
    re.MULTILINE,
)

# 负向平台门控整行
NEGATIVE_GATE = re.compile(r'^[ \t]*#\[cfg\(not\([^\]]*\)\]\]?', re.MULTILINE)

# 去门控后会重复定义的函数：按出现顺序加平台后缀，调用点统一指到 `keep`
DUPLICATES = [
    {
        "file": "src/utils/platform.rs",
        "function": "open_native",
        # 定义顺序即文件中的顺序；`not(any(...))` 的兜底版本会被删除
        "variants": ["windows", "macos", "linux"],
        "keep": "linux",
    },
]


def closing_brace(text: str, open_index: int) -> int:
    """返回与 `text[open_index]` 处 `{` 配对的 `}` 的索引。"""
    depth = 0
    for index in range(open_index, len(text)):
        if text[index] == "{":
            depth += 1
        elif text[index] == "}":
            depth -= 1
            if depth == 0:
                return index
    raise SystemExit("花括号未闭合，探针无法安全改写")


def drop_negative_items(text: str) -> str:
    """删除每一处 `#[cfg(not(...))]` 及其紧跟的那一项。"""
    while True:
        match = NEGATIVE_GATE.search(text)
        if match is None:
            return text

        cursor = match.end()
        while cursor < len(text) and text[cursor] in " \t\r\n":
            cursor += 1

        if cursor < len(text) and text[cursor] == "{":
            end = closing_brace(text, cursor)
        else:
            # 函数 / impl → 到花括号块；let / use 等语句 → 到分号
            brace = text.find("{", cursor)
            semicolon = text.find(";", cursor)
            if brace != -1 and (semicolon == -1 or brace < semicolon):
                end = closing_brace(text, brace)
            elif semicolon != -1:
                end = semicolon
            else:
                end = cursor
        text = text[: match.start()] + text[end + 1 :]


def rename_duplicates(text: str, entry: dict) -> tuple[str, int]:
    function = entry["function"]
    variants = entry["variants"]
    counter = [0]

    def rename(match: re.Match) -> str:
        index = counter[0]
        counter[0] += 1
        if index >= len(variants):
            raise SystemExit(f"{function} 存在第 {index + 1} 个定义，请更新 DUPLICATES")
        return match.group(0).replace(f"fn {function}", f"fn {function}_{variants[index]}")

    text = re.sub(rf"fn {function}\(", rename, text)
    # 定义已改名，剩下的都是调用点，统一指到 keep 那一份
    text = text.replace(f"{function}(", f"{function}_{entry['keep']}(")
    return text, counter[0]


def build(source: pathlib.Path, target: pathlib.Path) -> None:
    if target.exists():
        shutil.rmtree(target)
    target.mkdir(parents=True)
    shutil.copy(source / "Cargo.toml", target / "Cargo.toml")
    shutil.copytree(source / "src", target / "src")
    if (source / "tests").is_dir():
        shutil.copytree(source / "tests", target / "tests")

    for entry in DUPLICATES:
        path = target / entry["file"]
        text = drop_negative_items(path.read_text(encoding="utf-8"))
        text, count = rename_duplicates(POSITIVE_GATE.sub("", text), entry)
        if count != len(entry["variants"]):
            raise SystemExit(
                f"{entry['function']} 定义了 {count} 次，预期 {len(entry['variants'])} 次"
            )
        path.write_text(text, encoding="utf-8")
        print(f"  {entry['file']}: {entry['function']} 改写 {count} 份")

    for path in list((target / "src").rglob("*.rs")) + list((target / "tests").rglob("*.rs")):
        text = path.read_text(encoding="utf-8")
        if "#[cfg(" not in text:
            continue
        text = drop_negative_items(text)
        path.write_text(POSITIVE_GATE.sub("", text), encoding="utf-8")

    print(f"探针已生成于 {target}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    build(pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]))

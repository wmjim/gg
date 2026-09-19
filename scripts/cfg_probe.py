#!/usr/bin/env python3
"""生成「一次编译所有平台分支」的探针副本。

本机没有 rustup、也没有 windows/macos 的 std，无法真正交叉编译。但本项目的
平台差异基本只体现为「调哪个可执行文件」与「写哪种脚本」，用的都是可移植
API。因此把 `#[cfg(target_os = ...)]` / `#[cfg(unix)]` / `#[cfg(windows)]`
全部去掉、并把重名函数改成平台后缀，就能让各平台分支同时参与编译 ——
足以抓出「某个平台分支引用了不存在的导入或方法」这类只在 CI 才会暴露的错误。

局限：若某个平台分支使用了**真正平台专属的 API**（例如 `std::os::windows::*`），
探针在 Linux 上会误报。这是它有意接受的代价 —— 本项目当前不依赖这类 API。

用法: python3 scripts/cfg_probe.py <源目录> <目标目录>
"""

import pathlib
import re
import shutil
import sys

# 正向平台门控：激活（去掉属性），让该分支参与编译
POSITIVE_GATE = re.compile(
    r'^[ \t]*#\[cfg\((?:any\()?(?:target_os\s*=\s*"[^"]*"|unix|windows)[^\]]*\)\]',
    re.MULTILINE,
)

# 负向平台门控：置为永假，保持「不参与编译」——与真实构建的取舍一致。
# 若也一并激活，同一函数里的 unix / not(unix) 两个块会互相打架（尾表达式冲突）。
NEGATIVE_GATE = re.compile(r'^([ \t]*)#\[cfg\(not\([^\]]*\)\]\)?\]', re.MULTILINE)

# 去门控后会重复定义的函数：按出现顺序加平台后缀，调用点统一指到 `keep`
DUPLICATES = [
    {
        "file": "src/utils/platform.rs",
        "function": "open_native",
        "variants": ["windows", "macos", "linux", "other"],
        "keep": "linux",
    },
    {
        "file": "tests/cli_integration.rs",
        "function": "create_fake_claude",
        "variants": ["unix", "windows"],
        "keep": "unix",
    },
]


def strip_gates(text: str) -> str:
    text = NEGATIVE_GATE.sub(r"\1#[cfg(any())]", text)
    return POSITIVE_GATE.sub("", text)


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
        text, count = rename_duplicates(strip_gates(path.read_text(encoding="utf-8")), entry)
        expected = len(entry["variants"])
        if count != expected:
            raise SystemExit(f"{entry['function']} 定义了 {count} 次，预期 {expected} 次")
        path.write_text(text, encoding="utf-8")
        print(f"  {entry['file']}: {entry['function']} 去门控并改名 {count} 份")

    for path in list((target / "src").rglob("*.rs")) + list((target / "tests").rglob("*.rs")):
        text = path.read_text(encoding="utf-8")
        if "cfg(" in text and ("target_os" in text or "unix" in text or "windows" in text):
            path.write_text(strip_gates(text), encoding="utf-8")

    print(f"探针已生成于 {target}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    build(pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]))

#!/usr/bin/env python3
"""检查文档里以纯文本形式出现的 .md 文件名是否真的存在。

tools/verify_docs.py 只用正则匹配 markdown 链接 `[文字](目标)`，因此像
「1. DH-BOT-WINDOWS-CONTEXT.md」这样直接写在正文里的裸文件名它完全看不见。
docs/WINDOWS-CODEX-PROMPT.md 曾经这样引用一个只存在于 output/ 交接包里的
文件（output/ 被 .gitignore 忽略），克隆仓库的人按清单找不到它，而 CI 一直是绿的。
这个脚本补上那个缺口。
"""

import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# 裸文件名形式的 .md 引用。前后用非路径字符界定，避免把 `docs/X.md` 这类
# 带目录的引用重复报一遍——那种形式已经由 verify_docs.py 的链接检查覆盖。
BARE_MD = re.compile(r"(?<![\w/.-])([A-Za-z0-9][\w.一-鿿-]*\.md)(?![\w/])")

# 运行时由脚本生成、按设计不进仓库的产物名。每一项都注明出处，
# 避免以后有人不清楚为什么放行就直接删掉白名单。
GENERATED_ARTIFACTS = {
    # tauri3/scripts/package-windows.ps1 从 docs/DH使用手册.md 复制生成
    "DH-Manual-ZH.md",
    # tauri3/scripts/test-windows-real-machine.ps1 运行后写到桌面报告目录
    "TEST-REPORT.md",
    "DH-BOT-Windows-Real-Machine.md",
}


def tracked_files() -> set:
    # 必须用 -z：默认输出会把非 ASCII 路径转义成 "docs/DH\\344\\275\\277...".
    # 那样 basenames 里存的是转义后的名字，DH使用手册.md 之类会被误判成未跟踪。
    out = subprocess.run(
        ["git", "ls-files", "-z"], cwd=REPO_ROOT, capture_output=True, text=True, check=True
    ).stdout
    return {p for p in out.split("\0") if p}


def scan() -> int:
    tracked = tracked_files()
    basenames = {Path(p).name for p in tracked}

    targets = sorted(p for p in tracked if p == "README.md" or p.startswith("docs/"))
    targets = [p for p in targets if p.endswith(".md")]

    errors: list[str] = []
    warnings: list[str] = []

    for rel in targets:
        path = REPO_ROOT / rel
        if not path.exists():
            continue
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            for name in dict.fromkeys(BARE_MD.findall(line)):
                if name in GENERATED_ARTIFACTS:
                    continue
                if name == Path(rel).name:
                    continue
                if name in basenames:
                    continue
                sibling = path.parent / name
                if sibling.exists():
                    # 文件在磁盘上但没被 git 跟踪：本机能过，克隆的人拿不到。
                    warnings.append(
                        f"{rel}:{number} 引用 {name}，该文件存在但未被 git 跟踪，克隆仓库后会缺失"
                    )
                    continue
                errors.append(f"{rel}:{number} 引用了不存在的文档 {name}")

    for warning in warnings:
        print(f"warning: {warning}")

    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        print(
            f"\n共 {len(errors)} 处纯文本文档引用无法解析。"
            "若是脚本生成的产物，请加入 GENERATED_ARTIFACTS 并注明出处。",
            file=sys.stderr,
        )
        return 1

    print(f"纯文本文档引用检查通过：{len(targets)} 个文件，{len(warnings)} 条警告")
    return 0


if __name__ == "__main__":
    sys.exit(scan())

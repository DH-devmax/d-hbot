#!/usr/bin/env python3
"""Verify documentation links and version/schema facts against source files."""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
LINK = re.compile(r"\[[^\]]+\]\(([^)]+)\)")


def tracked_markdown() -> list[Path]:
    result = subprocess.run(
        [
            "git",
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "*.md",
        ],
        cwd=ROOT,
        check=True,
        capture_output=True,
    )
    return [ROOT / value.decode("utf-8") for value in result.stdout.split(b"\0") if value]


def relative_link_errors(path: Path) -> list[str]:
    errors: list[str] = []
    text = path.read_text("utf-8")
    for target in LINK.findall(text):
        target = target.strip().strip("<>")
        if not target or target.startswith(("#", "http://", "https://", "mailto:")):
            continue
        target = target.split("#", 1)[0]
        resolved = (path.parent / target).resolve()
        if not resolved.exists():
            errors.append(f"{path.relative_to(ROOT)}: missing link target {target}")
    return errors


def source_facts() -> tuple[str, int]:
    package = json.loads((ROOT / "tauri3/package.json").read_text("utf-8"))
    tauri = json.loads((ROOT / "tauri3/src-tauri/tauri.conf.json").read_text("utf-8"))
    cargo = (ROOT / "tauri3/src-tauri/Cargo.toml").read_text("utf-8")
    cargo_version = re.search(r'^version = "([^"]+)"$', cargo, re.MULTILINE)
    if cargo_version is None:
        raise RuntimeError("Cargo package version not found")
    versions = {package["version"], tauri["version"], cargo_version.group(1)}
    if len(versions) != 1:
        raise RuntimeError(f"version mismatch: {sorted(versions)}")

    database = (ROOT / "tauri3/src-tauri/src/database.rs").read_text("utf-8")
    schema_matches = re.findall(r"PRAGMA user_version\s*=\s*(\d+)", database)
    if not schema_matches:
        raise RuntimeError("schema version not found")
    return versions.pop(), max(int(value) for value in schema_matches)


def main() -> int:
    errors: list[str] = []
    version, schema = source_facts()
    required_facts = {
        "README.md": (version, f"schema v{schema}"),
        "docs/DATABASE-SCHEMA.md": (f"schema v{schema}",),
        "tauri3/STATUS.md": (version.split("-")[0], f"schema v{schema}"),
    }
    for relative, facts in required_facts.items():
        text = (ROOT / relative).read_text("utf-8")
        for fact in facts:
            if fact not in text:
                errors.append(f"{relative}: missing current fact {fact!r}")

    for path in tracked_markdown():
        errors.extend(relative_link_errors(path))

    if errors:
        print("Documentation verification failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print(f"Documentation verified: version={version} schema=v{schema}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Fail when tracked repository content contains credential or identity leaks."""

from __future__ import annotations

import re
import subprocess
import sys
import zipfile
from pathlib import Path, PurePosixPath


ROOT = Path(__file__).resolve().parents[1]
TEXT_LIMIT = 8 * 1024 * 1024
BLOCKED_PARTS = {".secrets", "node_modules", "raw", "target", "test-results"}
BLOCKED_SUFFIXES = {".db", ".dat", ".key", ".log", ".p12", ".pfx", ".pdb"}
BINARY_SUFFIXES = {".gif", ".icns", ".ico", ".jpeg", ".jpg", ".pdf", ".png"}
STRUCTURED_EXCLUSIONS = {
    "docs/ZCG-REFERENCE-SHA256.txt",
    "tauri3/Cargo.lock",
    "tauri3/pnpm-lock.yaml",
    "tauri3/contracts/prediction_v1.json",
    "tauri3/contracts/wangshangliao_capabilities.json",
}
STRUCTURED_PREFIX_EXCLUSIONS = ("tauri3/contracts/",)
LONG_ID_TEST_EXCLUSIONS = {"tauri3/scripts/sanitize-contract-capture.test.mjs"}
PATH_PATTERN_EXCLUSIONS = {"tauri3/src-tauri/src/diagnostics.rs"}

PATTERNS = (
    ("OpenAI-compatible API key", re.compile(r"\bsk-[A-Za-z0-9_-]{16,}\b")),
    ("GitHub token", re.compile(r"\b(?:gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,})\b")),
    ("Bearer credential", re.compile(r"\bBearer\s+[A-Za-z0-9._~+/=-]{12,}", re.IGNORECASE)),
    ("JWT", re.compile(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b")),
    ("private key", re.compile(r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----")),
    ("credential URL", re.compile(r"https?://[^\s/:]+:[^\s/@]+@", re.IGNORECASE)),
    ("real local macOS path", re.compile(r"/Users/(?!alice(?:/|\b)|USERNAME(?:/|\b)|<user>(?:/|\b))[^/\s]+/")),
    ("real local Windows path", re.compile(r"C:\\Users\\(?!alice(?:\\|\b)|USERNAME(?:\\|\b)|<user>(?:\\|\b))[^\\\s]+\\", re.IGNORECASE)),
    ("real test group name", re.compile(r"[\u4e00-\u9fffA-Za-z0-9②]{2,24}兼职群")),
    ("unredacted long object id", re.compile(r"(?<![A-Fa-f0-9])[0-9]{17,}(?![A-Fa-f0-9])")),
)

TEST_LITERAL_ALLOWLIST = {
    "Bearer abcdefghijklmnopqrstuvwxyz",
    "eyJhbGciOiJIUzI1NiJ9.cGF5bG9hZC1zZW50aW5lbA.c2lnbmF0dXJlLXNlbnRpbmVs",
}


def tracked_files() -> list[str]:
    result = subprocess.run(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        cwd=ROOT,
        check=True,
        capture_output=True,
    )
    return [value.decode("utf-8") for value in result.stdout.split(b"\0") if value]


def blocked_path(path: PurePosixPath) -> str | None:
    if path.as_posix() == "tauri3/contracts/raw/.gitignore":
        return None
    if any(part in BLOCKED_PARTS for part in path.parts):
        return "forbidden generated, raw, or secret directory"
    if path.suffix.lower() in BLOCKED_SUFFIXES:
        return "forbidden secret, database, log, or debug artifact"
    return None


def allowed_test_literal(path: str, value: str) -> bool:
    return (
        path
        in {
            "tauri3/scripts/sanitize-contract-capture.test.mjs",
            "tools/verify_repository_redaction.py",
        }
        and value in TEST_LITERAL_ALLOWLIST
    )


def scan_text(path: str, text: str) -> list[str]:
    findings: list[str] = []
    structured_excluded = path in STRUCTURED_EXCLUSIONS or path.startswith(
        STRUCTURED_PREFIX_EXCLUSIONS
    )
    for line_number, line in enumerate(text.splitlines(), start=1):
        for label, pattern in PATTERNS:
            if path == "tools/verify_repository_redaction.py" and "re.compile" in line:
                continue
            if structured_excluded and label == "unredacted long object id":
                continue
            if path in LONG_ID_TEST_EXCLUSIONS and label == "unredacted long object id":
                continue
            if path in PATH_PATTERN_EXCLUSIONS and label in {
                "real local macOS path",
                "real local Windows path",
            }:
                continue
            for match in pattern.finditer(line):
                value = match.group(0)
                if label == "unredacted long object id" and (
                    "let mut hash" in line or "wrapping_mul" in line
                ):
                    continue
                if allowed_test_literal(path, value):
                    continue
                findings.append(f"{path}:{line_number}: {label}")
    return findings


def scan_bytes(path: str, payload: bytes) -> list[str]:
    if len(payload) > TEXT_LIMIT and Path(path).suffix.lower() not in BINARY_SUFFIXES:
        return [f"{path}: tracked file exceeds {TEXT_LIMIT} bytes and cannot be audited safely"]
    text = payload.decode("utf-8", errors="ignore")
    if Path(path).suffix.lower() in BINARY_SUFFIXES:
        findings: list[str] = []
        for label, pattern in PATTERNS[:6]:
            if pattern.search(text):
                findings.append(f"{path}: binary contains {label}")
        return findings
    return scan_text(path, text)


def scan_zip(path: str, archive_path: Path) -> list[str]:
    findings: list[str] = []
    with zipfile.ZipFile(archive_path) as archive:
        for item in archive.infolist():
            inner = PurePosixPath(item.filename)
            if item.is_dir():
                continue
            reason = blocked_path(inner)
            if reason:
                findings.append(f"{path}!{item.filename}: {reason}")
                continue
            if item.file_size > TEXT_LIMIT:
                findings.append(f"{path}!{item.filename}: archive member exceeds audit limit")
                continue
            findings.extend(scan_bytes(f"{path}!{item.filename}", archive.read(item)))
    return findings


def main() -> int:
    findings: list[str] = []
    for relative in tracked_files():
        pure = PurePosixPath(relative)
        reason = blocked_path(pure)
        if reason:
            findings.append(f"{relative}: {reason}")
            continue
        absolute = ROOT / relative
        if pure.suffix.lower() == ".zip":
            findings.extend(scan_zip(relative, absolute))
        else:
            findings.extend(scan_bytes(relative, absolute.read_bytes()))

    if findings:
        print("Repository redaction audit failed:", file=sys.stderr)
        for finding in findings:
            print(f"- {finding}", file=sys.stderr)
        return 1
    print("Repository redaction audit passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

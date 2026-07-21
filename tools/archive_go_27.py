#!/usr/bin/env python3
"""Build and verify the read-only DH BOT Go 2.7 source archive."""

from __future__ import annotations

import argparse
import hashlib
import re
import subprocess
import sys
import zipfile
from dataclasses import dataclass
from pathlib import Path, PurePosixPath


TAG = "go-2.7-final"
EXPECTED_TAG_OBJECT = "0da624fdd017c71cefc98f3cb28ee8a34714c114"
EXPECTED_COMMIT = "a4bf37fcde40c5342de0b6b72348f32e876cb5ec"
PREFIX = "DH-BOT-go-2.7-final"
ARCHIVE_NAME = "DH-BOT-go-2.7-final-source.zip"
FIXED_ZIP_TIME = (1980, 1, 1, 0, 0, 0)

INCLUDED_FILES = {
    "AGENTS.md",
    "Makefile",
    "README.md",
    "build.ps1",
    "go.mod",
    "go.sum",
    "tools/build_manual_pdf.py",
    "tools/dh-macos-bridge.mjs",
}
INCLUDED_PREFIXES = ("assets/", "cmd/", "docs/", "internal/", "package/")
EXCLUDED_FILES = {
    "cmd/dh/rsrc_windows_amd64.syso",
    "docs/ZCG-REFERENCE-SHA256.txt",
    "docs/ZCG参考材料.md",
}
RENAMED_FILES = {"docs/DH使用手册.md": "docs/DH-Manual-ZH.md"}
FORBIDDEN_PARTS = {
    ".git",
    ".github",
    "build",
    "dist",
    "logs",
    "node_modules",
    "runtime-test",
    "target",
    "testdata",
    "tmp",
}
FORBIDDEN_SUFFIXES = {
    ".bak",
    ".db",
    ".dll",
    ".dmp",
    ".env",
    ".exe",
    ".key",
    ".log",
    ".p12",
    ".pem",
    ".pfx",
    ".sqlite",
    ".sqlite3",
    ".syso",
    ".tmp",
}
SECRET_PATTERNS = (
    re.compile(rb"sk-[A-Za-z0-9_-]{16,}"),
    re.compile(
        rb"(?i)(?:api[_ -]?key|authorization|cookie|token)\s*[:=]\s*[\"']?[A-Za-z0-9_.-]{16,}"
    ),
    re.compile(rb"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----"),
)


class ArchiveError(RuntimeError):
    pass


@dataclass(frozen=True)
class SourceFile:
    source_path: str
    archive_path: str
    mode: int
    object_id: str
    content: bytes


def run_git(repo: Path, *args: str) -> bytes:
    result = subprocess.run(
        ["git", *args], cwd=repo, check=False, capture_output=True
    )
    if result.returncode:
        detail = result.stderr.decode("utf-8", errors="replace").strip()
        raise ArchiveError(f"git {' '.join(args)} failed: {detail}")
    return result.stdout


def included(path: str) -> bool:
    return path in INCLUDED_FILES or path.startswith(INCLUDED_PREFIXES)


def validate_path(path: str) -> None:
    pure = PurePosixPath(path)
    if pure.is_absolute() or ".." in pure.parts:
        raise ArchiveError(f"unsafe archive path: {path}")
    lowered_parts = {part.lower() for part in pure.parts}
    if lowered_parts & FORBIDDEN_PARTS:
        raise ArchiveError(f"forbidden generated/runtime path: {path}")
    if pure.suffix.lower() in FORBIDDEN_SUFFIXES:
        raise ArchiveError(f"forbidden generated/secret file: {path}")
    lowered_name = pure.name.lower()
    if lowered_name in {"secrets.dat", "state.json", "runtime-mode"}:
        raise ArchiveError(f"forbidden runtime state: {path}")


def validate_content(path: str, content: bytes) -> None:
    for pattern in SECRET_PATTERNS:
        if pattern.search(content):
            raise ArchiveError(f"credential-like value found in {path}")


def source_files(repo: Path) -> tuple[str, list[SourceFile]]:
    tag_object = run_git(repo, "rev-parse", TAG).decode().strip()
    commit = run_git(repo, "rev-parse", f"{TAG}^{{commit}}").decode().strip()
    if tag_object != EXPECTED_TAG_OBJECT or commit != EXPECTED_COMMIT:
        raise ArchiveError(
            f"{TAG} provenance differs: tag={tag_object}, commit={commit}"
        )
    raw = run_git(repo, "ls-tree", "-r", "-z", TAG)
    selected: list[SourceFile] = []
    for record in raw.split(b"\0"):
        if not record:
            continue
        metadata, encoded_path = record.split(b"\t", 1)
        mode_text, object_type, object_id = metadata.decode("ascii").split()
        path = encoded_path.decode("utf-8")
        if object_type != "blob" or not included(path) or path in EXCLUDED_FILES:
            continue
        archive_path = RENAMED_FILES.get(path, path)
        validate_path(archive_path)
        content = run_git(repo, "cat-file", "blob", object_id)
        validate_content(archive_path, content)
        permission = 0o755 if mode_text.endswith("755") else 0o644
        selected.append(
            SourceFile(path, archive_path, permission, object_id, content)
        )

    selected.sort(key=lambda item: item.archive_path)
    if not selected:
        raise ArchiveError(f"tag {TAG} produced no source files")
    archive_paths = [item.archive_path for item in selected]
    if len(archive_paths) != len(set(archive_paths)):
        raise ArchiveError("archive path mapping contains duplicates")
    if not any(path.endswith(".go") for path in archive_paths):
        raise ArchiveError("archive does not contain Go source")
    if any(path.startswith("tauri3/") for path in archive_paths):
        raise ArchiveError("Tauri source crossed the Go archive boundary")
    return commit, selected


def zip_info(name: str, mode: int) -> zipfile.ZipInfo:
    info = zipfile.ZipInfo(f"{PREFIX}/{name}", FIXED_ZIP_TIME)
    info.compress_type = zipfile.ZIP_STORED
    info.create_system = 3
    info.external_attr = (0o100000 | mode) << 16
    return info


def checksum(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def build(repo: Path, output: Path, checksum_file: Path) -> tuple[str, int]:
    commit, files = source_files(repo)
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_suffix(output.suffix + ".tmp")
    with zipfile.ZipFile(temporary, "w", compression=zipfile.ZIP_STORED) as archive:
        archive.comment = f"{TAG} {commit}".encode("ascii")
        for item in files:
            archive.writestr(zip_info(item.archive_path, item.mode), item.content)
    temporary.replace(output)
    digest = checksum(output)
    checksum_file.write_text(f"{digest}  {output.name}\n", encoding="ascii")
    return commit, len(files)


def verify(repo: Path, output: Path, checksum_file: Path) -> tuple[str, int]:
    commit, expected_files = source_files(repo)
    if not output.is_file():
        raise ArchiveError(f"archive is missing: {output}")
    expected_digest = f"{checksum(output)}  {output.name}\n"
    if not checksum_file.is_file() or checksum_file.read_text("ascii") != expected_digest:
        raise ArchiveError("SHA256SUMS.txt does not match the source ZIP")

    expected = {
        f"{PREFIX}/{item.archive_path}": item for item in expected_files
    }
    with zipfile.ZipFile(output) as archive:
        actual_names = archive.namelist()
        if len(actual_names) != len(set(actual_names)):
            raise ArchiveError("ZIP contains duplicate entries")
        if set(actual_names) != set(expected):
            missing = sorted(set(expected) - set(actual_names))
            extra = sorted(set(actual_names) - set(expected))
            raise ArchiveError(f"ZIP file set differs from tag; missing={missing}, extra={extra}")
        for name, item in expected.items():
            info = archive.getinfo(name)
            archived_mode = (info.external_attr >> 16) & 0o777
            if archived_mode != item.mode:
                raise ArchiveError(f"file mode differs from tag: {name}")
            content = archive.read(name)
            if content != item.content:
                raise ArchiveError(f"file content differs from tag: {name}")
            validate_content(name, content)
    return commit, len(expected_files)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    action = parser.add_mutually_exclusive_group()
    action.add_argument("--build", action="store_true", help="rebuild then verify")
    action.add_argument("--verify", action="store_true", help="verify existing files")
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--output", type=Path)
    parser.add_argument("--checksum-file", type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    repo = args.repo.resolve()
    output = args.output or repo / "archive" / TAG / ARCHIVE_NAME
    checksum_file = args.checksum_file or output.parent / "SHA256SUMS.txt"
    try:
        if args.verify:
            commit, count = verify(repo, output, checksum_file)
        else:
            commit, count = build(repo, output, checksum_file)
            verify(repo, output, checksum_file)
    except (ArchiveError, OSError, zipfile.BadZipFile) as error:
        print(f"Go 2.7 archive verification failed: {error}", file=sys.stderr)
        return 1
    print(
        f"Go 2.7 archive verified: tag={TAG} commit={commit} files={count} "
        f"sha256={checksum(output)}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

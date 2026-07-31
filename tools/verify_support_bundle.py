#!/usr/bin/env python3
"""Read-only verifier for a DH BOT diagnostic ZIP.

The bundle is untrusted input. This script never extracts or executes files;
it checks archive paths, manifest metadata, and SHA-256 entries in place.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import posixpath
import sys
import zipfile


REQUIRED = {
    "README.txt",
    "diagnostic.json",
    "audit-summary.json",
    "SHA256SUMS.txt",
    "manifest.json",
}
FORBIDDEN_NAMES = {
    "dh.db",
    "dh.db-wal",
    "dh.db-shm",
    "secrets.dat",
    "cookies",
    "cookie",
    "local storage",
    "source code",
}
MAX_ARCHIVE_BYTES = 128 * 1024 * 1024


def safe_name(name: str) -> bool:
    normalized = posixpath.normpath(name.replace("\\", "/"))
    return (
        bool(name)
        and not normalized.startswith("../")
        and normalized != ".."
        and not normalized.startswith("/")
    )


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def read_entry(archive: zipfile.ZipFile, name: str) -> bytes:
    try:
        return archive.read(name)
    except Exception as error:  # malformed or CRC-broken archives are untrusted input
        raise ValueError(f"无法读取 {name}：{error}") from error


def fail(message: str) -> int:
    print(f"校验失败：{message}", file=sys.stderr)
    return 1


def main() -> int:
    parser = argparse.ArgumentParser(description="验证 DH BOT 诊断包完整性")
    parser.add_argument("bundle", help="DH-BOT-support-*.zip")
    args = parser.parse_args()

    try:
        archive = zipfile.ZipFile(args.bundle)
    except (OSError, zipfile.BadZipFile) as error:
        return fail(f"无法打开 ZIP：{error}")

    with archive:
        names = archive.namelist()
        if len(names) != len(set(names)):
            return fail("ZIP 中存在重复文件名")
        if any(not safe_name(name) for name in names):
            return fail("ZIP 中存在绝对路径或路径穿越项")
        total_bytes = sum(info.file_size for info in archive.infolist())
        if total_bytes > MAX_ARCHIVE_BYTES:
            return fail("ZIP 未压缩大小超过 128 MiB 支持包上限")
        missing = REQUIRED.difference(names)
        if missing:
            return fail(f"缺少必需文件：{', '.join(sorted(missing))}")
        suspicious = [
            name
            for name in names
            if name.lower() in FORBIDDEN_NAMES
            or any(token in name.lower() for token in (".pdb", ".map", "dh.db", "secrets.dat"))
        ]
        if suspicious:
            return fail(f"包含不应进入诊断包的文件名：{', '.join(suspicious)}")

        try:
            checksums_text = read_entry(archive, "SHA256SUMS.txt").decode("utf-8")
            manifest = json.loads(read_entry(archive, "manifest.json"))
        except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
            return fail(f"清单格式无效：{error}")

        checksums: dict[str, str] = {}
        for line in checksums_text.splitlines():
            if not line.strip():
                continue
            parts = line.split("  ", 1)
            if len(parts) != 2 or len(parts[0]) != 64:
                return fail(f"SHA256SUMS.txt 行格式无效：{line!r}")
            expected, name = parts
            if name in checksums:
                return fail(f"SHA256SUMS.txt 重复文件：{name}")
            if name not in names or name in {"SHA256SUMS.txt", "manifest.json"}:
                return fail(f"SHA256SUMS.txt 引用了不存在或不允许自引用的文件：{name}")
            checksums[name] = expected

        manifest_files = manifest.get("files")
        if not isinstance(manifest_files, list):
            return fail("manifest.json 缺少 files 数组")
        manifest_map = {}
        for entry in manifest_files:
            if not isinstance(entry, dict) or not isinstance(entry.get("path"), str):
                return fail("manifest.json 存在无效文件项")
            name = entry["path"]
            if name in manifest_map:
                return fail(f"manifest.json 重复文件：{name}")
            if name not in names or name == "manifest.json":
                return fail(f"manifest.json 引用了不存在或不允许自引用的文件：{name}")
            manifest_map[name] = entry

        if set(names) != set(manifest_map) | {"manifest.json"}:
            return fail("ZIP 文件集合与 manifest.json 不一致")
        if set(manifest_map) != set(checksums) | {"SHA256SUMS.txt"}:
            return fail("manifest.json 与 SHA256SUMS.txt 文件集合不一致")
        for name, expected in checksums.items():
            try:
                data = read_entry(archive, name)
            except ValueError as error:
                return fail(str(error))
            if digest(data) != expected:
                return fail(f"SHA-256 不匹配：{name}")
            entry = manifest_map[name]
            if entry.get("bytes") != len(data) or entry.get("sha256") != expected:
                return fail(f"manifest.json 元数据不匹配：{name}")

        try:
            sums_data = read_entry(archive, "SHA256SUMS.txt")
        except ValueError as error:
            return fail(str(error))
        sums_entry = manifest_map["SHA256SUMS.txt"]
        if sums_entry.get("bytes") != len(sums_data) or sums_entry.get("sha256") != digest(sums_data):
            return fail("manifest.json 未正确记录 SHA256SUMS.txt")

    print(f"校验通过：{args.bundle}（{len(checksums)} 个诊断文件）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

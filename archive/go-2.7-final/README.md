# DH BOT Go 2.7 source archive

This directory preserves the final Go/Win32 2.7 implementation as a read-only
architecture reference. The active DH BOT application is the Rust/Tauri 3.0
codebase; this archive is not a build input for the current branch.

## Contents

- `DH-BOT-go-2.7-final-source.zip`: source exported from Git tag
  `go-2.7-final`.
- `SHA256SUMS.txt`: SHA-256 checksum for the ZIP.

The ZIP includes the Go module, Win32 entry points, tests, protocol and business
packages, brand assets, build scripts, and the 2.7 documentation. It excludes
the Tauri 3 source tree, CI configuration, compiled resource objects, generated
build output, logs, databases, secrets, runtime state, and local test data.

To inspect the archive without adding Go sources back to the active branch:

```text
unzip -l DH-BOT-go-2.7-final-source.zip
```

Rebuild and verify from the repository root:

```text
python3 tools/archive_go_27.py --build
python3 tools/archive_go_27.py --verify
python3 -m unittest tools/test_archive_go_27.py
```

The generator uses a fixed file allowlist, deterministic ZIP metadata and
per-file Git blob comparison. Verification also rejects generated binaries,
runtime data and credential-like values.

Archive source: annotated tag `go-2.7-final` (tag object
`0da624fdd017c71cefc98f3cb28ee8a34714c114`), commit
`a4bf37fcde40c5342de0b6b72348f32e876cb5ec`.

The archived Chinese manual is named `docs/DH-Manual-ZH.md` so its path extracts
consistently on Windows and macOS.

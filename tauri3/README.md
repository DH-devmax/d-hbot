# DH BOT 3.0 Rust + Tauri migration workspace

This directory contains the DH BOT 3.0 Rust + Tauri beta.1. The production
application is Rust/Tauri only; the former Go 2.7 implementation is preserved
by the repository tag `go-2.7-final`.

## Target layout

- `src-tauri/`: Rust application shell, SQLite, tray, single-instance and
  protocol services.
- `src/`: React + TypeScript GPT-style workspace UI.
- `contracts/`: versioned, redacted protocol fixtures plus archived Go expectations.

The 3.0 beta deliberately uses a clean database at `%APPDATA%\\DH\\3.0\\dh.db`.
On first run it archives legacy `dh.db` and `secrets.dat` under
`%APPDATA%\\DH\\legacy-backups\\<timestamp>` and does not migrate test data.
AI starts with empty URLs and key, and model `deepseek-v4-pro`.旺商聊 keeps its
own Electron login data; Windows startup uses the fixed `dh-primary` partition
after a version-checked, backed-up script patch. A controlled Windows developer
machine creates the NSIS installer with bundled WebView2 and the portable ZIP
after Beta.2 device checks; GitHub Actions are not part of this release path.

Development:

```sh
pnpm install --frozen-lockfile
pnpm test
pnpm tauri:dev
pnpm tauri:build
```

Production and internal developer packages are separate build channels:

```sh
pnpm tauri:build:production   # real wangshangliao, port 9222, no Fixture feature
pnpm tauri:build:developer    # internal DH BOT Dev + DH-Fixture
pnpm test:production-isolation
```

The production build clears staged developer resources, compiles `dh-bot` with
no default Cargo features, and scans the frontend and Windows artifacts for
Fixture commands, ports and data paths before release.

For user-reported failures, the desktop app provides **Debug -> Generate support bundle**. The ZIP stays local until the user shares it and contains redacted runtime health, capability information, anonymized audit records, recent redacted logs and checksums. It never includes SQLite data, secrets, WangShangLiao login data, raw messages, real group names or Fixture data. Repeated exports use unique names and local retention is bounded to 20 bundles, 30 days and 128 MiB. Run `python3 ../tools/verify_repository_redaction.py` before committing source or docs. See [`../docs/DIAGNOSTICS-AND-SUPPORT.md`](../docs/DIAGNOSTICS-AND-SUPPORT.md).

Architecture and implementation references:

- [`../docs/ARCHITECTURE.md`](../docs/ARCHITECTURE.md): data-flow overview.
- [`../docs/TECHNICAL-DESIGN.md`](../docs/TECHNICAL-DESIGN.md): module map, executor, workers and failure semantics.
- [`../docs/API-REFERENCE.md`](../docs/API-REFERENCE.md): production Tauri commands, events and developer-only boundaries.
- [`../docs/DATABASE-SCHEMA.md`](../docs/DATABASE-SCHEMA.md): schema v11 and persistence boundaries.
- [`../docs/PROTOCOL-CONTRACT.md`](../docs/PROTOCOL-CONTRACT.md): ZCG baseline, WangShangLiao protocol and receipts.

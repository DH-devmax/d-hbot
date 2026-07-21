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
after a version-checked, backed-up script patch. Windows CI publishes an NSIS
installer with bundled WebView2 and a portable ZIP after Beta.2 device checks.

Development:

```sh
pnpm install
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

# DH BOT 3.0 Rust + Tauri migration workspace

This directory contains the DH BOT 3.0 Rust + Tauri beta.1. The Go/Win32
application remains the production implementation for 2.7.x.

## Target layout

- `src-tauri/`: Rust application shell, SQLite, tray, single-instance and
  protocol services.
- `src/`: React + TypeScript GPT-style workspace UI.
- `contracts/`: versioned JSON fixtures shared by Go and Rust tests.

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

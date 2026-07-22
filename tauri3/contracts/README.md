# DH BOT contract fixtures

The first frozen contracts are:

- `GroupGateway`: group/member reads, text, recall, mute, rename, remove,
  whole-group mute and calibrated group announcements.
- `AIProvider`: request version `1`, decision fields `reply`, `actions`,
  `tasks`, `confidence`, `reason`.
- `PredictionSnapshot`: game, period, normalized result, update time and
  freshness state.
- `GroupSchedule`: local daily open/close times, group bindings and idempotent
  run key.

The fixture files are `group_gateway_v1.json`, `group_gateway_v2.json`,
`group_announcement_v2.json`, `ai_provider_v1.json`, `prediction_v1.json` and
`schedule_v1.json`.

`wangshangliao_capabilities.json` is the production-only calibration registry.
It may only receive a version/hash pair after a sanitized real trace has passed
replay. Fixture versions, URLs and ports never belong in that file. With no
exact match, reads remain available while every write capability is
`Unverified`. Group announcements are enabled only for a verified app version
and script hash.

`group_gateway_v2.json` freezes the application file version, page title/URL,
main-script SHA-256, request, transport envelope, business envelope, normalized
receipt, callbacks and expected normalized state. Capability calibration uses
the exact `(appFileVersion, mainScriptSha256)` pair. A new or changed build is
`Unverified` until its sanitized trace is replayed successfully.

Raw traces belong in `contracts/raw/`, whose contents are ignored by Git. A
developer first records actual IPC/NIM request, response, callback and resulting
state JSON files from the developer build. The collector reads the running local
DevTools page and the installed main script to pin the exact page fingerprint:

```text
node scripts/assemble-contract-capture.mjs \
  --devtools http://127.0.0.1:9222 \
  --app-version APP_VERSION \
  --main-script "C:\\...\\resources\\app\\dist-electron\\main\\index.js" \
  --operations contracts/raw/operations.json \
  --callbacks contracts/raw/callbacks.json \
  --state contracts/raw/state.json \
  --output contracts/raw/TRACE.json
```

`operations.json` must contain the actual `request`, `transport`, `business`,
`normalizedReceipt` and `expectedNormalizedState` for each operation. The
assembler refuses incomplete envelope fields and refuses to write outside the
ignored raw directory. Before adding or updating a frozen trace, run:

```text
node scripts/sanitize-contract-capture.mjs contracts/raw/TRACE.json contracts/TRACE.sanitized.json
node scripts/sanitize-contract-capture.mjs --self-test
node --test scripts/sanitize-contract-capture.test.mjs scripts/assemble-contract-capture.test.mjs
```

The sanitizer sorts object keys, assigns stable
`ACCOUNT/GROUP/USER/NIM/MESSAGE` and display-name placeholders, and stops when
it finds API keys, authorization values, cookies, passwords, tokens or private
keys. Review the sanitized file before replacing a frozen contract; raw traces
must remain local.

# DH BOT contract fixtures

The first frozen contracts are:

- `GroupGateway`: group/member reads, text, recall, mute, rename, remove and
  whole-group mute.
- `AIProvider`: request version `1`, decision fields `reply`, `actions`,
  `tasks`, `confidence`, `reason`.
- `PredictionSnapshot`: game, period, normalized result, update time and
  freshness state.
- `GroupSchedule`: local daily open/close times, group bindings and idempotent
  run key.

The fixture files are `group_gateway_v1.json`, `group_gateway_v2.json`,
`ai_provider_v1.json`, `prediction_v1.json` and `schedule_v1.json`.

`group_gateway_v2.json` freezes the application file version, page title/URL,
main-script SHA-256, request, transport envelope, business envelope, normalized
receipt, callbacks and expected normalized state. Capability calibration uses
the exact `(appFileVersion, mainScriptSha256)` pair. A new or changed build is
`Unverified` until its sanitized trace is replayed successfully.

Raw traces belong in `contracts/raw/`, whose contents are ignored by Git. Before
adding or updating a frozen trace, run:

```text
node scripts/sanitize-contract-capture.mjs contracts/raw/TRACE.json contracts/TRACE.sanitized.json
node scripts/sanitize-contract-capture.mjs --self-test
node --test scripts/sanitize-contract-capture.test.mjs
```

The sanitizer sorts object keys, assigns stable `ACCOUNT/GROUP/USER/NIM/MESSAGE`
and display-name placeholders, and stops when it finds API keys, authorization values, cookies,
passwords, tokens or private keys. Review the sanitized file before replacing a
frozen contract; raw traces must remain local.

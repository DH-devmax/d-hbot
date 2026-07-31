# DH BOT contract fixtures

ZCG 群管理基线由 Rust 内置路由常量、参数校验和双层 envelope 测试覆盖，运行时通过 CDP/Electron/NIM 只读探测决定单项能力。Contract v2 继续保存旺商聊专有能力与真实写后回读的脱敏证据，不再把每个脚本 SHA 当作全部能力的统一开关。

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

`wangshangliao_capabilities.json` is the production-only evidence registry for
WangShangLiao-specific write behavior. It may only receive a version/hash pair
after a sanitized real trace has passed replay. Fixture versions, URLs and
ports never belong in that file. Runtime ZCG-baseline capabilities are opened
by the live read-only route/IPC/NIM probe, so an unknown hash with the same
protocol structure is not blocked by the hash alone. WangShangLiao-specific
operations such as announcements still require the corresponding evidence and
remain `ManualVerification` until the first real readback succeeds.

`group_gateway_v2.json` freezes the application file version, page title/URL,
main-script SHA-256, request, transport envelope, business envelope, normalized
receipt, callbacks and expected normalized state. The pair remains evidence and
diagnostic provenance; the runtime probe also compares route signatures, IPC,
NIM methods and response envelopes before opening a baseline capability.

`execute_group_batch` is an application-level composition over the same frozen
single-group operations. It accepts only `announcement`, `mute` and `unmute`,
validates each group independently, and preserves selection order in its result.
It does not introduce a generic protocol route. Announcement read/add/update/
broadcast remains subject to dedicated readback evidence. Whole-group mute and
unmute are opened by the live ZCG route/IPC/NIM probe when `MUTE_MEMBER` and
`MUTE_NO` are present and state readback is valid; otherwise the UI shows the
concrete `ManualVerification` or `Unavailable` reason instead of a generic
version lock. A sanitized two-managed-group trace is still required before
enabling WangShangLiao-specific automation.

Raw traces created by `DH-BOT-Dev.exe` belong in
`%APPDATA%\DH\3.0\developer\contracts\raw` and never enter Git or a release bundle.
The developer calibration panel records actual IPC/NIM requests, responses,
callbacks and resulting state while enforcing real mode, `127.0.0.1:9222` and a
ready NIM session. The legacy assembler remains available for manually prepared
inputs and pins the running DevTools page plus installed main script:

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
node scripts/sanitize-contract-capture.mjs contracts/raw/TRACE.json contracts/TRACE.sanitized.json --compact
pnpm verify:calibration-capture -- contracts/TRACE.sanitized.json
node scripts/sanitize-contract-capture.mjs --self-test
node --test scripts/sanitize-contract-capture.test.mjs scripts/assemble-contract-capture.test.mjs scripts/verify-calibration-capture.test.mjs scripts/merge-calibration-capabilities.test.mjs
```

The sanitizer sorts object keys, assigns stable
`ACCOUNT/GROUP/USER/NIM/MESSAGE/NOTICE` and display-name placeholders, and stops
when it finds API keys, authorization values, cookies, passwords, tokens or
private keys. NIM `target`, nested JSON protocol identifiers and announcement
`noticeId`/`id` values are sanitized using their route and object context. A
generic business `id` outside a known protocol context is preserved so normal
business fields are not silently rewritten. URL and session values, UUIDs,
group accounts, member names, avatar identifiers, notification UID/tag values
and opaque Base64 protocol bodies are also replaced. `--compact` retains only
the required group/member preflight, requested writes, their following
readbacks and the callbacks required by the requested capabilities.

The strict verifier treats repeated operations on the same route as separate
ordered evidence. Every operation must include a non-empty request ID, a
successful transport envelope, a successful business envelope, a consistent
normalized receipt and a non-empty expected normalized state. `sendText`,
`recall`, `mute`, `rename`, `announcement` and `groupMute` each require complete
evidence from two distinct groups. Restorable operations must include the
operation-before baseline, the temporary state, the restore operation and an
equal final readback for both groups. `memberEvents` is independent: it becomes
`supported` only when `teamMemberJoined`, `teamMemberUpdated` and
`teamMemberLeft` were all observed in one group. An incomplete passive event
capture remains `unverified` without invalidating other complete capabilities.

Verification results can be merged only into an already reviewed exact
`(appFileVersion, mainScriptSha256)` registry entry:

```text
node scripts/verify-calibration-capture.mjs contracts/GROUP-A.sanitized.json > contracts/raw/GROUP-A.verified.json
node scripts/verify-calibration-capture.mjs contracts/GROUP-B.sanitized.json > contracts/raw/GROUP-B.verified.json
node scripts/merge-calibration-capabilities.mjs \
  contracts/wangshangliao_capabilities.json \
  contracts/raw/GROUP-A.verified.json \
  contracts/raw/GROUP-B.verified.json \
  --output contracts/raw/wangshangliao_capabilities.merged.json
```

The merge is additive for `supported` capabilities and never inserts an unknown
version or script hash. Add a new exact registry identity only through explicit
review, then rerun the merge. `removeMember` is always emitted as `unsupported`.
Review the sanitized capture and merged registry before replacing frozen files;
raw traces and intermediate verification output must remain local.

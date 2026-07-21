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

The fixture files are `group_gateway_v1.json`, `ai_provider_v1.json`,
`prediction_v1.json` and `schedule_v1.json`. Rust tests validate their version
and shape without opening the Go runtime; protocol replay against a real
旺商聊 session remains an alpha acceptance step.

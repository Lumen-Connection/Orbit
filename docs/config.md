# Allowlist and spend caps

`.orbit/config.toml` is created with defaults. You can edit it by hand.

```toml
[commands]
allowed = [
  { program = "cargo", args = ["test"] },
]

[context]
recent_decisions = 10
token_cap = 4000

[budget]
session_usd = 2.0
```

## Commands

Approving a command once stores **program + argument prefix**.

- Allowed `cargo` + `["test"]` also matches `cargo test --lib`.
- It does **not** match `cargo run --bin x`.

The absolute denylist cannot be overridden by this file or by clicking Allow.

## Budget

`session_usd` is the default cap for each Coder session (US dollars, estimated
from the model catalog). At 80% the meter warns. At 100% the agent stops and
asks to raise the cap.

Estimates use OpenRouter `/models` prices. Treat them as a guardrail, not an
invoice.

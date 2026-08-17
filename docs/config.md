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

[[hooks]]
event = "PreToolUse"
matcher = "write_file|edit_file|multi_edit"
command = "python"
args = ["scripts/guard.py"]
```

## Commands

Approving a command once stores **program + argument prefix**.

- Allowed `cargo` + `["test"]` also matches `cargo test --lib`.
- It does **not** match `cargo run --bin x`.

The absolute denylist cannot be overridden by this file or by clicking Allow.

## Hooks

`[[hooks]]` entries run a local command around a tool call. `matcher` is a
regex on the tool name. `PreToolUse` may deny the call; `PostToolUse` only
observes. A cloned repo cannot run a hook on this machine until you trust it
in Settings. Editing the command or args invalidates that trust.

Hooks are **not** a security boundary. A hook that crashes, times out (10s),
or prints unreadable JSON is treated as allow, with a warning. The role
matrix and human approval still run first.

## Budget

`session_usd` is the default cap for each Coder session (US dollars, estimated
from the model catalog). At 80% the meter warns. At 100% the agent stops and
asks to raise the cap.

Estimates use OpenRouter `/models` prices. Treat them as a guardrail, not an
invoice.

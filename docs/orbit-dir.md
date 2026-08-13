# `.orbit/` format

Created in the project root the first time Coder Mode opens the folder.
Hand-editing is supported. Corrupt files degrade to a warning; they never
crash the app.

```
.orbit/
├── context.md
├── decisions.md
├── findings.md
├── tasks.md
├── sessions.json
└── config.toml
```

## context.md

Free-form markdown. Injected into every agent turn (trimmed to the digest
token cap). Describe the goal, stack, and hard constraints.

## decisions.md

Append-only. Agents write via `record_decision`. A block looks like:

```markdown
## 2026-08-12T14:32:11Z — claude-opus-5 (session "architecture")
**Decision:** Use JWT with refresh tokens.
**Rationale:** Stateless API sessions.
**Files:** src/auth/token.rs
```

Portuguese headings (`Decisão`, `Motivo`, `Arquivos`, `sessão`) also parse.

## findings.md

Same heading shape, with `**Finding:**` / `**Severity:**` / `**Location:**`.

## tasks.md

```
- [ ] `t1` Open tasks
- [/] `t2` In progress
- [x] `t3` Done
```

## sessions.json

Index of sessions: id, label, model, `last_active_at`, and files each session
touched. Used for the digest handoff block.

## config.toml

See [config.md](config.md).

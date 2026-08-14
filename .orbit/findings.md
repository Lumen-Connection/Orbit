# Findings

Append-only log of problems and observations.

## 2026-08-13T15:21:15.133195300+00:00 — deepseek/deepseek-v4-flash-0731 (session "Orbit")
**Finding:** R5 conflict: migrations/0001_init.sql already declares `role TEXT` (nullable) on the session table. The spec's `ALTER TABLE session ADD COLUMN role TEXT NOT NULL DEFAULT 'coder'` would fail on a fresh DB because the column already exists (SQLite ADD COLUMN errors on duplicate). Solution: keep the existing column; migration 0005 must NOT re-ADD it. Instead 0005 is a backfill `UPDATE session SET role='coder' WHERE role IS NULL OR role=''`, safe/idempotent on both fresh and v4 databases. Persistence reads/writes the existing `role` column; NULL/corrupt degrades to Coder via from_id.
**Severity:** medium

## 2026-08-13T18:02:02.070584700+00:00 — deepseek/deepseek-v4-flash-0731 (session "Orbit")
**Role:** Reviewer
**Finding:** [R7 review] Tool enforcement is catalog-level only: the per-role registry built by `ToolRegistry::for_role` narrows `schemas()` sent to the model, but `dispatch_tool` (src/session/agent_loop.rs:255) resolves via `deps.registry.get(name)` and has no role check. This is exactly the spec's intent (a Reviewer cannot see write tools), but any path that builds a full registry (tests/tools use `ToolRegistry::workspace_tools()`, and `reset_agent_session` at src/coder.rs:1612 reassigns `registry = workspace_tools()` on every project open, before the per-role override at the TurnDeps construction) means the boundary is purely cryptographic-trust-in-the-model. A reviewer session whose turn somehow dispatched a mutating tool would execute it. This is acceptable per spec §“O ponto que importa” but worth documenting as an acceptance/security note.
**Severity:** info

## 2026-08-13T18:02:02.072327500+00:00 — deepseek/deepseek-v4-flash-0731 (session "Orbit")
**Role:** Reviewer
**Finding:** [R7 review] `src/coder.rs` `reset_agent_session` always builds `registry: Arc::new(ToolRegistry::workspace_tools())` (line ~1612); the per-role registry is only built fresh inside the two TurnDeps constructors (`ToolRegistry::for_role(session_role)`). On a project reopen the shared `state.coder.registry` is the full 14-tool set, but since each turn rebuilds by role this is harmless to the model. However it is redundant and a latent trap: any future code path that reads `state.coder.registry` directly would expose all 14 tools regardless of role. Suggest removing the shared registry or documenting that it must never be used for execution.
**Severity:** low

## 2026-08-13T18:12:14.731069700+00:00 — z-ai/glm-5.2 (session "Session 2")
**Role:** Reviewer
**Finding:** [R7 review] `set_coder_role` (src/coder.rs:1570) early-returns if `live.busy`, meaning a role switch is silently ignored while a turn is running. The spec says "prompt and permissões valem dali em diante", which is satisfied by the next-turn rebuild (registry is rebuilt per-turn from `live.role` via `ToolRegistry::for_role(session_role)` at lines 933 and 1224). The block is defensively reasonable — it avoids a mid-turn inconsistency where the running TurnDeps already captured the old role — but it is a silent no-op with no user feedback. The UI combo box will appear to "accept" the choice and then not take effect until the turn finishes. Minor UX defect, not a functional bug.
**Severity:** minor
**Location:** src/coder.rs:1570

## 2026-08-13T18:12:14.732252400+00:00 — z-ai/glm-5.2 (session "Session 2")
**Role:** Reviewer
**Finding:** [R7 review — latent trap, already noted] `reset_agent_session` (src/coder.rs:1643) reassigns `state.coder.registry = Arc::new(ToolRegistry::workspace_tools())` (the full 14-tool set) on every project open/reopen. This shared field is never used for actual tool dispatch — both TurnDeps constructors at lines 933 and 1224 build a fresh `ToolRegistry::for_role(session_role)`. The shared registry is dead weight and a latent trap: any future code path that reads `state.coder.registry` directly would bypass the role boundary entirely. It should either be removed or documented as "must never be used for execution". Confirmed harmless today by tracing all dispatch paths through `deps.registry.get(...)` in `dispatch_tool` (src/session/agent_loop.rs:255).
**Severity:** low
**Location:** src/coder.rs:1643

## 2026-08-13T20:58:16.881169300+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Finding:** Executed a third batch from PlanejamentoMelhoria.md, keeping the tríade green (fmt ✓, clippy -D warnings ✓, 182/182 tests ✓). Completed: N0.1 dispatch-level role guard (refuses write for Reviewer even with full registry; only enforced for canonical workspace tools so custom test tools pass); N0.2 removed dead state.coder.registry latent trap; N0.4 surfaced OpenRouter cached_tokens end-to-end into LiveSession + cost meter (prep for N0.5); N1.1 multi_edit tool (atomic batch edits, single patch/approval); N1.2 grep context lines; N1.6 read_file binary detection; N2.4 digest trim no longer cuts mid-entry; N2.5 edit_file anchor_line. Coder/Tester now allow 15 tools (multi_edit added), Reviewer 12, Architect 7· unchanged.
**Severity:** info
**Location:** src/session/agent_loop.rs, src/coder.rs, src/tools/fs.rs, src/tools/mod.rs, src/session/roles.rs, src/context/digest.rs, src/providers/openrouter.rs, src/session/{mod,manager}.rs, src/ui/widgets/cost_meter.rs

## 2026-08-13T22:06:32.835042800+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Finding:** Executed a fourth batch from PlanejamentoMelhoria.md, tríade green (fmt ✓, clippy -D warnings ✓, 185/185 tests ✓, up from 182). Completed: N1.4 configurable summary_model in [context] of config.toml wired into summarize_middle via a new TurnDeps.summary_model field (falls back to session model; summary usage still recorded with the actual model under kind=summary); N1.5 run_command now interleaves stdout/stderr in actual arrival order via a single tokio select loop (removed the two racy pump_reader tasks), with a cross-platform OUT1/ERR1/OUT2 order test; N2.1 Ctrl+Shift+M toggles Chat/Coder (Table covers it, settings screen auto-lists it); N2.6 cost_meter occupancy label now switches to the warning palette at >= 80% context occupancy. All TurnDeps test constructors updated with the new summary_model field.
**Severity:** info
**Location:** src/context/store.rs, src/session/agent_loop.rs, src/coder.rs, src/tools/shell.rs, src/ui/shortcuts.rs, src/ui/mod.rs, src/ui/widgets/cost_meter.rs, src/session/manager.rs, src/e2e.rs

## 2026-08-13T22:29:43.223927900+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Finding:** Executed a fifth batch from PlanejamentoMelhoria.md: N0.3 — the Coder Mode role ComboBox is now disabled during a busy turn with an explanatory hover ("Role switching takes effect on the next turn when this session is not busy."), replacing the prior silent ignore in set_coder_role. Tríade remains green (fmt ✓, clippy -D warnings ✓, 185/185 tests ✓).
**Severity:** info
**Location:** src/ui/coder/sessions.rs

## 2026-08-14T03:06:25.288261500+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Finding:** Executed a sixth batch from PlanejamentoMelhoria.md: N1.3 — Coder Mode session search reusing the existing history_fts index. A search field in the Coder Mode sessions header calls db.search_history, filters to source=="session", and clicking a hit opens that session via select_coder_session. Backed by a new CoderState.coder_search string. Tríade green (fmt ✓, clippy -D warnings ✓, 185/185 tests ✓).
**Severity:** info
**Location:** src/ui/coder/sessions.rs, src/coder.rs

## 2026-08-14T03:50:57.047083200+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Finding:** Executed a seventh batch from PlanejamentoMelhoria.md, tríade green (fmt ✓, clippy -D warnings ✓, 190/190 tests ✓, up from 187). Completed: N0.5 explicit prompt caching (ChatRequest.system_cache_chars touches the stable base+role prefix and encode_messages emits it with cache_control {type:"ephemeral"}, leaving the per-turn digest uncached; unit test asserts the marker); N2.3 glob/grep file-walk cache (250ms TTL, single traversal reused across same-turn calls, new files visible after the TTL<approval round-trip); and the N3.2 foundational slice — a new src/pipeline module with StageResult + PipelineEvent::StageFinished and a tested TurnResult→StageResult mapping (the completion signal an orchestrator needs).
**Severity:** info
**Location:** src/providers/{mod,openrouter}.rs, src/session/agent_loop.rs, src/tools/fs.rs, src/pipeline/mod.rs, src/main.rs

## 2026-08-14T03:59:49.006580700+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Finding:** Created PlanejamentoMelhoriaPENDENTES.md documenting precisely what remains. It records that the P0/P1/P2 tracks are fully complete (N0.1–N0.4, N1.1–N1.6, N2.1–N2.6, with N0.5 done except a pending wiremock test of the cache marker). All remaining work is the N3 pipeline track (G-scope): N3.1 config dialog, N3.2 spawner wiring for the completion signal, N3.3 reviewer loop + approve_stage, N3.4 Planner read-only, N3.5 Git Gate tools, N3.6 orchestrator, N3.7 contract/AC, N3.8 deterministic verification, N3.9 complexity routing, N3.10 Auto model. Included dependency order and recommended next steps.
**Severity:** info
**Location:** PlanejamentoMelhoriaPENDENTES.md

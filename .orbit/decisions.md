# Decisions

Append-only log. Agents and humans can edit this file.

## 2026-08-13T15:21:15.134581800+00:00 — deepseek/deepseek-v4-flash-0731 (session "Orbit")
**Decision:** Instantiate “Papéis de Agente” (R1–R7) per melhoriaparte2.md. The session `role` column already exists in migration 0001, so migration 0005 is a data backfill (UPDATE role='coder' WHERE role IS NULL/empty), not an ADD COLUMN, to avoid breaking fresh DBs. All identifiers/comments stay in English.
**Rationale:** Spec is permissive enough to adapt where the repo diverges; invariant rules forbid renaming public APIs and refactoring beyond scope.
**Files:** src/session/roles.rs, src/session/mod.rs, src/tools/mod.rs, src/storage/db.rs

## 2026-08-13T17:57:23.850830600+00:00 — deepseek/deepseek-v4-flash-0731 (session "Orbit")
**Decision:** All seven tasks from melhoriaparte2.md (R1–R7 “Papéis de Agente”) are implemented and verified. Verification tríade is green: cargo fmt --all --check, cargo clippy --all-targets -- -D warnings, cargo test --all-targets (173 passed). Session::new keeps its two-arg signature; migration 0005 is the role backfill because the column already exists in 0001.
**Rationale:** Acceptance criteria in melhoriaparte2.md are met; role is enforced via the tool catalog (schemas) sent to the model, not via negative instruction; invalid/corrupt role data degrades to Coder.
**Files:** src/session/roles.rs, src/session/mod.rs, src/session/agent_loop.rs, src/session/manager.rs, src/tools/mod.rs, src/tools/context.rs, src/storage/db.rs, migrations/0005_session_role.sql, src/context/store.rs, src/coder.rs, src/ui/coder/sessions.rs

## 2026-08-13T18:16:04.596854400+00:00 — z-ai/glm-5.2 (session "Session 2")
**Role:** Reviewer
**Decision:** Created PlanejamentoMelhoria.md as a third-round improvement backlog, grounded in a line-by-line review of the current codebase. It is organized in three tiers (P0 blockers/security, P1 high-ROI, P2 polish) and each task cites concrete file:line locations. The highest-impact items are: prompt caching (cost), multi_edit tool (latency), role enforcement at dispatch (security), and Coder Mode session search (UX).
**Rationale:** The existing planning docs (Tasks.md, Melhorias.md, melhoriaparte2.md) are largely completed — the tríade passes with 173 tests and the feature set from R1–R7 is in place. A new improvement plan must build on the actual current state, not on the old diagnosis. Every proposal is validated against the code I inspected.
**Files:** PlanejamentoMelhoria.md

## 2026-08-13T19:48:48.891040500+00:00 — z-ai/glm-5.2 (session "Session 2")
**Role:** Reviewer
**Decision:** PlanejamentoMelhoria.md atualizado para incorporar as 12 recomendações do Feedback_PlanejamentoMelhoria.md. Mudanças aplicadas: (1) terminologia "gratuito"→"baixo overhead" em 3 pontos; (2) UI reformulada de 4 combos (incluindo Submitter) para 3 combos de inteligência + seletor de Git Gate; (3) Submitter renomeado para Git Gate em toda a doc; (4) N3.6 agora declara dependência explícita de N3.7 (contrato) e N3.8 (verificação); (5) adicionadas 4 novas seções: N3.7 (Pipeline Context Contract + Acceptance Criteria, pré-requisito do orquestrador), N3.8 (verificação determinística fmt/clippy/test antes do Reviewer), N3.9 (routing por complexidade, versão manual), N3.10 (opção Auto de modelo, com ressalva de que exige enriquecer o catálogo); (6) resumo e escopo atualizados. A tese central e os dois modos (Concorrente/Orquestrador) permanecem intactos.
**Rationale:** O feedback era consistente e correto em todos os 12 pontos. As recomendações obrigatórias antes do orquestrador (contrato formal, ACs, verificação determinística, stage artifacts) transformam a pipeline de sequência de sessões dependentes de digest implícito em linha de produção verificável. As ressalvas (routing manual nesta rodada, Auto depende de enriquecer catálogo) foram incorporadas como notas de escopo.
**Files:** PlanejamentoMelhoria.md

## 2026-08-13T20:01:02.172503500+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Decision:** Implement N0.1 from PlanejamentoMelhoria.md: dispatch-level role enforcement. Added a guard in dispatch_tool (src/session/agent_loop.rs) that checks deps.session_role.allowed_tools().contains(&call.name) before executing, returning a clear ToolResult error if the role is not permitted. This is independent of the catalog filter and closes the latent trap where a Reviewer whose turn carried a full registry could execute a write tool. Added two tests proving a Reviewer with a full workspace registry is refused write_file at dispatch but allowed read_file.
**Rationale:** Highest-value security item in the backlog; closes the boundary gap that was only catalog-level.
**Files:** src/session/agent_loop.rs

## 2026-08-13T20:04:13.427109400+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Decision:** Implement N0.2 from PlanejamentoMelhoria.md: remove the dead `state.coder.registry` latent trap. Deleted the `registry: Arc<ToolRegistry>` field from CoderState, its initialization in the default constructor, and the reassignment in reset_agent_session (src/coder.rs). TurnDeps already builds a fresh per-role registry each turn; the shared field was never used for dispatch and only risked a future path bypassing the role boundary. grep confirms no remaining CoderState.registry reads.
**Rationale:** Removes the documented latent trap where a future reader of state.coder.registry would bypass role enforcement.
**Files:** src/coder.rs

## 2026-08-13T20:13:03.681321300+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Decision:** Implement N1.6 from PlanejamentoMelhoria.md: read_file binary detection. Changed read_file to read bytes, check for a NUL byte in the first 8KB, and return a clear "binary file, N bytes" message instead of a confusing UTF-8 error. Non-binary files still read as text with offset/limit. Added a test proving a PNG returns the binary message while .rs files still read normally.
**Rationale:** Small, self-contained UX fix replacing a confusing error with a clear binary-file message.
**Files:** src/tools/fs.rs

## 2026-08-13T20:13:03.682686800+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Decision:** Implement N1.2 from PlanejamentoMelhoria.md: grep context lines. Added an optional `context` (integer, default 0) param to the Grep schema and execution. When set, it prints N surrounding lines before and after each match, context lines marked with `-` (like grep -C), still bounded by GREP_LIMIT=50 and truncate_output. Behavior unchanged without the param. Added a test proving context: 2 shows surrounding lines.
**Rationale:** Reduces the extra read_file turn the agent needs to see surrounding context, cutting latency.
**Files:** src/tools/fs.rs

## 2026-08-13T20:27:33.359206400+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Decision:** Implement N1.1 from PlanejamentoMelhoria.md: multi_edit tool for batch edits. Added a MultiEdit tool (src/tools/fs.rs) accepting a path and an array of {old_string, new_string} pairs. All replacements are applied sequentially to the original content; the operation fails atomically (produces no patch) if any snippet is missing or ambiguous, yielding a single FilePatch and a single approval. Registered in workspace_tools (src/tools/mod.rs) and added to Coder/Tester allowed_tools in roles.rs (now 15 tools); Reviewer and Architect still exclude it. Added tests proving 3 edits become one patch and ambiguity fails without applying anything.
**Rationale:** Highest-latency ROI item: collapses multiple edit calls/approvals/model turns into one.
**Files:** src/tools/fs.rs, src/tools/mod.rs, src/session/roles.rs

## 2026-08-13T20:30:56.997865400+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Decision:** Implement N2.5 from PlanejamentoMelhoria.md: edit_file anchor_line. Added an optional 1-based `anchor_line` param to the EditFile schema and execution. When present, the search is narrowed to a byte window around that line (ANCHOR_WINDOW=20 lines each side), so uniqueness is only required within the window and only the in-window occurrence is replaced. Without the param, behavior is unchanged (whole-file uniqueness). Added tests proving it disambiguates a repeated snippet near the anchor and that no-anchor still rejects ambiguity.
**Rationale:** Saves tokens the agent would otherwise spend including large context to disambiguate repeated snippets.
**Files:** src/tools/fs.rs

## 2026-08-13T20:41:56.604734200+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Decision:** Implement N2.4 from PlanejamentoMelhoria.md: digest trim_to_cap no longer cuts mid-entry. trim_to_cap now keeps text up to the last newline before the cutoff so a decision or finding is never split mid-line; falls back to the raw char cutoff when no earlier newline exists. Added a test proving the trimmed digest ends with a complete line before the truncation marker.
**Rationale:** Improves digest coherence the agent and users see; trivial change.
**Files:** src/context/digest.rs

## 2026-08-13T20:56:43.793874900+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Decision:** Implement N0.4 from PlanejamentoMelhoria.md: surface cached_tokens from OpenRouter. Added cache deserialization: StreamUsage now parses prompt_tokens_details.cached_tokens (and top-level cached_tokens) and propagates it into TokenUsage.cached_tokens. The value flows through AgentEvent::Usage (new cached_tokens field) into LiveSession.cached_tokens and is displayed in the cost meter (N2.2) as "cached N". Cost calculation unchanged. Updated the wiremock recorded stream to include prompt_tokens_details and assert cached_tokens=2.
**Rationale:** Makes prompt-cache savings visible instead of silently paying full price; foundation for N0.5 explicit caching.
**Files:** src/providers/openrouter.rs, src/session/mod.rs, src/session/agent_loop.rs, src/session/manager.rs, src/ui/widgets/cost_meter.rs

## 2026-08-13T21:44:33.321306900+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Decision:** Implement N1.5 from PlanejamentoMelhoria.md: preserve stdout/stderr arrival order in run_command. Replaced the two independent pump_reader tasks writing to a shared sink with a single tokio select loop that polls both pipes and emits chunks to the terminal + sink in actual arrival order (fixes scrambled build diagnostics). EOF on a pipe drops it; loop ends when both close. Removed the now-unused pump_reader. Added a cross-platform test that interleaves OUT1/ERR1/OUT2 and asserts the report preserves that order.
**Rationale:** Fixes the concrete correctness defect where concurrent pipe tasks could not preserve interleaving order of stdout/stderr diagnostics.
**Files:** src/tools/shell.rs

## 2026-08-13T21:44:33.322685100+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Decision:** Implement N2.1 from PlanejamentoMelhoria.md: Ctrl+Shift+M toggles Chat/Coder mode. Added ShortcutId::ToggleMode to the central shortcut table (keys "Ctrl+Shift+M", while_typing: false) and wired it in ui/mod.rs dispatch to flip state.mode between AppMode::Chat and AppMode::Coder. The settings shortcuts screen already iterates SHORTCUTS so the new binding appears automatically; updated the covering test.
**Rationale:** Small UX win: keyboard-operated users can switch modes without reaching for the mouse.
**Files:** src/ui/shortcuts.rs, src/ui/mod.rs

## 2026-08-13T21:46:21.768447900+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Decision:** Implement N2.6 from PlanejamentoMelhoria.md: visible context occupancy. The cost_meter in the Coder Mode header already surfaced occupancy_label when context_occupancy is set; enhanced it so the color switches to the warning palette at >= 80% occupancy, making the approaching-context-limit state visible at a glance next to the cost meter. Updates every turn because pack_messages emits AgentEvent::ContextOccupancy each iteration into LiveSession.context_occupancy.
**Rationale:** Cheap clarity win: the user can now see they're near the context limit without computing it themselves.
**Files:** src/ui/widgets/cost_meter.rs

## 2026-08-13T22:04:59.609945300+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Decision:** Implement N1.4 from PlanejamentoMelhoria.md: configurable cheaper summary model. Added optional `summary_model` to the [context] section of .orbit/config.toml, parsed into DigestSettings.summary_model. Added a `summary_model: Option<String>` field to TurnDeps, populated from the orbit store in both coder.rs constructors, and used in summarize_middle (falls back to the session model when absent). The summary usage is still recorded with the actual model used under kind=summary. Added tests proving the config parses and defaults to None.
**Rationale:** Lets users run expensive reasoning sessions without paying reasoning-model prices for the context-window summarization pass.
**Files:** src/context/store.rs, src/session/agent_loop.rs, src/coder.rs

## 2026-08-13T22:19:09.473293700+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Decision:** Implement N0.3 from PlanejamentoMelhoria.md: feedback when switching role during an active turn. The Coder Mode role ComboBox is now disabled while the session is busy, with an explanatory hover ("Role switching takes effect on the next turn when this session is not busy."). Previously set_coder_role silently ignored the switch during busy; now the UI is honest that the change applies next turn. When the turn finishes, the combo re-enables and the switch applies immediately.
**Rationale:** Removes the silent no-op UX defect: user is told explicitly why the role combo is locked, and it re-enables when the turn ends.
**Files:** src/ui/coder/sessions.rs

## 2026-08-14T03:06:13.412136400+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Decision:** Implement N1.3 from PlanejamentoMelhoria.md: Coder Mode session search reusing FTS. Added a search text field in the Coder Mode sessions header wired to db.search_history, filtered to source=="session" hits. Clicking a hit selects and opens that session via select_coder_session. Added a CoderState.coder_search field (default empty). Reuses the existing FTS index (history_fts), so searching by content of a past session works with no new infra.
**Rationale:** The doc's "Maior ganho de UX" — lets users find an old Coder conversation by content instead of guessing which tab, reusing the FTS infra that already exists for Chat.
**Files:** src/ui/coder/sessions.rs, src/coder.rs

## 2026-08-14T03:20:02.686787500+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Decision:** Implement N0.5 from PlanejamentoMelhoria.md: explicit prompt caching via cache marker. Added `system_cache_chars` to ChatRequest (byte length of the stable system prefix). In agent_loop, system_cache_prefix_chars returns the base-prompt+role-fragment byte length (0 when no store/digest). encode_messages now splits the system into a stable prefix (emitted with cache_control {type:"ephemeral"}, supported by Anthropic via OpenRouter) and the trailing per-turn digest (left uncached). Added a wiremock-agnostic unit test proving the cache marker is attached to the stable prefix and the digest is not.
**Rationale:** The doc's "Maior ganho de custo": lets prompt-caching-capable models reuse the stable prefix instead of re-billing full price every turn.
**Files:** src/providers/mod.rs, src/session/agent_loop.rs, src/providers/openrouter.rs

## 2026-08-14T03:26:08.164330100+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Decision:** Implement N2.3 from PlanejamentoMelhoria.md: glob/grep file walk cache. Added a short-TTL (250ms) single-entry cache of the walked file list keyed by the canonical project root, shared across consecutive Grep/Glob calls in the same turn. A human approval round-trip is far slower than the TTL, so a newly-written file is always visible to the next grep/glob (no staleness), while repeated same-turn calls reuse one traversal. Added a test proving cache reuse and that a new file is found after the TTL expires.
**Rationale:** Avoids re-walking the tree on every grep/glob in a large project, cutting latency for repeated searches without risking stale results after a write.
**Files:** src/tools/fs.rs

## 2026-08-14T03:50:41.630056500+00:00 — deepseek/deepseek-v4-flash-0731 (session "Session 3")
**Role:** Coder
**Decision:** Implement the foundational slice of N3.2 from PlanejamentoMelhoria.md: stage-completion signal. Created a new src/pipeline module holding StageResult and PipelineEvent::StageFinished { session_id, result }, with a From<TurnResult> mapping and a stage_finished constructor — the sole completion signal an orchestrator (N3.6) needs to chain stages, reusing the existing run_turn result instead of inventing a second channel. Marked #![allow(dead_code)] until the spawner wiring in coder.rs and the orchestrator land as the G-scope item N3.6. Three unit tests prove the TurnResult→StageResult mapping (Completed/Failed/Cancelled).
**Rationale:** N3 is a G-scope multi-stage feature; this is the surgical, tested foundation (the completion signal) that unblocks the rest, keeping the tríade green while the larger wiring is a separate task.
**Files:** src/pipeline/mod.rs, src/main.rs

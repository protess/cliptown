# TODOS

## Open

_(empty)_

## Completed

### M12 P2.1 daemon health buckets — 2026-05-12
**Source:** Phase 2 backlog first item (from `docs/superpowers/specs/2026-05-09-real-llm-e2e-design.md` § P2.1). PR `<TBD — fill in at PR creation>`.

Was: cliptown's operator console treated worker liveness as binary (WS connected vs closed). A 5-minute network blip looked identical to a hard crash, generating noise.

Fixed: `AvatarView` now carries `last_seen_at: Option<i64>` (updated on RegisterWorker / HandleWorkerMsg) and `health: Health` (derived per tick from connection state + age of last_seen). 4 states — `online` / `recently_lost` / `offline` / `about_to_gc` — replace the binary signal. New `crates/world/src/health.rs` pure module owns derivation + thresholds. Frontend `AvatarVM` mirrors the shape; Pixi alpha dims non-online avatars. 11 new tests (8 unit + 3 integration).

### M11 real bench harness — 2026-05-12
**Source:** Phase 1 known-limitation cleanup. PR `#37`.

Was: `crates/world/benches/world_bench.rs` shipped Phase 0 with two placeholder benches — `tick_latency_per_loop_iter` ran `sum 0..1000` inside a tokio runtime; `mpsc_throughput_1k_msgs` measured a generic in-process mpsc channel. Neither touched real world code.

Fixed: both benches now drive a real `loop_::spawn` handle. `tick_latency_real_loop` measures one `Cmd::Tick` round-trip end to end; `console_dispatch_throughput_100_msgs` fires 100 `Cmd::HandleConsoleMsg` through the same dispatcher lane real console commands use. `bench/check.mjs` learned the `100_div_median_us` extract recipe; `bench/baselines.json` carries fresh medians captured on the dev box. The Phase-1 known-limitations bullet about placeholder benches retires.

### M11 hook bridge parity — codex + opencode — 2026-05-12
**Source:** Phase 0 known-limitation (`#31` follow-up). PR `#36`.

Was: codex + opencode adapters advertised `[session_stop, session_error]` capabilities but no hook events actually flowed. Each spun up an HTTP `startHookBridge` listener and exposed it via `CODEX_HOOK_PORT` / `OPENCODE_HOOK_PORT`, but nothing on the upstream CLI side ever POSTed to it — dead weight.

Fixed: codex now drives `opts.onHook` from a streaming JSONL parser over `codex exec --json` stdout (`event_parser.ts`); opencode was rebuilt around `opencode serve --port 0 --pure` + `/event` SSE so we observe `pending → running → completed` state transitions for true pre/post semantics (`event_mapper.ts` + `sse_client.ts` + `serve_lifecycle.ts` + `session_client.ts`). Dead HTTP bridge removed from both adapters; `adapter-core/hook_bridge.ts` kept for claude-code. Three smoke-discovered fixes shipped in the same PR: claude CLI 2.1.x needs `--settings <path>` (env var ignored), `opencode serve` emits listening URL on stdout not stderr, and the shared bridge now reads `tool_name` first (claude payload shape) before `tool`. Capability advertising on all three adapters now matches reality. § 11.9 smoke verified named-tool hook lines on each backend (claude `Write` / `mcp__cliptown__task_done`, codex `shell` / `task_done`, opencode `apply_patch` / `cliptown_task_done`).

### Body-length validation on chat/directive (P2) — 2026-05-11
**Source:** Codex adversarial review on M5 ship (P2 #1)

Was: workers could send unbounded `body` via `speak`, managers could send unbounded `feedback` via `task_request_changes`, and operators could send unbounded `body` via `OperatorDirective`. Each cloned the full string into the broadcast channel (capacity 4096, Lagged-fatal-close), the SQL `messages` row, and the frontend's 500-entry messages array — a chatty / malicious agent or operator could starve the operator console by pushing real events out of the buffer.

Fixed: `mcp_dispatch::MAX_BODY_LENGTH = 4096` (chars) + `check_body_length` helper guarding the three producer call sites pre-side-effect. Workers see `mcp_error{code:"body_too_long"}`; operators see `error{reason:"body_too_long"}`. Regression guards: `speak_rejects_body_too_long`, `speak_accepts_body_at_cap`, `task_request_changes_rejects_feedback_too_long`, `no_broadcast_on_body_too_long`.

### `emit_system_event` silent JSON fallback on malformed payload (P3) — 2026-05-11
**Source:** Codex adversarial review on M5 ship

Was: `emit_system_event` wrote the raw payload string to SQL but used `serde_json::from_str(payload).unwrap_or(Value::Null)` for the broadcast frame. SQL row had the raw string, broadcast frame had `Value::Null` — operator console and audit log diverged on malformed input.

Fixed in `crates/world/src/emit.rs`: parse via `match` and log `tracing::error!` on failure, then send the raw string as `Value::String(raw)` on the wire so SQL and broadcast carry identical data. Loud-fail surfaces the producer bug to operators instead of silent null-degradation. Regression guard: `console_emit::emit_system_event_malformed_payload_preserves_raw_on_broadcast`.

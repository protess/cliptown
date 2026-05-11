# TODOS

## Open

_(empty)_

## Completed

### Body-length validation on chat/directive (P2) — 2026-05-11
**Source:** Codex adversarial review on M5 ship (P2 #1)

Was: workers could send unbounded `body` via `speak`, managers could send unbounded `feedback` via `task_request_changes`, and operators could send unbounded `body` via `OperatorDirective`. Each cloned the full string into the broadcast channel (capacity 4096, Lagged-fatal-close), the SQL `messages` row, and the frontend's 500-entry messages array — a chatty / malicious agent or operator could starve the operator console by pushing real events out of the buffer.

Fixed: `mcp_dispatch::MAX_BODY_LENGTH = 4096` (chars) + `check_body_length` helper guarding the three producer call sites pre-side-effect. Workers see `mcp_error{code:"body_too_long"}`; operators see `error{reason:"body_too_long"}`. Regression guards: `speak_rejects_body_too_long`, `speak_accepts_body_at_cap`, `task_request_changes_rejects_feedback_too_long`, `no_broadcast_on_body_too_long`.

### `emit_system_event` silent JSON fallback on malformed payload (P3) — 2026-05-11
**Source:** Codex adversarial review on M5 ship

Was: `emit_system_event` wrote the raw payload string to SQL but used `serde_json::from_str(payload).unwrap_or(Value::Null)` for the broadcast frame. SQL row had the raw string, broadcast frame had `Value::Null` — operator console and audit log diverged on malformed input.

Fixed in `crates/world/src/emit.rs`: parse via `match` and log `tracing::error!` on failure, then send the raw string as `Value::String(raw)` on the wire so SQL and broadcast carry identical data. Loud-fail surfaces the producer bug to operators instead of silent null-degradation. Regression guard: `console_emit::emit_system_event_malformed_payload_preserves_raw_on_broadcast`.

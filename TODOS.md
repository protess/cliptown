# TODOS

## Open

### Chat / Directive (M5 follow-up)

#### Body-length validation on chat/directive
**Priority:** P2
**Source:** Codex adversarial review on M5 ship (P2 #1)

Workers can send unbounded body strings via `speak` (chat or directive) and operators can send unbounded `OperatorDirective` bodies. The new broadcast path now amplifies this: huge bodies get cloned into Chat/Directive frames, broadcast to all operator consoles, and stored in the frontend's 500-entry messages array.

Combined with the broadcast-channel lag-loss path (capacity 4096, Lagged-fatal-close), a chatty/malicious agent can push real events out of the buffer.

Fix: add `MAX_BODY_LENGTH` constant (suggest 4096 chars), validate at the top of `cmd_console::OperatorDirective`, `mcp_dispatch::handle_speak`, and `mcp_dispatch::handle_task_request_changes`. Return `bad_args` / `error{reason:"body_too_long"}` if exceeded.

## Completed

### `emit_system_event` silent JSON fallback on malformed payload (P3) — 2026-05-11
**Source:** Codex adversarial review on M5 ship

Was: `emit_system_event` wrote the raw payload string to SQL but used `serde_json::from_str(payload).unwrap_or(Value::Null)` for the broadcast frame. SQL row had the raw string, broadcast frame had `Value::Null` — operator console and audit log diverged on malformed input.

Fixed in `crates/world/src/emit.rs`: parse via `match` and log `tracing::error!` on failure, then send the raw string as `Value::String(raw)` on the wire so SQL and broadcast carry identical data. Loud-fail surfaces the producer bug to operators instead of silent null-degradation. Regression guard: `console_emit::emit_system_event_malformed_payload_preserves_raw_on_broadcast`.

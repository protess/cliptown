# TODOS

## Open

### Chat / Directive (M5 follow-up)

#### Body-length validation on chat/directive
**Priority:** P2
**Source:** Codex adversarial review on M5 ship (P2 #1)

Workers can send unbounded body strings via `speak` (chat or directive) and operators can send unbounded `OperatorDirective` bodies. The new broadcast path now amplifies this: huge bodies get cloned into Chat/Directive frames, broadcast to all operator consoles, and stored in the frontend's 500-entry messages array.

Combined with the broadcast-channel lag-loss path (capacity 4096, Lagged-fatal-close), a chatty/malicious agent can push real events out of the buffer.

Fix: add `MAX_BODY_LENGTH` constant (suggest 4096 chars), validate at the top of `cmd_console::OperatorDirective`, `mcp_dispatch::handle_speak`, and `mcp_dispatch::handle_task_request_changes`. Return `bad_args` / `error{reason:"body_too_long"}` if exceeded.

#### `emit_system_event` silent JSON fallback on malformed payload
**Priority:** P3
**Source:** Codex adversarial review on M5 ship

`emit_system_event` writes the raw payload string to SQL but uses `serde_json::from_str(payload).unwrap_or(Value::Null)` for the broadcast frame. Malformed JSON → SQL row has the raw string, broadcast frame has `Value::Null`. Operators see null, audit log has different data.

Fix: switch to `match serde_json::from_str(payload)` — log `tracing::error!` if parse fails, then either skip the broadcast OR send with the raw string as a string Value. Either way, fail loud (don't silently degrade).

## Completed

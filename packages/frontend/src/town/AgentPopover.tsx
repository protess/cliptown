/**
 * AgentPopover (M4.11) — small floating dialog anchored at a clicked avatar.
 *
 * Surfaces:
 *   - agent identity (name, role, backend, status, room, tile position)
 *   - current task (assignee + status, if one exists)
 *   - a directive input (visible only when the operator is possessing) that
 *     emits a Phase-0 OperatorDirective frame
 *
 * Behavior:
 *   - Position is anchored at the click coords, edge-clamped to the viewport.
 *   - Click-outside (overlay) and Escape both close the popover.
 *   - Enter inside the input submits a directive and closes.
 */
import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { useWorld } from "../hooks/useWorld.js";
import type { AvatarVM, TaskVM } from "../store.js";

const HUES = [
  "#E63946", "#F4A261", "#E9C46A", "#2A9D8F",
  "#264653", "#A663CC", "#FF8FAB", "#83A4D4",
] as const;
const OPERATOR_AVATAR_ID = "__operator__";

function hueFor(id: string): string {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) | 0;
  return HUES[Math.abs(h) % HUES.length]!;
}

interface AgentPopoverProps {
  agentId: string;
  anchorX: number;
  anchorY: number;
  onClose: () => void;
}

export function AgentPopover({ agentId, anchorX, anchorY, onClose }: AgentPopoverProps) {
  const { state, send } = useWorld();
  const a: AvatarVM | undefined = state.avatars[agentId];
  const [draft, setDraft] = useState("");
  const op = state.avatars[OPERATOR_AVATAR_ID];
  const possessing = !!op;
  // Imperative focus: React's `autoFocus` doesn't reliably move focus when the
  // popover mounts inside a `position: fixed` overlay above a Pixi canvas
  // (the canvas's pointerdown handler asserts focus on the canvas element,
  // and useEffect fires before that finishes). Defer with rAF to let layout +
  // any in-flight focus events settle, then take focus.
  const inputRef = useRef<HTMLInputElement | null>(null);
  useEffect(() => {
    if (!possessing) return;
    const id = requestAnimationFrame(() => {
      inputRef.current?.focus();
    });
    return () => cancelAnimationFrame(id);
  }, [possessing]);

  // Find a current task assigned to this agent (in_progress > queued).
  const currentTask: TaskVM | undefined = Object.values(state.tasks).find(
    (t) =>
      t.assignee_agent_id === agentId &&
      (t.status === "in_progress" || t.status === "queued"),
  );

  // Close on Escape (kept as a local belt-and-suspenders even though the
  // global keymap dispatches `cliptown:dismiss`; the directive input grabs
  // focus, so suppressing global handling there means we still need a local
  // path for the in-input Escape).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    const onDismiss = () => onClose();
    window.addEventListener("keydown", onKey);
    window.addEventListener("cliptown:dismiss", onDismiss);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("cliptown:dismiss", onDismiss);
    };
  }, [onClose]);

  const sendDirective = () => {
    const body = draft.trim();
    if (!body || !possessing) return;
    send({
      type: "operator_directive",
      v: 1,
      to_agent_id: agentId,
      body,
    });
    setDraft("");
    onClose();
  };

  // Edge-clamp position. Use a fixed nominal size for the clamp; the panel's
  // CSS `min-height` is informational.
  const W = 280;
  const H = 220;
  const clampedX = Math.min(Math.max(anchorX, 8), Math.max(8, window.innerWidth - W - 8));
  const clampedY = Math.min(Math.max(anchorY, 8), Math.max(8, window.innerHeight - H - 8));

  if (!a) {
    return (
      <div onClick={onClose} style={overlayStyle}>
        <div
          onClick={(e) => e.stopPropagation()}
          style={{ ...panelStyle, top: clampedY, left: clampedX, width: W }}
          role="dialog"
          aria-label={`Agent ${agentId} (missing)`}
        >
          <p style={{ margin: 0, color: "var(--fg-secondary)", fontSize: 13 }}>
            Agent <code>{agentId}</code> not found.
          </p>
          <button onClick={onClose} style={btnStyle}>Close</button>
        </div>
      </div>
    );
  }

  const hue = hueFor(a.startup_id);
  const mono = (a.agent_id.slice(0, 1) || "?").toUpperCase();

  return (
    <div onClick={onClose} style={overlayStyle}>
      <div
        onClick={(e) => e.stopPropagation()}
        style={{ ...panelStyle, top: clampedY, left: clampedX, width: W, minHeight: H }}
        role="dialog"
        aria-label={`Agent ${a.agent_id}`}
      >
        <header style={headerStyle}>
          <span aria-hidden style={{ ...avatarChipStyle, background: hue }}>
            {mono}
          </span>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontWeight: 600, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              {a.agent_id}
            </div>
            <div style={{ fontSize: 11, color: "var(--fg-secondary)" }}>
              {a.role || "—"} · <code>{a.backend || "—"}</code>
            </div>
          </div>
          <button onClick={onClose} style={iconBtnStyle} aria-label="Close">×</button>
        </header>

        <dl style={dlStyle}>
          <Row label="status">{a.status || "—"}</Row>
          <Row label="room">{a.room_id || "—"}</Row>
          <Row label="position">
            ({a.current_pos[0]}, {a.current_pos[1]})
          </Row>
          {currentTask && (
            <Row label="task">
              <code>{currentTask.id.slice(0, 6)}</code> · {currentTask.title}{" "}
              · <em>{currentTask.status}</em>
            </Row>
          )}
        </dl>

        {possessing ? (
          <div style={{ display: "flex", gap: 6, marginTop: 12 }}>
            <input
              ref={inputRef}
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") sendDirective();
              }}
              placeholder="Directive…"
              style={inputStyle}
              aria-label="Directive body"
            />
            <button
              onClick={sendDirective}
              disabled={!draft.trim()}
              style={primaryBtn}
            >
              Send
            </button>
          </div>
        ) : (
          <p style={{ margin: "12px 0 0", fontSize: 11, color: "var(--fg-secondary)" }}>
            Possess the town to send directives.
          </p>
        )}
      </div>
    </div>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt style={{ fontSize: 11, color: "var(--fg-secondary)" }}>{label}</dt>
      <dd style={{ margin: 0, fontSize: 13 }}>{children}</dd>
    </>
  );
}

const overlayStyle: CSSProperties = {
  position: "fixed",
  inset: 0,
  zIndex: 80,
};

const panelStyle: CSSProperties = {
  position: "fixed",
  background: "var(--raised)",
  border: "1px solid var(--border)",
  borderRadius: 8,
  padding: 14,
  boxShadow: "0 6px 20px rgba(0,0,0,0.16)",
  color: "var(--fg)",
};

const headerStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 10,
  marginBottom: 10,
};

const avatarChipStyle: CSSProperties = {
  width: 32,
  height: 32,
  borderRadius: "50%",
  color: "white",
  display: "grid",
  placeItems: "center",
  fontWeight: 700,
  flexShrink: 0,
};

const dlStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "70px 1fr",
  gap: "4px 10px",
  margin: 0,
};

const inputStyle: CSSProperties = {
  flex: 1,
  font: "inherit",
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: "4px 8px",
  background: "var(--bg)",
  color: "var(--fg)",
};

const primaryBtn: CSSProperties = {
  font: "inherit",
  background: "var(--fg)",
  color: "var(--bg)",
  border: "none",
  borderRadius: 6,
  padding: "4px 12px",
  cursor: "pointer",
};

const btnStyle: CSSProperties = {
  font: "inherit",
  background: "var(--raised)",
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: "4px 12px",
  cursor: "pointer",
  color: "var(--fg)",
  marginTop: 8,
};

const iconBtnStyle: CSSProperties = {
  font: "inherit",
  background: "transparent",
  border: "none",
  cursor: "pointer",
  fontSize: 18,
  color: "var(--fg)",
};

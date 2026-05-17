/**
 * AgentsPanel — Theme G slice 2.
 *
 * Admin-only collapsible panel that lists agents for the currently-selected
 * startup and lets the admin flip `is_peer_reviewer` per row. Mirrors
 * `OperatorsPanel`'s collapsed-by-default + hide-for-non-admin pattern.
 *
 * Wire: sends `agent_set_peer_reviewer` ConsoleInbound (admin-only at the
 * server — non-admins land on a `forbidden` reply). State updates arrive
 * via the next `world_view_snapshot`, which enriches each avatar with
 * `is_peer_reviewer` (see `build_console_snapshot` in `http.rs`).
 */
import { useState, type CSSProperties } from "react";
import { useWorld } from "../hooks/useWorld.js";

export function AgentsPanel({ startupId }: { startupId: string | null }) {
  const { state, send } = useWorld();
  const [open, setOpen] = useState(false);

  if (state.currentOperator?.role !== "admin") return null;
  if (!startupId) return null;

  const agents = Object.values(state.avatars)
    .filter((a) => a.startup_id === startupId && a.agent_id !== "__operator__")
    .sort((a, b) => a.agent_id.localeCompare(b.agent_id));

  const togglePeerReviewer = (agentId: string, next: boolean) => {
    send({
      type: "agent_set_peer_reviewer",
      v: 1,
      agent_id: agentId,
      is_peer_reviewer: next,
    });
  };

  return (
    <div data-testid="agents-panel" style={panelStyle}>
      <button
        style={collapseRowStyle}
        onClick={() => setOpen((v) => !v)}
        data-testid="agents-toggle"
      >
        <span style={headingStyle}>{open ? "▾" : "▸"} Agents</span>
        <span style={countStyle}>{agents.length}</span>
      </button>
      {open && (
        <div style={bodyStyle}>
          {agents.length === 0 ? (
            <p style={emptyStyle}>(no agents in this startup)</p>
          ) : (
            <ul style={listStyle}>
              {agents.map((a) => (
                <li
                  key={a.agent_id}
                  style={rowStyle}
                  data-testid={`agent-row-${a.agent_id}`}
                >
                  <span style={{ flex: 1, fontFamily: "monospace", fontSize: 12 }}>
                    {a.agent_id}
                  </span>
                  <span style={roleStyle}>{a.role}</span>
                  <label style={checkboxLabelStyle}>
                    <input
                      type="checkbox"
                      checked={a.is_peer_reviewer}
                      onChange={(e) =>
                        togglePeerReviewer(a.agent_id, e.target.checked)
                      }
                      data-testid={`agent-peer-reviewer-${a.agent_id}`}
                    />
                    <span>peer reviewer</span>
                  </label>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}

const panelStyle: CSSProperties = {
  padding: "0 16px 8px 16px",
  borderTop: "1px solid var(--border)",
  background: "var(--raised)",
};

const collapseRowStyle: CSSProperties = {
  display: "flex",
  justifyContent: "space-between",
  alignItems: "center",
  width: "100%",
  border: "none",
  background: "transparent",
  cursor: "pointer",
  padding: "12px 0",
  font: "inherit",
  color: "var(--fg)",
};

const headingStyle: CSSProperties = {
  fontSize: 12,
  fontWeight: 600,
  color: "var(--fg-secondary)",
  textTransform: "uppercase",
  letterSpacing: "0.04em",
};

const countStyle: CSSProperties = {
  fontSize: 11,
  color: "var(--fg-secondary)",
};

const bodyStyle: CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 8,
  paddingBottom: 8,
};

const emptyStyle: CSSProperties = {
  fontSize: 12,
  color: "var(--fg-secondary)",
  margin: 0,
};

const listStyle: CSSProperties = {
  listStyle: "none",
  margin: 0,
  padding: 0,
  display: "flex",
  flexDirection: "column",
  gap: 4,
};

const rowStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  fontSize: 13,
  padding: "4px 8px",
  background: "var(--bg)",
  border: "1px solid var(--border)",
  borderRadius: 6,
};

const roleStyle: CSSProperties = {
  fontSize: 11,
  color: "var(--fg-secondary)",
  textTransform: "uppercase",
  letterSpacing: "0.04em",
  padding: "1px 6px",
  border: "1px solid var(--border)",
  borderRadius: 4,
};

const checkboxLabelStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 4,
  fontSize: 12,
  color: "var(--fg)",
  cursor: "pointer",
};

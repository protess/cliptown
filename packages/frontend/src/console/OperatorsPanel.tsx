/**
 * OperatorsPanel — P3 Theme B follow-up: admin-only operator management UI.
 *
 * Collapsed by default. Expanding it sends `operator_list`; non-admin
 * callers see an empty list because the server returns `forbidden`. The
 * panel doesn't gate visibility client-side — admins see real data,
 * everyone else sees "(none)" + the create form (which will also be
 * rejected at the server). Acceptable for an MVP; explicit role-detect
 * would require shipping the operator's identity on the WS hello reply.
 */
import { useCallback, useEffect, useState, type CSSProperties } from "react";
import { useWorld } from "../hooks/useWorld.js";

const ROLES = ["viewer", "manager", "admin"] as const;

export function OperatorsPanel() {
  const { state, send, clearMintedOperatorToken } = useWorld();
  const [open, setOpen] = useState(false);
  const [newName, setNewName] = useState("");
  const [newRole, setNewRole] = useState<(typeof ROLES)[number]>("viewer");

  // Hydrate operator list when the panel is first expanded. Hooks must
  // be called every render — keep this BEFORE the admin gate so React's
  // hook-order invariant holds. (Returning early *before* useEffect
  // crashes the app once the gate flips.)
  useEffect(() => {
    if (open && state.operators === null) {
      send({ type: "operator_list", v: 1 });
    }
  }, [open, state.operators, send]);

  const onCreate = useCallback(() => {
    const trimmed = newName.trim();
    if (!trimmed) return;
    send({ type: "operator_create", v: 1, name: trimmed, role: newRole });
    setNewName("");
    setNewRole("viewer");
  }, [newName, newRole, send]);

  const onRevoke = useCallback(
    (id: string, name: string) => {
      if (!confirm(`Revoke operator "${name}"? Their token stops working immediately.`)) return;
      send({ type: "operator_revoke", v: 1, operator_id: id });
    },
    [send],
  );

  const onRoleChange = useCallback(
    (id: string, role: string) => {
      send({ type: "operator_set_role", v: 1, operator_id: id, role });
    },
    [send],
  );

  // P3 carry-forward: hide the panel entirely for non-admin operators
  // once we know the identity. Pre-hello (`currentOperator === null`) we
  // also hide — avoids the panel briefly flashing in for everyone before
  // the role frame arrives. Render the entire body conditionally rather
  // than returning early so hooks above always run.
  if (state.currentOperator?.role !== "admin") {
    return null;
  }

  return (
    <div data-testid="operators-panel" style={panelStyle}>
      <button
        style={collapseRowStyle}
        onClick={() => setOpen((v) => !v)}
        data-testid="operators-toggle"
      >
        <span style={headingStyle}>{open ? "▾" : "▸"} Operators</span>
        <span style={countStyle}>{state.operators?.length ?? 0}</span>
      </button>
      {open && (
        <div style={bodyStyle}>
          {state.mintedOperatorToken && (
            <MintedTokenBanner
              info={state.mintedOperatorToken}
              onDismiss={clearMintedOperatorToken}
            />
          )}
          {state.operators === null ? (
            <p style={emptyStyle}>loading…</p>
          ) : state.operators.length === 0 ? (
            <p style={emptyStyle}>(no operators visible — admin role required)</p>
          ) : (
            <ul style={listStyle}>
              {state.operators.map((o) => (
                <li key={o.id} style={rowStyle} data-testid={`operator-row-${o.id}`}>
                  <span style={{ flex: 1 }}>{o.name}</span>
                  <select
                    style={selectStyle}
                    value={o.role}
                    onChange={(e) => onRoleChange(o.id, e.target.value)}
                    data-testid={`operator-role-${o.id}`}
                  >
                    {ROLES.map((r) => (
                      <option key={r} value={r}>{r}</option>
                    ))}
                  </select>
                  <button
                    style={revokeStyle}
                    onClick={() => onRevoke(o.id, o.name)}
                    data-testid={`operator-revoke-${o.id}`}
                    title="Revoke this operator's token"
                  >
                    Revoke
                  </button>
                </li>
              ))}
            </ul>
          )}
          <div style={createRowStyle}>
            <input
              style={inputStyle}
              placeholder="new operator name"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              data-testid="operator-new-name"
            />
            <select
              style={selectStyle}
              value={newRole}
              onChange={(e) => setNewRole(e.target.value as (typeof ROLES)[number])}
              data-testid="operator-new-role"
            >
              {ROLES.map((r) => (
                <option key={r} value={r}>{r}</option>
              ))}
            </select>
            <button
              style={createButtonStyle}
              onClick={onCreate}
              disabled={newName.trim() === ""}
              data-testid="operator-create"
            >
              Create
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

interface MintedTokenBannerProps {
  info: { id: string; name: string; token: string };
  onDismiss: () => void;
}

function MintedTokenBanner({ info, onDismiss }: MintedTokenBannerProps) {
  return (
    <div style={bannerStyle} data-testid="minted-token-banner">
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "baseline" }}>
        <strong>New token for {info.name}</strong>
        <button style={dismissStyle} onClick={onDismiss} data-testid="minted-token-dismiss">
          ×
        </button>
      </div>
      <p style={bannerHintStyle}>
        Copy this token now — cliptown will not show it again.
      </p>
      <code style={tokenStyle}>{info.token}</code>
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

const selectStyle: CSSProperties = {
  font: "inherit",
  fontSize: 12,
  background: "var(--bg)",
  color: "var(--fg)",
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: "2px 6px",
};

const revokeStyle: CSSProperties = {
  font: "inherit",
  fontSize: 11,
  background: "transparent",
  border: "1px solid var(--border)",
  color: "var(--danger, #c33)",
  borderRadius: 6,
  padding: "2px 10px",
  cursor: "pointer",
};

const createRowStyle: CSSProperties = {
  display: "flex",
  gap: 6,
  padding: 6,
  background: "var(--bg)",
  border: "1px dashed var(--border)",
  borderRadius: 6,
};

const inputStyle: CSSProperties = {
  flex: 1,
  font: "inherit",
  fontSize: 12,
  background: "var(--raised)",
  color: "var(--fg)",
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: "4px 8px",
};

const createButtonStyle: CSSProperties = {
  font: "inherit",
  fontSize: 11,
  fontWeight: 600,
  background: "var(--bg)",
  color: "var(--fg)",
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: "4px 12px",
  cursor: "pointer",
};

const bannerStyle: CSSProperties = {
  padding: 8,
  background: "var(--bg)",
  border: "1px solid var(--accent, #4a90e2)",
  borderRadius: 6,
};

const bannerHintStyle: CSSProperties = {
  fontSize: 12,
  color: "var(--fg-secondary)",
  margin: "4px 0 4px 0",
};

const tokenStyle: CSSProperties = {
  display: "block",
  fontSize: 12,
  fontFamily: "monospace",
  padding: 6,
  background: "var(--raised)",
  borderRadius: 4,
  wordBreak: "break-all",
};

const dismissStyle: CSSProperties = {
  font: "inherit",
  fontSize: 14,
  background: "transparent",
  border: "none",
  cursor: "pointer",
  color: "var(--fg-secondary)",
  padding: 0,
};

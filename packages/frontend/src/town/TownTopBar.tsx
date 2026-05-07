/**
 * TownTopBar: horizontal top band for the `/town/:id` route. Shows a Back link
 * to /console, the startup name + monogram in the deterministic startup hue,
 * a thin budget bar with the same 80% / 95% / 100% thresholds as MainHeader,
 * a Possess/Unpossess toggle that sends ConsoleInbound::Operator(Un)Possess
 * over /ws/console, and a small connection-status dot.
 *
 * Phase 0 / M4.8 — possession is detected by the presence of the operator
 * avatar (`__operator__`) in `state.avatars`, mirroring M1.12 cmd_console.
 */
import { useEffect, useRef, useState, type CSSProperties } from "react";
import { Link } from "react-router-dom";
import { useWorld } from "../hooks/useWorld.js";

const HUES = [
  "#E63946",
  "#F4A261",
  "#E9C46A",
  "#2A9D8F",
  "#264653",
  "#A663CC",
  "#FF8FAB",
  "#83A4D4",
] as const;

const OPERATOR_AVATAR_ID = "__operator__";

function hueFor(id: string): string {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) | 0;
  return HUES[Math.abs(h) % HUES.length]!;
}

function monogramFor(name: string | undefined, id: string): string {
  const src = name && name.trim().length > 0 ? name : id;
  return src.slice(0, 1).toUpperCase();
}

export function TownTopBar({ startupId }: { startupId: string }) {
  const { state, send } = useWorld();
  const s = state.startups[startupId];
  const possessing = OPERATOR_AVATAR_ID in state.avatars;
  const [busy, setBusy] = useState(false);

  const togglePossess = () => {
    setBusy(true);
    if (possessing) {
      send({ type: "operator_unpossess", v: 1 });
    } else {
      send({ type: "operator_possess", v: 1, startup_id: startupId });
    }
    setTimeout(() => setBusy(false), 250);
  };

  // M4.13 — global keymap `p` fires the same toggle. Use a ref so we always
  // call the latest closure without re-binding the listener on every render.
  const toggleRef = useRef(togglePossess);
  toggleRef.current = togglePossess;
  useEffect(() => {
    const onToggle = () => toggleRef.current();
    window.addEventListener("cliptown:possess-toggle", onToggle);
    return () =>
      window.removeEventListener("cliptown:possess-toggle", onToggle);
  }, []);

  const hue = hueFor(startupId);
  const spent = s?.budget_spent_usd ?? 0;
  const cap = s?.budget_cap_usd ?? 0;
  const ratio = cap > 0 ? Math.min(spent / cap, 1.2) : 0;
  const barColor =
    ratio >= 1
      ? "#D62828"
      : ratio >= 0.95
        ? "#E69F00"
        : ratio >= 0.8
          ? "#E9C46A"
          : hue;

  return (
    <div style={bandStyle}>
      <Link to="/console" style={backStyle} aria-label="Back to console">
        ← Back
      </Link>

      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
        <span
          aria-hidden
          style={{
            width: 28,
            height: 28,
            borderRadius: 6,
            background: hue,
            color: "white",
            display: "grid",
            placeItems: "center",
            fontWeight: 700,
            fontSize: 13,
          }}
        >
          {monogramFor(s?.name, startupId)}
        </span>
        <div>
          <div style={{ fontWeight: 600 }}>{s?.name ?? startupId}</div>
          <div style={{ fontSize: 11, color: "var(--fg-secondary)" }}>
            <code>{startupId}</code>
          </div>
        </div>
      </div>

      <div
        style={{
          display: "flex",
          flexDirection: "column",
          gap: 4,
          minWidth: 180,
        }}
      >
        <div style={{ fontSize: 11, color: "var(--fg-secondary)" }}>
          Budget: ${spent.toFixed(2)} / ${cap.toFixed(2)}
        </div>
        <div style={budgetTrackStyle}>
          <div
            aria-label="budget-bar"
            style={{
              width: `${Math.min(ratio, 1) * 100}%`,
              height: "100%",
              background: barColor,
              transition: "width 200ms ease",
            }}
          />
        </div>
      </div>

      <button
        onClick={togglePossess}
        disabled={busy}
        aria-pressed={possessing}
        style={{
          marginLeft: "auto",
          font: "inherit",
          background: possessing ? "var(--fg)" : "var(--raised)",
          color: possessing ? "var(--bg)" : "var(--fg)",
          border: "1px solid var(--border)",
          borderRadius: 6,
          padding: "6px 14px",
          cursor: busy ? "wait" : "pointer",
          fontWeight: 500,
        }}
      >
        {possessing ? "Unpossess" : "Possess"}
      </button>

      <span
        aria-label="connection-status"
        title={state.status}
        style={{
          width: 8,
          height: 8,
          borderRadius: "50%",
          background:
            state.status === "open"
              ? "#2A9D8F"
              : state.status === "connecting"
                ? "#E9C46A"
                : "#D62828",
        }}
      />
    </div>
  );
}

const bandStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 24,
  padding: "10px 20px",
  borderBottom: "1px solid var(--border)",
  background: "var(--raised)",
  position: "sticky",
  top: 0,
  zIndex: 10,
};

const backStyle: CSSProperties = {
  textDecoration: "none",
  color: "var(--fg)",
  fontWeight: 500,
};

const budgetTrackStyle: CSSProperties = {
  width: "100%",
  height: 5,
  background: "var(--border)",
  borderRadius: 3,
  overflow: "hidden",
};

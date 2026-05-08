/**
 * MainHeader: horizontal "header band" at the top of the main area showing
 * the currently-selected startup — name + monogram in startup hue, a thin
 * budget bar with 80% / 95% / 100% thresholds, derived counts (agents,
 * tasks, in-progress), and an "Open town →" CTA that routes to
 * `/town/:startupId`.
 *
 * Phase 0 / M4.5 — empty state when no startup is selected. Hue palette is
 * the same 8-color deterministic-by-id-hash set used in Sidebar; kept inline
 * here to avoid a third file for now.
 */

import { Link } from "react-router-dom";
import type { CSSProperties } from "react";
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

function hueFor(id: string): string {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) | 0;
  return HUES[Math.abs(h) % HUES.length]!;
}

function monogramFor(name: string | undefined, id: string): string {
  const src = name && name.trim().length > 0 ? name : id;
  return src.slice(0, 1).toUpperCase();
}

export function MainHeader({ startupId }: { startupId: string | null }) {
  const { state } = useWorld();
  if (!startupId) {
    return (
      <div style={emptyStyle}>
        <p style={{ margin: 0, color: "var(--fg-secondary)" }}>
          Select a startup from the sidebar.
        </p>
      </div>
    );
  }
  const s = state.startups[startupId];
  if (!s) {
    return (
      <div style={emptyStyle}>
        <p style={{ margin: 0, color: "var(--fg-secondary)" }}>
          Startup <code>{startupId}</code> not found.
        </p>
      </div>
    );
  }
  const hue = hueFor(s.id);

  const agentCount = Object.values(state.avatars).filter(
    (a) => a.startup_id === s.id,
  ).length;
  const allTasks = Object.values(state.tasks).filter(
    (t) => t.startup_id === s.id,
  );
  const inProgress = allTasks.filter((t) => t.status === "in_progress").length;
  const taskCount = allTasks.length;

  const spent = s.budget_spent_usd ?? 0;
  const cap = s.budget_cap_usd ?? 0;
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
      <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
        <span
          aria-hidden
          style={{
            width: 32,
            height: 32,
            borderRadius: 6,
            background: hue,
            color: "white",
            display: "grid",
            placeItems: "center",
            fontWeight: 700,
          }}
        >
          {monogramFor(s.name, s.id)}
        </span>
        <div>
          <div style={{ fontWeight: 600 }}>{s.name || s.id}</div>
          <div style={{ fontSize: 12, color: "var(--fg-secondary)" }}>
            <code title={s.id}>{s.id.slice(0, 6)}</code>
          </div>
        </div>
      </div>

      <div
        style={{
          display: "flex",
          flexDirection: "column",
          gap: 4,
          minWidth: 220,
        }}
      >
        <div style={{ fontSize: 12, color: "var(--fg-secondary)" }}>
          Budget: ${spent.toFixed(2)} / ${cap.toFixed(2)}
          {ratio >= 1 && " (paused)"}
          {ratio >= 0.95 && ratio < 1 && " (warn)"}
        </div>
        <div
          style={{
            width: "100%",
            height: 6,
            background: "var(--border)",
            borderRadius: 3,
            overflow: "hidden",
          }}
        >
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

      <div style={{ display: "flex", gap: 16 }}>
        <Stat label="agents" value={agentCount} />
        <Stat label="tasks" value={taskCount} />
        <Stat label="active" value={inProgress} />
      </div>

      <Link
        to={`/town/${s.id}`}
        style={{
          marginLeft: "auto",
          textDecoration: "none",
          color: "var(--fg)",
          border: "1px solid var(--border)",
          borderRadius: 6,
          padding: "6px 12px",
          fontWeight: 500,
        }}
      >
        Open town →
      </Link>
    </div>
  );
}

function Stat({ label, value }: { label: string; value: number }) {
  return (
    <div style={{ textAlign: "center" }}>
      <div style={{ fontWeight: 600 }}>{value}</div>
      <div style={{ fontSize: 11, color: "var(--fg-secondary)" }}>{label}</div>
    </div>
  );
}

const bandStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 24,
  padding: "12px 24px",
  borderBottom: "1px solid var(--border)",
  background: "var(--raised)",
};

const emptyStyle: CSSProperties = {
  padding: "20px 24px",
  borderBottom: "1px solid var(--border)",
  background: "var(--raised)",
};

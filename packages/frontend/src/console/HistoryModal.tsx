/**
 * HistoryModal: full system-event list rendered in a centered modal.
 *
 * Phase 0 / M4.3 — bound to the in-memory store cap (MAX_SYSTEM_EVENTS in
 * store.ts). Closes on backdrop click and on the explicit close button.
 *
 * M4.13 — also closes on the `cliptown:dismiss` event so the global Esc
 * handler can dismiss it without prop drilling.
 *
 * Theme G slice 5 — adds filtering by severity (toggle chips), kind
 * (substring match), and startup (dropdown). Uses the same
 * `prettifySystemEventPayload` helper as the TopBar marquee so the detail
 * column reads as "T1 stolen by e2 ← e1 (auto)" instead of raw JSON.
 */

import { useEffect, useMemo, useState, type CSSProperties } from "react";
import type { SystemEventVM } from "../store.js";
import { prettifySystemEventPayload } from "../store.js";

const ALL_SEVERITIES: Array<SystemEventVM["severity"]> = [
  "info",
  "warn",
  "alert",
  "critical",
];

export function HistoryModal({
  events,
  onClose,
}: {
  events: SystemEventVM[];
  onClose: () => void;
}) {
  useEffect(() => {
    const onDismiss = () => onClose();
    window.addEventListener("cliptown:dismiss", onDismiss);
    return () => window.removeEventListener("cliptown:dismiss", onDismiss);
  }, [onClose]);

  // Filter state: severity is a Set (toggle chips); kind is substring;
  // startup is a single-select dropdown (empty = no filter).
  const [enabledSeverities, setEnabledSeverities] = useState<
    Set<SystemEventVM["severity"]>
  >(() => new Set(ALL_SEVERITIES));
  const [kindFilter, setKindFilter] = useState("");
  const [startupFilter, setStartupFilter] = useState("");

  const startupOptions = useMemo(() => {
    const seen = new Set<string>();
    for (const e of events) {
      if (e.startup_id) seen.add(e.startup_id);
    }
    return Array.from(seen).sort();
  }, [events]);

  const filtered = useMemo(() => {
    const kindLc = kindFilter.trim().toLowerCase();
    return events.filter((e) => {
      if (!enabledSeverities.has(e.severity)) return false;
      if (startupFilter && e.startup_id !== startupFilter) return false;
      if (kindLc && !e.kind.toLowerCase().includes(kindLc)) return false;
      return true;
    });
  }, [events, enabledSeverities, kindFilter, startupFilter]);

  const toggleSeverity = (s: SystemEventVM["severity"]) => {
    setEnabledSeverities((prev) => {
      const next = new Set(prev);
      if (next.has(s)) next.delete(s);
      else next.add(s);
      return next;
    });
  };

  return (
    <div
      onClick={onClose}
      role="dialog"
      aria-label="System event history"
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(0,0,0,0.4)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        zIndex: 100,
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          background: "var(--raised)",
          borderRadius: 8,
          padding: 16,
          width: "min(720px, 90vw)",
          maxHeight: "80vh",
          overflow: "auto",
          boxShadow: "0 8px 24px rgba(0,0,0,0.16)",
        }}
      >
        <header
          style={{
            display: "flex",
            justifyContent: "space-between",
            marginBottom: 12,
            alignItems: "baseline",
          }}
        >
          <h2 style={{ margin: 0, fontWeight: 700 }}>System events</h2>
          <span style={{ fontSize: 12, color: "var(--fg-secondary)" }}>
            {filtered.length} of {events.length}
          </span>
          <button
            onClick={onClose}
            style={{
              font: "inherit",
              background: "transparent",
              border: "none",
              cursor: "pointer",
            }}
          >
            Close
          </button>
        </header>

        <div style={filterRowStyle} data-testid="history-filters">
          <div style={{ display: "flex", gap: 4 }}>
            {ALL_SEVERITIES.map((s) => {
              const on = enabledSeverities.has(s);
              return (
                <button
                  key={s}
                  onClick={() => toggleSeverity(s)}
                  data-testid={`history-severity-${s}`}
                  style={severityChipStyle(s, on)}
                  title={`Toggle ${s}`}
                >
                  {s}
                </button>
              );
            })}
          </div>
          <input
            placeholder="filter by kind…"
            value={kindFilter}
            onChange={(e) => setKindFilter(e.target.value)}
            data-testid="history-kind-filter"
            style={inputStyle}
          />
          <select
            value={startupFilter}
            onChange={(e) => setStartupFilter(e.target.value)}
            data-testid="history-startup-filter"
            style={selectStyle}
          >
            <option value="">All startups</option>
            {startupOptions.map((sid) => (
              <option key={sid} value={sid}>
                {sid.slice(0, 8)}
              </option>
            ))}
          </select>
        </div>

        {filtered.length === 0 ? (
          <p style={{ color: "var(--fg-secondary)" }}>
            {events.length === 0 ? "No events yet." : "No events match the filters."}
          </p>
        ) : (
          <ul style={{ listStyle: "none", padding: 0, margin: 0 }}>
            {filtered.map((e, i) => (
              <li
                key={i}
                style={{
                  display: "grid",
                  gridTemplateColumns: "auto 80px 200px 1fr",
                  gap: 8,
                  fontFamily: "var(--mono, ui-monospace)",
                  fontSize: 12,
                  padding: "4px 0",
                  borderBottom: "1px solid var(--border)",
                }}
              >
                <span>{new Date(e.ts).toLocaleTimeString()}</span>
                <span style={{ color: severityColor(e.severity) }}>
                  {e.severity}
                </span>
                <code>{e.kind}</code>
                <span
                  style={{
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                  }}
                  title={detailString(e.payload)}
                >
                  {prettifySystemEventPayload(e.kind, e.payload) || detailString(e.payload)}
                </span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function severityColor(s: SystemEventVM["severity"]): string {
  return s === "critical" ? "#8B0000" : s === "alert" ? "#D62828" : s === "warn" ? "#E69F00" : "#6B6B6B";
}

function detailString(d: unknown): string {
  if (typeof d === "string") return d;
  if (d == null) return "";
  return JSON.stringify(d);
}

function severityChipStyle(
  s: SystemEventVM["severity"],
  on: boolean,
): CSSProperties {
  const fg = severityColor(s);
  return {
    font: "inherit",
    fontSize: 10,
    textTransform: "uppercase",
    letterSpacing: "0.04em",
    background: on ? fg : "transparent",
    color: on ? "white" : fg,
    border: `1px solid ${fg}`,
    borderRadius: 999,
    padding: "2px 8px",
    cursor: "pointer",
    opacity: on ? 1 : 0.7,
  };
}

const filterRowStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  marginBottom: 10,
  flexWrap: "wrap",
};

const inputStyle: CSSProperties = {
  flex: 1,
  font: "inherit",
  fontSize: 12,
  background: "var(--bg)",
  color: "var(--fg)",
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: "3px 8px",
  minWidth: 120,
};

const selectStyle: CSSProperties = {
  font: "inherit",
  fontSize: 12,
  background: "var(--bg)",
  color: "var(--fg)",
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: "3px 8px",
};

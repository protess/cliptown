/**
 * HistoryModal: full system-event list rendered in a centered modal.
 *
 * Phase 0 / M4.3 — bound to the in-memory store cap (MAX_SYSTEM_EVENTS in
 * store.ts). Closes on backdrop click and on the explicit close button.
 *
 * M4.13 — also closes on the `cliptown:dismiss` event so the global Esc
 * handler can dismiss it without prop drilling.
 */

import { useEffect } from "react";
import type { SystemEventVM } from "../store.js";

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
        {events.length === 0 ? (
          <p style={{ color: "var(--fg-secondary)" }}>No events yet.</p>
        ) : (
          <ul style={{ listStyle: "none", padding: 0, margin: 0 }}>
            {events.map((e, i) => (
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
                >
                  {detailString(e.payload)}
                </span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function severityColor(s: "info" | "warn" | "alert"): string {
  return s === "alert" ? "#D62828" : s === "warn" ? "#E69F00" : "#6B6B6B";
}

function detailString(d: unknown): string {
  if (typeof d === "string") return d;
  if (d == null) return "";
  return JSON.stringify(d);
}

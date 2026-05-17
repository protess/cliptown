/**
 * TopBar: wordmark, rolling 1-line system event marquee, "+ New Startup"
 * trigger, History modal trigger, and a settings menu with Recheck Backends.
 *
 * Phase 0 / M4.3 + M4.7 — the "+ New Startup" button now opens the
 * NewStartupModal (M4.7). Recheck POSTs to the world's
 * /api/backend-catalog/recheck (plumbed in M0/M1.4); failures are swallowed
 * since the world surfaces follow-up state via system_event ConsoleOutbound
 * messages.
 */

import { useEffect, useState } from "react";
import type { CSSProperties } from "react";
import { useWorld } from "../hooks/useWorld.js";
import { prettifySystemEventPayload } from "../store.js";
import { HistoryModal } from "./HistoryModal.js";
import { NewStartupModal } from "./NewStartupModal.js";

const ROTATE_MS = 3_000;
// Empty default → relative URL so fetch() goes through the Vite dev proxy
// (vite.config.ts) rather than triggering a cross-origin preflight against
// 127.0.0.1:8080. Production builds set VITE_WORLD_HTTP_URL explicitly.
const RECHECK_URL =
  ((import.meta.env.VITE_WORLD_HTTP_URL as string | undefined) ?? "") +
  "/api/backend-catalog/recheck";

export function TopBar() {
  const { state } = useWorld();
  const recent = state.systemEvents.slice(0, 3);
  const [tickIdx, setTickIdx] = useState(0);
  const [menuOpen, setMenuOpen] = useState(false);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [newStartupOpen, setNewStartupOpen] = useState(false);
  const [recheckBusy, setRecheckBusy] = useState(false);

  useEffect(() => {
    if (recent.length <= 1) return;
    const id = setInterval(() => {
      setTickIdx((i) => (i + 1) % recent.length);
    }, ROTATE_MS);
    return () => clearInterval(id);
  }, [recent.length]);

  // Reset tickIdx when the list shrinks under the current index.
  useEffect(() => {
    if (tickIdx >= recent.length) setTickIdx(0);
  }, [recent.length, tickIdx]);

  // M4.13 — global keymap "/" opens the New Startup modal as a Phase-0
  // search stand-in.
  useEffect(() => {
    const onOpen = () => setNewStartupOpen(true);
    window.addEventListener("cliptown:new-startup", onOpen);
    return () => window.removeEventListener("cliptown:new-startup", onOpen);
  }, []);

  // M4.13 — Escape dismisses any open menu/modal hosted here.
  useEffect(() => {
    const onDismiss = () => {
      setMenuOpen(false);
      setHistoryOpen(false);
      setNewStartupOpen(false);
    };
    window.addEventListener("cliptown:dismiss", onDismiss);
    return () => window.removeEventListener("cliptown:dismiss", onDismiss);
  }, []);

  const recheck = async () => {
    setRecheckBusy(true);
    setMenuOpen(false);
    try {
      await fetch(RECHECK_URL, { method: "POST" });
    } catch {
      /* swallow — toast surfaces in store via system_event eventually */
    } finally {
      setRecheckBusy(false);
    }
  };

  const newStartup = () => setNewStartupOpen(true);

  const ev = recent[tickIdx];

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 16,
        padding: "8px 16px",
        borderBottom: "1px solid var(--border)",
        background: "var(--raised)",
        position: "sticky",
        top: 0,
        zIndex: 10,
      }}
    >
      <span style={{ fontWeight: 700, letterSpacing: "-0.02em" }}>cliptown</span>

      <div
        aria-label="event-feed"
        style={{
          flex: 1,
          minWidth: 0,
          overflow: "hidden",
          whiteSpace: "nowrap",
          textOverflow: "ellipsis",
          color: "var(--fg-secondary)",
          fontSize: 13,
        }}
      >
        {ev ? (
          <span>
            <SeverityDot severity={ev.severity} />
            <code style={{ marginRight: 6 }}>{ev.kind}</code>
            <span>
              {prettifySystemEventPayload(ev.kind, ev.payload) || describeDetail(ev.payload)}
            </span>
          </span>
        ) : (
          <span>No events yet.</span>
        )}
      </div>

      <button onClick={newStartup} style={btnStyle}>
        + New Startup
      </button>

      <button onClick={() => setHistoryOpen(true)} style={btnStyle}>
        History
      </button>

      <div style={{ position: "relative" }}>
        <button
          onClick={() => setMenuOpen((v) => !v)}
          style={btnStyle}
          aria-haspopup="menu"
          aria-expanded={menuOpen}
        >
          ⚙
        </button>
        {menuOpen && (
          <div
            role="menu"
            style={{
              position: "absolute",
              right: 0,
              top: "100%",
              background: "var(--raised)",
              border: "1px solid var(--border)",
              borderRadius: 6,
              padding: 4,
              minWidth: 180,
              boxShadow: "0 4px 12px rgba(0,0,0,0.08)",
            }}
          >
            <button
              onClick={recheck}
              disabled={recheckBusy}
              style={{ ...menuItemStyle, opacity: recheckBusy ? 0.5 : 1 }}
            >
              {recheckBusy ? "Rechecking…" : "Recheck Backends"}
            </button>
          </div>
        )}
      </div>

      {historyOpen && (
        <HistoryModal
          events={state.systemEvents}
          onClose={() => setHistoryOpen(false)}
        />
      )}

      {newStartupOpen && (
        <NewStartupModal onClose={() => setNewStartupOpen(false)} />
      )}
    </div>
  );
}

const btnStyle: CSSProperties = {
  font: "inherit",
  background: "var(--raised)",
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: "4px 10px",
  cursor: "pointer",
};

const menuItemStyle: CSSProperties = {
  display: "block",
  width: "100%",
  textAlign: "left",
  background: "transparent",
  border: "none",
  padding: "6px 10px",
  cursor: "pointer",
  font: "inherit",
};

function SeverityDot({ severity }: { severity: "info" | "warn" | "alert" | "critical" }) {
  const color =
    severity === "critical"
      ? "#8B0000"
      : severity === "alert"
      ? "#D62828"
      : severity === "warn"
      ? "#E69F00"
      : "#6B6B6B";
  return (
    <span
      aria-hidden
      style={{
        display: "inline-block",
        width: 8,
        height: 8,
        borderRadius: "50%",
        background: color,
        marginRight: 6,
        verticalAlign: "middle",
      }}
    />
  );
}

function describeDetail(d: unknown): string {
  if (typeof d === "string") return d;
  if (d && typeof d === "object") {
    const o = d as Record<string, unknown>;
    if (typeof o.message === "string") return o.message;
    return JSON.stringify(o);
  }
  return "";
}

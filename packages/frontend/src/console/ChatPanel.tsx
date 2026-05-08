/**
 * ChatPanel — floating, collapsible chat surface (M4.10).
 *
 * Bottom-right of the viewport. Collapses to a chip with an unread badge.
 * When expanded, lists messages from `state.messages` filtered by scope:
 *   selected agent's room → operator's room → all (no filter).
 *
 * Send box is enabled only when the operator is possessing a town (detected
 * by the `__operator__` avatar being present in `state.avatars`, mirroring
 * the convention from M1.12 cmd_console + TownTopBar). With a selected
 * agent, the send emits `operator_directive` (a defined ConsoleInbound
 * variant — see packages/protocol/dist/ConsoleInbound.ts). Without a
 * selected agent, the send emits `operator_chat` as a forward-compat frame:
 * the protocol doesn't yet ship a room-scoped operator chat variant, so the
 * server may ignore it until M5+ adds one. The reducer in store.ts likewise
 * routes any inbound `chat` / `directive` ConsoleOutbound frame into
 * `state.messages`, so the panel populates as soon as the world emits them.
 */
import { useEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent } from "react";
import { useWorld } from "../hooks/useWorld.js";
import type { MessageVM } from "../store.js";

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

interface ChatPanelProps {
  /** When set, scope to this agent's room (their context). */
  selectedAgentId?: string | null;
}

export function ChatPanel({ selectedAgentId }: ChatPanelProps = {}) {
  const { state, send } = useWorld();
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState("");
  const [readCount, setReadCount] = useState(0);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  const op = state.avatars[OPERATOR_AVATAR_ID];
  const possessing = !!op;

  // Scope priority: selected agent's room → operator's room → all.
  const scopedRoom: string | null = (() => {
    if (selectedAgentId) {
      const a = state.avatars[selectedAgentId];
      if (a?.room_id) return a.room_id;
    }
    if (op?.room_id) return op.room_id;
    return null;
  })();

  const scopedStartup: string | null = (() => {
    if (selectedAgentId) return state.avatars[selectedAgentId]?.startup_id ?? null;
    return op?.startup_id ?? null;
  })();

  const visible = useMemo(() => {
    return state.messages.filter((m) => {
      if (scopedRoom && m.room_id && m.room_id !== scopedRoom) return false;
      // Cross-startup messages still appear; tagging happens at render.
      return true;
    });
  }, [state.messages, scopedRoom]);

  const unread = Math.max(0, visible.length - readCount);

  useEffect(() => {
    if (open && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
      setReadCount(visible.length);
    }
  }, [open, visible.length]);

  // M4.13 — global keymap `c` opens the panel; Escape closes it.
  useEffect(() => {
    const onOpen = () => setOpen(true);
    const onDismiss = () => setOpen(false);
    window.addEventListener("cliptown:chat-open", onOpen);
    window.addEventListener("cliptown:dismiss", onDismiss);
    return () => {
      window.removeEventListener("cliptown:chat-open", onOpen);
      window.removeEventListener("cliptown:dismiss", onDismiss);
    };
  }, []);

  const submit = () => {
    const body = draft.trim();
    if (!body || !possessing) return;
    if (selectedAgentId) {
      send({
        type: "operator_directive",
        v: 1,
        to_agent_id: selectedAgentId,
        body,
      });
    } else {
      // Forward-compat: no `operator_chat` variant exists in ConsoleInbound
      // today. The world ignores unknown frames; M5+ may add a room-scoped
      // operator chat command, at which point this send will start working.
      send({ type: "operator_chat", v: 1, body });
    }
    setDraft("");
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") submit();
  };

  if (!open) {
    return (
      <button
        type="button"
        onClick={() => setOpen(true)}
        style={chipStyle}
        aria-label="Open chat"
      >
        Chat
        {unread > 0 && <span style={badgeStyle}>{unread}</span>}
      </button>
    );
  }

  return (
    <aside style={panelStyle} role="complementary" aria-label="Chat">
      <header style={headerStyle}>
        <span style={{ fontWeight: 600, fontSize: 13 }}>
          Chat
          {scopedRoom && (
            <span style={{ color: "var(--fg-secondary)", fontWeight: 400 }}>
              {" · "}
              {scopedRoom}
            </span>
          )}
        </span>
        <button
          type="button"
          onClick={() => setOpen(false)}
          style={iconBtnStyle}
          aria-label="Collapse chat"
        >
          ▼
        </button>
      </header>
      <div ref={scrollRef} style={listStyle}>
        {visible.length === 0 ? (
          <p style={{ color: "var(--fg-secondary)", fontSize: 12, margin: 0 }}>
            No messages yet.
          </p>
        ) : (
          visible.map((m) => (
            <Bubble key={m.id} m={m} scopedStartup={scopedStartup} />
          ))
        )}
      </div>
      <footer style={footerStyle}>
        {possessing ? (
          <>
            <input
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={onKeyDown}
              placeholder={
                selectedAgentId
                  ? `Directive to ${selectedAgentId}`
                  : "Chat in current room"
              }
              style={inputStyle}
              aria-label="Message body"
            />
            <button
              type="button"
              onClick={submit}
              disabled={!draft.trim()}
              style={sendBtnStyle}
            >
              Send
            </button>
          </>
        ) : (
          <p style={{ color: "var(--fg-secondary)", fontSize: 11, margin: 0 }}>
            Possess a town to send messages.
          </p>
        )}
      </footer>
    </aside>
  );
}

// Sentinel ids like `operator` (cmd_console.rs writes this for
// operator-sourced directives) and `__operator__` (the avatar id) stay
// readable; real agent UUIDs (36-char, dashes at positions 8/13/18/23)
// get the Sidebar/MainHeader/TownTopBar 6-char + title pattern.
function looksLikeUuid(id: string): boolean {
  return id.length === 36 && id.charAt(8) === "-" && id.charAt(13) === "-";
}
function AuthorId({ id }: { id: string }) {
  if (!id) return <code>?</code>;
  if (!looksLikeUuid(id)) return <code>{id}</code>;
  return <code title={id}>{id.slice(0, 6)}</code>;
}

function Bubble({
  m,
  scopedStartup,
}: {
  m: MessageVM;
  scopedStartup: string | null;
}) {
  const cross = scopedStartup !== null && m.startup_id !== "" && m.startup_id !== scopedStartup;
  const tag = m.kind === "directive" ? "→" : "·";
  const hue = hueFor(m.startup_id || m.author_id);
  return (
    <div style={{ marginBottom: 6 }}>
      <div
        style={{
          fontSize: 11,
          color: "var(--fg-secondary)",
          display: "flex",
          gap: 6,
          alignItems: "center",
        }}
      >
        <span
          aria-hidden
          style={{
            width: 6,
            height: 6,
            borderRadius: "50%",
            background: hue,
            opacity: cross ? 1 : 0.6,
          }}
        />
        <AuthorId id={m.author_id} />
        {cross && (
          <span
            title={m.startup_id}
            style={{ color: hue, fontWeight: 600 }}
          >
            {m.startup_id.slice(0, 6)}
          </span>
        )}
        <span>{tag}</span>
        <span>{new Date(m.ts).toLocaleTimeString()}</span>
      </div>
      <div style={{ fontSize: 13, paddingLeft: 12 }}>{m.body}</div>
    </div>
  );
}

const chipStyle: CSSProperties = {
  position: "fixed",
  right: 24,
  bottom: 24,
  background: "var(--raised)",
  border: "1px solid var(--border)",
  borderRadius: 999,
  padding: "8px 16px",
  font: "inherit",
  color: "var(--fg)",
  cursor: "pointer",
  boxShadow: "0 2px 8px rgba(0,0,0,0.08)",
  display: "flex",
  alignItems: "center",
  gap: 8,
  zIndex: 50,
};

const badgeStyle: CSSProperties = {
  background: "#D62828",
  color: "white",
  borderRadius: 999,
  padding: "0 6px",
  fontSize: 11,
  fontWeight: 700,
};

const panelStyle: CSSProperties = {
  position: "fixed",
  right: 24,
  bottom: 24,
  width: 320,
  height: 360,
  background: "var(--raised)",
  border: "1px solid var(--border)",
  borderRadius: 8,
  display: "flex",
  flexDirection: "column",
  boxShadow: "0 4px 16px rgba(0,0,0,0.12)",
  zIndex: 50,
};

const headerStyle: CSSProperties = {
  display: "flex",
  justifyContent: "space-between",
  alignItems: "center",
  padding: "8px 10px",
  borderBottom: "1px solid var(--border)",
};

const listStyle: CSSProperties = {
  flex: 1,
  padding: "8px 10px",
  overflowY: "auto",
};

const footerStyle: CSSProperties = {
  padding: "8px 10px",
  borderTop: "1px solid var(--border)",
  display: "flex",
  gap: 6,
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

const sendBtnStyle: CSSProperties = {
  font: "inherit",
  background: "var(--fg)",
  color: "var(--bg)",
  border: "none",
  borderRadius: 6,
  padding: "4px 10px",
  cursor: "pointer",
};

const iconBtnStyle: CSSProperties = {
  font: "inherit",
  background: "transparent",
  border: "none",
  cursor: "pointer",
  color: "var(--fg)",
};

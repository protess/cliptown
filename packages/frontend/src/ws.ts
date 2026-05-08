/**
 * Reconnecting WebSocket client for the operator console.
 *
 * Contract (see crates/world/src/http.rs::handle_console):
 *   1. On connect, send `{type:"hello", v:1, operator_token}` as the first frame.
 *   2. If the server replies `{type:"auth_error"}`, close and stop reconnecting.
 *   3. Subsequent inbound frames are ConsoleOutbound variants (JSON-text only).
 *
 * ChunkFrame reassembly (M1.11):
 *   The world chunks WorldViewSnapshot payloads >256 KiB into ChunkFrame
 *   messages. We buffer chunks keyed by `snapshot_id`, ordered by `index`,
 *   and once `total` chunks are present, concatenate their `chunk` strings
 *   and JSON.parse the result before delivering it to `onMessage`. The
 *   wire format expected here is:
 *     `{ type: "chunk_frame", v: 1, snapshot_id, index, total, chunk }`
 *   matching crates/world/src/view.rs::ChunkFrame once the WS handler is
 *   updated to emit them. Single-frame snapshots are still delivered as
 *   plain `world_view_snapshot` messages with no chunking overhead.
 *
 * Backoff: [500ms, 1s, 2s, 5s, 10s] capped, with up to 250ms jitter.
 */

export type ConnectionStatus =
  | "connecting"
  | "open"
  | "auth_error"
  | "closed";

export interface ConsoleClientOpts {
  url: string;
  operatorToken: string;
  onMessage: (msg: object) => void;
  onStatus?: (status: ConnectionStatus) => void;
}

const BACKOFF_MS = [500, 1_000, 2_000, 5_000, 10_000];
const MAX_JITTER_MS = 250;

interface ChunkBuf {
  total: number;
  parts: Map<number, string>;
}

export class ConsoleClient {
  private opts: ConsoleClientOpts;
  private ws: WebSocket | null = null;
  private closedByUser = false;
  private failures = 0;
  private chunks = new Map<string, ChunkBuf>();
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(opts: ConsoleClientOpts) {
    this.opts = opts;
  }

  start(): void {
    this.closedByUser = false;
    this.connect();
  }

  close(): void {
    this.closedByUser = true;
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.ws) {
      try {
        this.ws.close();
      } catch {
        // ignore — close races with onclose
      }
      this.ws = null;
    }
  }

  send(msg: object): void {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;
    this.ws.send(JSON.stringify(msg));
  }

  private connect(): void {
    this.opts.onStatus?.("connecting");
    let ws: WebSocket;
    try {
      ws = new WebSocket(this.opts.url);
    } catch {
      // Construction can throw on bad URLs; treat as a failure and back off.
      this.scheduleReconnect();
      return;
    }
    this.ws = ws;

    ws.onopen = () => {
      this.failures = 0;
      ws.send(
        JSON.stringify({
          type: "hello",
          v: 1,
          operator_token: this.opts.operatorToken,
        }),
      );
      this.opts.onStatus?.("open");
    };

    ws.onmessage = (ev) => {
      if (typeof ev.data !== "string") return;
      let parsed: unknown;
      try {
        parsed = JSON.parse(ev.data);
      } catch {
        return;
      }
      if (!parsed || typeof parsed !== "object") return;
      const m = parsed as Record<string, unknown>;

      if (m.type === "auth_error") {
        this.opts.onStatus?.("auth_error");
        // Treat auth errors as terminal — operator must refresh credentials.
        this.closedByUser = true;
        try {
          ws.close();
        } catch {
          // ignore
        }
        return;
      }

      if (m.type === "chunk_frame") {
        this.handleChunk(m);
        return;
      }

      this.opts.onMessage(m);
    };

    ws.onclose = () => {
      this.ws = null;
      // auth_error path already flipped closedByUser; status was emitted there.
      if (!this.closedByUser) {
        this.opts.onStatus?.("closed");
        this.scheduleReconnect();
      }
    };

    ws.onerror = () => {
      // onclose runs next; reconnect scheduling lives there.
    };
  }

  private scheduleReconnect(): void {
    if (this.closedByUser) return;
    const idx = Math.min(this.failures, BACKOFF_MS.length - 1);
    this.failures += 1;
    const base = BACKOFF_MS[idx]!;
    const jitter = Math.floor(Math.random() * MAX_JITTER_MS);
    if (this.reconnectTimer !== null) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      if (this.closedByUser) return;
      this.connect();
    }, base + jitter);
  }

  private handleChunk(m: Record<string, unknown>): void {
    const id = typeof m.snapshot_id === "string" ? m.snapshot_id : "";
    const idx = typeof m.index === "number" ? m.index : -1;
    const total = typeof m.total === "number" ? m.total : 0;
    const chunk = typeof m.chunk === "string" ? m.chunk : "";
    if (!id || idx < 0 || total <= 0 || idx >= total) return;

    let buf = this.chunks.get(id);
    if (!buf) {
      buf = { total, parts: new Map() };
      this.chunks.set(id, buf);
    } else if (buf.total !== total) {
      // Total changed mid-stream — drop the buffer and restart.
      this.chunks.delete(id);
      buf = { total, parts: new Map() };
      this.chunks.set(id, buf);
    }
    buf.parts.set(idx, chunk);

    if (buf.parts.size === buf.total) {
      const ordered: string[] = [];
      for (let i = 0; i < buf.total; i++) {
        ordered.push(buf.parts.get(i) ?? "");
      }
      this.chunks.delete(id);
      let reassembled: unknown;
      try {
        reassembled = JSON.parse(ordered.join(""));
      } catch {
        // Malformed chunk reassembly — drop silently. (A future SystemEvent
        // could surface this to the operator.)
        return;
      }
      if (reassembled && typeof reassembled === "object") {
        this.opts.onMessage(reassembled as object);
      }
    }
  }
}

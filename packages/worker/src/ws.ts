import WebSocket from "ws";

export interface HelloPayload {
  type: "hello";
  v: 1;
  agent_id: string;
  startup_id: string;
  secret: string;
}

export interface WorkerHandle {
  send(msg: object): void;
  onMessage(fn: (m: unknown) => void): void;
  offMessage(fn: (m: unknown) => void): void;
  close(): void;
  /** For tests / diagnostics. */
  listenerCount(): number;
}

export interface ConnectOpts {
  url: string;
  agentId: string;
  startupId: string;
  secret: string;
  /** ms; defaults to 5000 */
  helloTimeoutMs?: number;
  /**
   * Fired ONCE after a successful handshake when the underlying WS closes
   * (world disconnected, network drop, etc.). Pre-ack closes still reject the
   * `connect()` promise with `ws_closed_before_ack` and do NOT fire `onClose`.
   */
  onClose?: () => void;
}

/**
 * Connect to /ws/worker, send hello, wait for the auth-ack reply, return a WorkerHandle.
 * Rejects if the world rejects auth or the WS closes before ack.
 */
export async function connect(opts: ConnectOpts): Promise<WorkerHandle> {
  const ws = new WebSocket(opts.url);
  const listeners = new Set<(m: unknown) => void>();

  ws.on("message", (data: WebSocket.RawData) => {
    let parsed: unknown;
    try {
      parsed = JSON.parse(data.toString());
    } catch {
      return;
    }
    for (const fn of listeners) fn(parsed);
  });

  await new Promise<void>((resolve, reject) => {
    ws.once("open", () => resolve());
    ws.once("error", (e) => reject(e));
  });

  const hello: HelloPayload = {
    type: "hello",
    v: 1,
    agent_id: opts.agentId,
    startup_id: opts.startupId,
    secret: opts.secret,
  };

  // Send hello, wait for ack OR close-on-auth-fail.
  await new Promise<void>((resolve, reject) => {
    const timeoutMs = opts.helloTimeoutMs ?? 5_000;
    const t = setTimeout(() => {
      cleanup();
      reject(new Error("hello_ack_timeout"));
    }, timeoutMs);

    const onMsg = (m: unknown) => {
      const o = m as { type?: string };
      if (o?.type === "ok" || o?.type === "hello_ack") {
        cleanup();
        resolve();
      } else if (o?.type === "error" || o?.type === "auth_error") {
        cleanup();
        reject(new Error(`auth_rejected: ${JSON.stringify(o)}`));
      }
    };
    const onClose = () => {
      cleanup();
      reject(new Error("ws_closed_before_ack"));
    };
    const cleanup = () => {
      clearTimeout(t);
      listeners.delete(onMsg);
      ws.off("close", onClose);
    };

    listeners.add(onMsg);
    ws.on("close", onClose);
    ws.send(JSON.stringify(hello));
  });

  // Post-handshake close handler — surface the disconnect to the caller via
  // `opts.onClose` so `main()` can resolve its keep-alive promise and let the
  // supervisor see an exit code. We register only AFTER the hello-ack path has
  // already cleaned up its own pre-ack `onClose`, so this fires at most once.
  if (opts.onClose) {
    let fired = false;
    ws.on("close", () => {
      if (fired) return;
      fired = true;
      opts.onClose?.();
    });
  }

  return {
    send(msg) {
      ws.send(JSON.stringify(msg));
    },
    onMessage(fn) {
      listeners.add(fn);
    },
    offMessage(fn) {
      listeners.delete(fn);
    },
    close() {
      ws.close();
    },
    listenerCount() {
      return listeners.size;
    },
  };
}

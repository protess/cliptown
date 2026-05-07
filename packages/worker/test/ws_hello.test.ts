import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { WebSocketServer } from "ws";
import { connect } from "../src/ws.js";

describe("worker WS hello + auth", () => {
  let server: WebSocketServer;
  let port: number;

  beforeEach(async () => {
    server = new WebSocketServer({ port: 0 });
    await new Promise<void>((resolve) =>
      server.once("listening", () => resolve()),
    );
    port = (server.address() as { port: number }).port;
  });

  afterEach(async () => {
    for (const client of server.clients) client.terminate();
    await new Promise<void>((resolve) => server.close(() => resolve()));
  });

  it("sends hello and resolves on ok ack", async () => {
    let receivedHello: unknown = null;
    server.on("connection", (ws) => {
      ws.on("message", (data) => {
        receivedHello = JSON.parse(data.toString());
        ws.send(JSON.stringify({ type: "ok", kind: "hello" }));
      });
    });

    const handle = await connect({
      url: `ws://127.0.0.1:${port}`,
      agentId: "a1",
      startupId: "s1",
      secret: "test-secret",
    });

    expect(receivedHello).toMatchObject({
      type: "hello",
      v: 1,
      agent_id: "a1",
      startup_id: "s1",
      secret: "test-secret",
    });
    handle.close();
  });

  it("rejects on auth error reply", async () => {
    server.on("connection", (ws) => {
      ws.on("message", () => {
        ws.send(JSON.stringify({ type: "error", reason: "bad_secret" }));
      });
    });

    await expect(
      connect({
        url: `ws://127.0.0.1:${port}`,
        agentId: "a1",
        startupId: "s1",
        secret: "wrong",
      }),
    ).rejects.toThrow(/auth_rejected/);
  });

  it("rejects when server closes before ack", async () => {
    server.on("connection", (ws) => {
      ws.close();
    });
    await expect(
      connect({
        url: `ws://127.0.0.1:${port}`,
        agentId: "a1",
        startupId: "s1",
        secret: "x",
      }),
    ).rejects.toThrow(/ws_closed_before_ack|auth_rejected/);
  });

  it("times out if no ack arrives", async () => {
    server.on("connection", () => {
      /* swallow hello, never reply */
    });
    await expect(
      connect({
        url: `ws://127.0.0.1:${port}`,
        agentId: "a1",
        startupId: "s1",
        secret: "x",
        helloTimeoutMs: 100,
      }),
    ).rejects.toThrow(/hello_ack_timeout|ws_closed_before_ack/);
  });

  it("fires onClose when the server closes the socket post-handshake", async () => {
    let serverWs: import("ws").WebSocket | null = null;
    server.on("connection", (ws) => {
      serverWs = ws;
      ws.on("message", () => {
        ws.send(JSON.stringify({ type: "ok", kind: "hello" }));
      });
    });

    const closed = new Promise<void>((resolve) => {
      void connect({
        url: `ws://127.0.0.1:${port}`,
        agentId: "a1",
        startupId: "s1",
        secret: "x",
        onClose: () => resolve(),
      }).then(() => {
        // After handshake completes, simulate a world-side disconnect.
        serverWs?.close();
      });
    });

    // If onClose never fires, this test will time out. 1s is generous.
    await Promise.race([
      closed,
      new Promise<never>((_, reject) =>
        setTimeout(() => reject(new Error("onClose did not fire")), 1_000),
      ),
    ]);
  });
});

import { describe, it, expect } from "vitest";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { subscribeSse } from "../src/sse_client.js";

interface ServerHandle {
  server: Server;
  port: number;
  push: (frame: string) => void;
  end: () => void;
  close: () => Promise<void>;
}

async function startServer(): Promise<ServerHandle> {
  let writer: { write: (s: string) => void; end: () => void } | null = null;
  const server = createServer((req, res) => {
    if (req.url !== "/event") {
      res.writeHead(404);
      res.end();
      return;
    }
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
    });
    writer = {
      write: (s) => res.write(s),
      end: () => res.end(),
    };
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", () => resolve()));
  const port = (server.address() as AddressInfo).port;
  return {
    server,
    port,
    push: (frame) => { writer?.write(frame); },
    end: () => { writer?.end(); },
    async close() {
      await new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}

describe("sse_client", () => {
  it("yields parsed JSON events from data: frames", async () => {
    const h = await startServer();
    const ctrl = new AbortController();
    const got: unknown[] = [];
    const sub = (async () => {
      for await (const evt of subscribeSse(`http://127.0.0.1:${h.port}/event`, ctrl.signal)) {
        got.push(evt);
        if (got.length === 2) ctrl.abort();
      }
    })();
    // Push two frames, then close.
    await new Promise((r) => setTimeout(r, 50));
    h.push(`data: {"type":"server.connected"}\n\n`);
    h.push(`data: {"type":"session.idle"}\n\n`);
    await sub;
    expect(got).toEqual([
      { type: "server.connected" },
      { type: "session.idle" },
    ]);
    await h.close();
  });

  it("handles a frame split across two TCP chunks", async () => {
    const h = await startServer();
    const ctrl = new AbortController();
    const got: unknown[] = [];
    const sub = (async () => {
      for await (const evt of subscribeSse(`http://127.0.0.1:${h.port}/event`, ctrl.signal)) {
        got.push(evt);
        if (got.length === 1) ctrl.abort();
      }
    })();
    await new Promise((r) => setTimeout(r, 50));
    h.push(`data: {"type":"sess`);
    await new Promise((r) => setTimeout(r, 30));
    h.push(`ion.idle"}\n\n`);
    await sub;
    expect(got).toEqual([{ type: "session.idle" }]);
    await h.close();
  });

  it("skips malformed JSON without crashing", async () => {
    const h = await startServer();
    const ctrl = new AbortController();
    const got: unknown[] = [];
    const sub = (async () => {
      for await (const evt of subscribeSse(`http://127.0.0.1:${h.port}/event`, ctrl.signal)) {
        got.push(evt);
        if (got.length === 1) ctrl.abort();
      }
    })();
    await new Promise((r) => setTimeout(r, 50));
    h.push(`data: not json\n\n`);
    h.push(`data: {"type":"session.idle"}\n\n`);
    await sub;
    expect(got).toEqual([{ type: "session.idle" }]);
    await h.close();
  });
});

import { describe, it, expect } from "vitest";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { fetchSkillsForAgent } from "../src/skills_client.js";

interface Handle {
  url: string;
  close: () => Promise<void>;
}

async function startServer(
  handler: (req: import("node:http").IncomingMessage, res: import("node:http").ServerResponse) => void,
): Promise<Handle> {
  const server: Server = createServer(handler);
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", () => r()));
  const port = (server.address() as AddressInfo).port;
  return {
    url: `http://127.0.0.1:${port}`,
    async close() {
      await new Promise<void>((r) => server.close(() => r()));
    },
  };
}

describe("fetchSkillsForAgent", () => {
  it("returns parsed list on 200", async () => {
    const h = await startServer((req, res) => {
      if (req.method === "GET" && req.url === "/api/agents/A1/skills" &&
          req.headers.authorization === "Bearer A1:sec1") {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({ skills: [{ name: "deploy", content_md: "body" }] }));
        return;
      }
      res.writeHead(500); res.end();
    });
    const skills = await fetchSkillsForAgent(h.url, "A1", "sec1");
    expect(skills).toEqual([{ name: "deploy", content_md: "body" }]);
    await h.close();
  });

  it("returns [] when the agent has no attached skills (200 + empty array)", async () => {
    const h = await startServer((_req, res) => {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify({ skills: [] }));
    });
    const skills = await fetchSkillsForAgent(h.url, "A1", "sec1");
    expect(skills).toEqual([]);
    await h.close();
  });

  it("throws on non-2xx", async () => {
    const h = await startServer((_req, res) => {
      res.writeHead(401, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: "unauthorized" }));
    });
    await expect(fetchSkillsForAgent(h.url, "A1", "sec1")).rejects.toThrow(/status=401/);
    await h.close();
  });
});

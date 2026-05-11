import { spawn as nodeSpawn, type ChildProcess } from "node:child_process";

/**
 * Spawns `opencode serve --port 0 --pure --hostname 127.0.0.1` and waits
 * for its "listening on http://127.0.0.1:<port>" log line on stderr to
 * capture the chosen port. Returns a handle the adapter uses to read
 * the URL, await the process exit, and terminate it.
 *
 * Why this lives in its own module:
 *   - The "wait for listening line" dance is the most fragile part of
 *     the opencode adapter; isolating it makes the index.ts orchestration
 *     readable and the lifecycle independently swappable if opencode
 *     adds a /ready endpoint or changes its log format.
 *   - Keeping the child process reference inside lets the adapter call
 *     `kill()` without keeping a `child` variable around in index.ts.
 */

export interface ServeHandle {
  /** Base URL like `http://127.0.0.1:54321` (no trailing slash). */
  url: string;
  /** Resolves when the server process exits. */
  exit: Promise<{ exit_code: number; signal?: string }>;
  /** SIGTERM the server child. Idempotent. */
  kill(signal?: NodeJS.Signals): void;
  /** The underlying child for log forwarding. */
  child: ChildProcess;
}

export interface StartServeOpts {
  bin: string;
  /** Working dir for the child (does not affect listening port). */
  cwd: string;
  /** Extra env merged onto inherited env. */
  env?: NodeJS.ProcessEnv;
  /** Max ms to wait for the listening line. Default 15000. */
  readyTimeoutMs?: number;
  /** Forwarded so callers can tee stderr to operator logs. */
  onLog?: (stream: "stdout" | "stderr", line: string) => void;
}

const LISTENING_RE = /opencode server listening on (http:\/\/[^\s]+)/;

export async function startServe(opts: StartServeOpts): Promise<ServeHandle> {
  const readyMs = opts.readyTimeoutMs ?? 15000;
  const env = { ...process.env, ...opts.env };
  const child = nodeSpawn(
    opts.bin,
    ["serve", "--port", "0", "--pure", "--hostname", "127.0.0.1", "--print-logs"],
    { cwd: opts.cwd, env, stdio: ["ignore", "pipe", "pipe"] },
  );

  const exit = new Promise<{ exit_code: number; signal?: string }>((resolve) => {
    child.on("exit", (code, signal) => {
      resolve({ exit_code: code ?? -1, signal: signal ?? undefined });
    });
  });

  const url = await new Promise<string>((resolve, reject) => {
    let stderrBuf = "";
    let settled = false;
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      reject(new Error(`opencode serve did not announce listening URL within ${readyMs}ms`));
    }, readyMs);

    const onChunk = (b: Buffer) => {
      const s = b.toString("utf-8");
      opts.onLog?.("stderr", s);
      stderrBuf += s;
      const m = LISTENING_RE.exec(stderrBuf);
      if (m && !settled) {
        settled = true;
        clearTimeout(timer);
        resolve(m[1].replace(/\/$/, ""));
      }
    };
    child.stderr?.on("data", onChunk);
    child.stdout?.on("data", (b: Buffer) => opts.onLog?.("stdout", b.toString("utf-8")));
    child.on("exit", (code) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      reject(new Error(`opencode serve exited before announcing listening URL (code=${code})`));
    });
  });

  return {
    url,
    exit,
    kill(signal: NodeJS.Signals = "SIGTERM") {
      try { child.kill(signal); } catch { /* noop */ }
    },
    child,
  };
}

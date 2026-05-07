import type { BackendAdapter, HookEvent, SpawnOpts, SpawnResult } from "@cliptown/adapter-core";

/**
 * Manages a single BackendAdapter session — spawns the CLI, watches for
 * unexpected exit, and respawns with exponential backoff. After three
 * consecutive failures the supervisor emits `onDead` and stops trying.
 *
 * Backoff: [1s, 5s, 30s]. An exit_code of 0 is "expected" — the supervisor
 * does NOT respawn on clean exit.
 */

export interface SupervisorOpts {
  adapter: BackendAdapter;
  spawnOpts: SpawnOpts;
  /**
   * Called when the supervisor gives up (3 consecutive failed spawns).
   * The worker should escalate this to the world via `system_event`.
   */
  onDead?: (info: { attempts: number; last_error?: string }) => void;
  /** Hook callback forwarded into spawnOpts.onHook. */
  onHook?: (e: HookEvent) => void;
  /** For tests: override backoff schedule. */
  backoffMs?: ReadonlyArray<number>;
  /** For tests: override "now" / sleep. */
  sleep?: (ms: number) => Promise<void>;
}

const DEFAULT_BACKOFF_MS: ReadonlyArray<number> = [1_000, 5_000, 30_000];

const defaultSleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

export class Supervisor {
  private adapter: BackendAdapter;
  private spawnOpts: SpawnOpts;
  private onDead: (info: { attempts: number; last_error?: string }) => void;
  private onHook: (e: HookEvent) => void;
  private backoff: ReadonlyArray<number>;
  private sleep: (ms: number) => Promise<void>;

  private current: SpawnResult | null = null;
  private stopped = false;
  private failureCount = 0;
  private lastError: string | undefined;
  /** Resolves when the supervisor terminates (clean exit OR dead). */
  private done: Promise<void> | null = null;

  constructor(opts: SupervisorOpts) {
    this.adapter = opts.adapter;
    this.spawnOpts = opts.spawnOpts;
    this.onDead = opts.onDead ?? (() => { /* noop */ });
    this.onHook = opts.onHook ?? (() => { /* noop */ });
    this.backoff = opts.backoffMs ?? DEFAULT_BACKOFF_MS;
    this.sleep = opts.sleep ?? defaultSleep;
  }

  /** Start the supervised session. Resolves when the session terminates. */
  start(): Promise<void> {
    if (this.done) return this.done;
    this.done = (async () => {
      while (!this.stopped) {
        let exit: { exit_code: number; signal?: string };
        try {
          this.current = await this.adapter.spawn({
            ...this.spawnOpts,
            onHook: this.onHook,
            onLog: this.spawnOpts.onLog,
          });
          exit = await this.current.wait();
          this.current = null;
        } catch (e) {
          this.lastError = e instanceof Error ? e.message : String(e);
          exit = { exit_code: -1 };
        }

        if (this.stopped) break;
        if (exit.exit_code === 0 && !exit.signal) {
          // Clean exit — supervisor is done.
          return;
        }
        // Failure path.
        this.failureCount += 1;
        this.lastError =
          this.lastError ??
          `exit_code=${exit.exit_code}${exit.signal ? ` signal=${exit.signal}` : ""}`;
        if (this.failureCount > this.backoff.length) {
          this.onDead({ attempts: this.failureCount, last_error: this.lastError });
          return;
        }
        const delay = this.backoff[this.failureCount - 1];
        await this.sleep(delay);
        this.lastError = undefined; // reset; next spawn either succeeds or sets a new one
      }
    })();
    return this.done;
  }

  /** Signal the supervisor to stop respawning. Kills any running child. */
  async stop(): Promise<void> {
    this.stopped = true;
    if (this.current) {
      this.current.kill("SIGTERM");
    }
    if (this.done) await this.done;
  }

  /** For tests / diagnostics. */
  state(): { failures: number; running: boolean; stopped: boolean; last_error?: string } {
    return {
      failures: this.failureCount,
      running: this.current !== null,
      stopped: this.stopped,
      last_error: this.lastError,
    };
  }
}

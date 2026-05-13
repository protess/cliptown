import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { mkdtemp, rm, lstat, realpath, readFile, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { prepareWorkdir } from "../src/execenv.js";

describe("prepareWorkdir", () => {
  let root: string;

  beforeEach(async () => {
    root = await mkdtemp(join(tmpdir(), "ct-execenv-"));
  });

  afterEach(async () => {
    await rm(root, { recursive: true, force: true });
  });

  it("creates the workdir hierarchy at <root>/workspaces/<sid>/<tid>/workdir/", async () => {
    const workdir = await prepareWorkdir({
      workspacesRoot: root,
      startupId: "s1",
      taskId: "t1",
      agentId: "a1",
    });
    const expected = resolve(root, "workspaces", "s1", "t1", "workdir");
    expect(workdir).toBe(expected);
    const st = await stat(workdir);
    expect(st.isDirectory()).toBe(true);
  });

  it("creates the workspaces symlink resolving to <root>/workspaces", async () => {
    const workdir = await prepareWorkdir({
      workspacesRoot: root,
      startupId: "s1",
      taskId: "t1",
      agentId: "a1",
    });
    const linkPath = join(workdir, "workspaces");
    const lst = await lstat(linkPath);
    expect(lst.isSymbolicLink()).toBe(true);
    const target = await realpath(linkPath);
    expect(target).toBe(await realpath(resolve(root, "workspaces")));
  });

  it("writes CLAUDE.md with agent/task/startup context + canonical artifact path", async () => {
    const workdir = await prepareWorkdir({
      workspacesRoot: root,
      startupId: "s1",
      taskId: "t1",
      agentId: "a1",
    });
    const content = await readFile(join(workdir, "CLAUDE.md"), "utf-8");
    expect(content).toContain("a1");
    expect(content).toContain("t1");
    expect(content).toContain("s1");
    expect(content).toContain("workspaces/s1/artifacts/t1.md");
  });

  it("is idempotent — second call with same inputs does not throw", async () => {
    const first = await prepareWorkdir({
      workspacesRoot: root,
      startupId: "s1",
      taskId: "t1",
      agentId: "a1",
    });
    const second = await prepareWorkdir({
      workspacesRoot: root,
      startupId: "s1",
      taskId: "t1",
      agentId: "a1",
    });
    expect(second).toBe(first);
    const lst = await lstat(join(second, "workspaces"));
    expect(lst.isSymbolicLink()).toBe(true);
  });
});

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

  it("writes attached skills as <workdir>/skills/<name>.md and lists them in CLAUDE.md", async () => {
    const workdir = await prepareWorkdir({
      workspacesRoot: root,
      startupId: "s1",
      taskId: "t1",
      agentId: "a1",
      skills: [
        { name: "deploy-to-fly", content_md: "deploy steps" },
        { name: "read-logs", content_md: "log locations" },
      ],
    });
    const deploy = await readFile(join(workdir, "skills", "deploy-to-fly.md"), "utf-8");
    const logs = await readFile(join(workdir, "skills", "read-logs.md"), "utf-8");
    expect(deploy).toBe("deploy steps");
    expect(logs).toBe("log locations");
    const claudeMd = await readFile(join(workdir, "CLAUDE.md"), "utf-8");
    expect(claudeMd).toContain("## Available skills");
    expect(claudeMd).toContain("deploy-to-fly");
    expect(claudeMd).toContain("./skills/deploy-to-fly.md");
    expect(claudeMd).toContain("read-logs");
  });

  it("omits skills section and skills dir when skills is empty or absent", async () => {
    const workdir = await prepareWorkdir({
      workspacesRoot: root,
      startupId: "s1",
      taskId: "t1",
      agentId: "a1",
      skills: [],
    });
    const claudeMd = await readFile(join(workdir, "CLAUDE.md"), "utf-8");
    expect(claudeMd).not.toContain("## Available skills");
    await expect(stat(join(workdir, "skills"))).rejects.toThrow();
  });
});

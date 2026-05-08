import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { resolveSandbox } from "../src/sandbox";
import * as fs from "node:fs";
import * as path from "node:path";
import * as os from "node:os";

let root: string;

beforeEach(() => {
  root = fs.mkdtempSync(path.join(os.tmpdir(), "ct-sb-"));
  fs.mkdirSync(path.join(root, "artifacts"), { recursive: true });
});
afterEach(() => {
  fs.rmSync(root, { recursive: true, force: true });
});

describe("sandbox attacks", () => {
  it("rejects empty", () => {
    expect(() => resolveSandbox(root, "")).toThrow(/empty/);
  });
  it("rejects dot-dot escape", () => {
    expect(() => resolveSandbox(root, "../etc/passwd")).toThrow();
    expect(() => resolveSandbox(root, "../../etc/passwd")).toThrow();
  });
  it("rejects unix absolute", () => {
    expect(() => resolveSandbox(root, "/etc/passwd")).toThrow(/absolute/);
  });
  it("rejects windows drive letter", () => {
    expect(() => resolveSandbox(root, "C:\\Windows\\System32")).toThrow(/windows/);
  });
  it("rejects UNC", () => {
    expect(() => resolveSandbox(root, "\\\\server\\share")).toThrow(/windows/);
  });
  it("rejects nul byte", () => {
    expect(() => resolveSandbox(root, "artifacts/foo\0.md")).toThrow(/nul/);
  });
  it("rejects too long", () => {
    expect(() => resolveSandbox(root, "a".repeat(5000))).toThrow(/too long/);
  });
  it("rejects bidi RTL override", () => {
    expect(() => resolveSandbox(root, `artifacts/foo‮gpj.exe`)).toThrow(/bidi/);
  });
  it("rejects non-NFC", () => {
    // Decomposed e + combining acute (NFD) — NFC form is precomposed é
    expect(() => resolveSandbox(root, "artifacts/cafe\u0301.md")).toThrow(/non-NFC/);
  });
  it("rejects trailing dot", () => {
    expect(() => resolveSandbox(root, "artifacts/foo.")).toThrow(/trailing/);
  });
  it("rejects trailing space", () => {
    expect(() => resolveSandbox(root, "artifacts/foo ")).toThrow(/trailing/);
  });
  it("allows legit artifact path", () => {
    const r = resolveSandbox(root, "artifacts/T1.md");
    expect(r.startsWith(fs.realpathSync(root))).toBe(true);
  });
});

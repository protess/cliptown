import * as path from "node:path";
import * as fs from "node:fs";

/**
 * Sandbox path resolver. Mirrors the Rust crates/world/src/sandbox.rs ruleset.
 * Used by the worker's pre_tool hook (M3+) to vet every file write/read by the CLI.
 */
export function resolveSandbox(root: string, candidate: string): string {
  if (!candidate) throw new Error("empty path");
  if (candidate.includes("\0")) throw new Error("nul byte");
  if (candidate.length > 4096) throw new Error("too long");
  if (path.isAbsolute(candidate)) throw new Error("absolute forbidden");
  if (/^[A-Za-z]:/.test(candidate) || candidate.startsWith("\\\\") || candidate.startsWith("//")) {
    throw new Error("windows abs");
  }
  // Bidi controls: U+202A..U+202E, U+200E..U+200F
  if (/[‪-‮‎‏]/.test(candidate)) throw new Error("bidi control");
  if (candidate.normalize("NFC") !== candidate) throw new Error("non-NFC");
  if (candidate.endsWith(".") || candidate.endsWith(" ")) throw new Error("trailing dot/space");

  const joined = path.join(root, candidate);
  const realRoot = fs.realpathSync(root);
  let real: string;
  try {
    real = fs.realpathSync(joined);
  } catch {
    const parent = path.dirname(joined);
    const parentReal = fs.realpathSync(parent);
    if (!parentReal.startsWith(realRoot + path.sep) && parentReal !== realRoot) {
      throw new Error("parent escapes root");
    }
    real = path.join(parentReal, path.basename(joined));
  }
  if (!real.startsWith(realRoot + path.sep) && real !== realRoot) {
    throw new Error("escapes root");
  }
  return real;
}

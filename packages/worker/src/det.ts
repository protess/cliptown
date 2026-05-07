export interface Clock { nowUnix(): number; }
export interface Randomness { nextU32(): number; }
export interface UuidGen { new(): string; }
export interface DetCtx { clock: Clock; random: Randomness; uuid: UuidGen; }
export function prodCtx(): DetCtx {
  return {
    clock: { nowUnix: () => Math.floor(Date.now() / 1000) },
    random: { nextU32: () => Math.floor(Math.random() * 0x1_0000_0000) },
    uuid: { new: () => crypto.randomUUID() },
  };
}
export function testCtx(t0: number, seed: number): DetCtx {
  let t = t0; let r = seed >>> 0; let u = 0;
  return {
    clock: { nowUnix: () => t },
    random: { nextU32: () => { r ^= r << 13; r ^= r >>> 17; r ^= r << 5; return r >>> 0; } },
    uuid: { new: () => `00000000-0000-0000-0000-${(u++).toString(16).padStart(12, "0")}` },
  };
}

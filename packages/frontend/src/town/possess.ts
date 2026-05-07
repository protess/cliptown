/**
 * Possess transition utilities. Handles the visual "possess in" / "possess out"
 * sequence when the operator avatar (`__operator__`) enters or leaves
 * `state.avatars`. Pure module — no Pixi imports — so it stays unit-testable.
 *
 * Phase machine: idle → in → settled → out → idle.
 *   - idle:    operator is absent; alpha = 0, scale = 1.
 *   - in:      operator just appeared; alpha eases 0 → 1, scale eases 1 → 1.08 → 1.
 *   - settled: steady-state possessed; alpha = 1, scale = 1.
 *   - out:     operator just disappeared; alpha eases 1 → 0, scale eases 1 → 1.08 → 1.
 *
 * The settled state is a sticky midpoint: PixiStage holds the operator sprite
 * at full opacity until unpossess flips the lifecycle to "out". When "out"
 * progress reaches 1, callers transition to "idle" and remove the sprite.
 */

export const POSSESS_DURATION_MS = 400;

export type Phase = "idle" | "in" | "settled" | "out";

export interface PossessState {
  phase: Phase;
  /** Wall-clock ms when the current phase started. */
  startMs: number;
}

export function initialState(): PossessState {
  return { phase: "idle", startMs: 0 };
}

export function transitionTo(
  prev: PossessState,
  phase: Phase,
  nowMs: number,
): PossessState {
  if (prev.phase === phase) return prev;
  return { phase, startMs: nowMs };
}

/** Ease in-out cubic. */
export function easeInOut(t: number): number {
  if (t <= 0) return 0;
  if (t >= 1) return 1;
  return t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
}

/** Linear progress in [0, 1] over POSSESS_DURATION_MS. */
export function progress(state: PossessState, nowMs: number): number {
  if (state.phase === "idle" || state.phase === "settled") {
    // Static phases don't have a meaningful progress; callers shouldn't rely
    // on it but we return 1 (fully-resolved) as the safe default.
    return 1;
  }
  const t = (nowMs - state.startMs) / POSSESS_DURATION_MS;
  return Math.max(0, Math.min(1, t));
}

/**
 * Compute the operator avatar alpha:
 *   idle    → 0 (not possessing)
 *   in      → easeInOut(progress)  (0 → 1)
 *   settled → 1 (steady-state possessed)
 *   out     → 1 - easeInOut(progress)  (1 → 0)
 */
export function operatorAlpha(state: PossessState, nowMs: number): number {
  switch (state.phase) {
    case "idle":    return 0;
    case "in":      return easeInOut(progress(state, nowMs));
    case "settled": return 1;
    case "out":     return 1 - easeInOut(progress(state, nowMs));
  }
}

/**
 * Camera scale during transition. 1 → 1.08 → 1 over the transition window;
 * 1.0 in idle and settled phases.
 */
export function cameraScale(state: PossessState, nowMs: number): number {
  if (state.phase === "idle" || state.phase === "settled") return 1;
  const t = progress(state, nowMs);
  // Symmetric peak at t=0.5 — sin(π·t) gives 0 at the endpoints, 1 at the
  // midpoint, so the camera lifts and settles within the transition window.
  const peak = 0.08;
  return 1 + peak * Math.sin(Math.PI * t);
}

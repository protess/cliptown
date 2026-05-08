/**
 * Avatar rendering helpers for the Pixi stage.
 *
 * Phase 0: avatars are rendered directly inside `PixiStage` (not as a separate
 * React tree), but this module owns the sprite construction, hue palette,
 * status overlays, and tick-interpolation math so the stage stays focused on
 * lifecycle wiring.
 */

import {
  Graphics,
  Text,
  Container,
  type FederatedPointerEvent,
} from "pixi.js";

const HUES = [
  0xE63946, 0xF4A261, 0xE9C46A, 0x2A9D8F,
  0x264653, 0xA663CC, 0xFF8FAB, 0x83A4D4,
] as const;

export function hueFor(id: string): number {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) | 0;
  return HUES[Math.abs(h) % HUES.length]!;
}

const BACKEND_RING: Record<string, number> = {
  claude_code: 0xFF8C42,
  codex: 0x4A7BC8,
  opencode: 0x6BAA75,
  operator: 0x1A1A1A,
};

export interface AvatarSnapshot {
  agent_id: string;
  startup_id: string;
  role: string;
  backend: string;
  current_pos: [number, number];
  target_pos: [number, number] | null;
  room_id: string;
  status: string;
}

export interface AvatarSprite {
  container: Container;
  agent_id: string;
  startup_id: string;
  /** Last seen position (tile coords). */
  prevPos: [number, number];
  /** Target position to interpolate toward (tile coords). */
  targetPos: [number, number];
  /** Tick at which the current movement started (performance.now ms). */
  startMs: number;
}

export function statusOverlayChar(status: string): string {
  if (status === "working") return "…"; // …
  if (status === "blocked") return "!";
  if (status === "paused") return "⏸"; // ⏸
  if (status === "done") return "✓"; // ✓
  return "";
}

export function statusOverlayColor(status: string): number {
  if (status === "working") return 0xE9C46A;
  if (status === "blocked") return 0xD62828;
  if (status === "paused") return 0xE69F00;
  if (status === "done") return 0x2A9D8F;
  return 0x000000;
}

const TILE = 20;
const RADIUS = TILE * 0.7;

export function buildAvatarSprite(
  data: AvatarSnapshot,
  onClick: (agentId: string, gx: number, gy: number) => void,
): AvatarSprite {
  const c = new Container();
  c.eventMode = "static";
  c.cursor = "pointer";
  c.on("pointerdown", (e: FederatedPointerEvent) => {
    e.stopPropagation();
    onClick(data.agent_id, e.global.x, e.global.y);
  });

  // Backend ring (outer)
  const ring = new Graphics();
  const ringColor = BACKEND_RING[data.backend] ?? 0x6B6B6B;
  ring.circle(0, 0, RADIUS + 2).fill(ringColor);
  c.addChild(ring);

  // Body circle (startup hue)
  const body = new Graphics();
  body.circle(0, 0, RADIUS).fill(hueFor(data.startup_id));
  c.addChild(body);

  // Monogram letter. Operator avatar is special-cased to a star instead of
  // the literal first char of `__operator__` (which renders as "_").
  const monoText =
    data.agent_id === "__operator__"
      ? "★"
      : (data.agent_id.slice(0, 1) || "?").toUpperCase();
  const mono = new Text({
    text: monoText,
    style: {
      fontFamily: "IBM Plex Sans",
      fontSize: 14,
      fill: 0xFFFFFF,
      fontWeight: "700",
    },
  });
  mono.anchor.set(0.5);
  c.addChild(mono);

  // Status overlay (top-right corner)
  const ovlChar = statusOverlayChar(data.status);
  if (ovlChar) {
    const ovl = new Text({
      text: ovlChar,
      style: {
        fontFamily: "IBM Plex Sans",
        fontSize: 12,
        fill: statusOverlayColor(data.status),
        fontWeight: "700",
      },
    });
    ovl.position.set(RADIUS - 4, -RADIUS - 4);
    c.addChild(ovl);
  }

  c.position.set(
    data.current_pos[0] * TILE + TILE / 2,
    data.current_pos[1] * TILE + TILE / 2,
  );

  return {
    container: c,
    agent_id: data.agent_id,
    startup_id: data.startup_id,
    prevPos: data.current_pos,
    targetPos: data.target_pos ?? data.current_pos,
    startMs: performance.now(),
  };
}

/** Update an existing sprite's interpolation targets when a new snapshot arrives. */
export function updateAvatarTargets(
  sprite: AvatarSprite,
  newCurrent: [number, number],
  newTarget: [number, number] | null,
): void {
  // The sprite's *visible* position becomes the new interpolation source so
  // mid-tick snapshot arrivals don't snap the avatar backward. We translate
  // the current pixel position back to tile units (subtracting the half-tile
  // centering offset baked in at construction).
  sprite.prevPos = [
    sprite.container.position.x / TILE - 0.5,
    sprite.container.position.y / TILE - 0.5,
  ];
  sprite.targetPos = newTarget ?? newCurrent;
  sprite.startMs = performance.now();
}

export const TICK_DURATION_MS = 1_000;

export function interpolatePosition(
  sprite: AvatarSprite,
  nowMs: number,
): [number, number] {
  const t = Math.min(1, (nowMs - sprite.startMs) / TICK_DURATION_MS);
  const x = sprite.prevPos[0] + (sprite.targetPos[0] - sprite.prevPos[0]) * t;
  const y = sprite.prevPos[1] + (sprite.targetPos[1] - sprite.prevPos[1]) * t;
  return [x, y];
}

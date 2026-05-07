/**
 * PixiStage — the live avatar canvas for a town.
 *
 * Phase 0:
 *   - 7 rooms drawn from the hardcoded layout (mirror of `default_town`).
 *   - Doors highlighted as small bright squares.
 *   - Avatars filtered to the current startup (+ the operator if they exist),
 *     interpolated smoothly toward their server-published `target_pos` over
 *     `TICK_DURATION_MS` (1s) at Pixi's ticker rate (≈60fps).
 *   - Click any avatar → invokes `onAvatarClick(agentId)` (M4.11 will mount a
 *     popover on top of this).
 */

import { useEffect, useRef } from "react";
import { Application, Graphics, Container } from "pixi.js";
import { useWorld } from "../hooks/useWorld.js";
import {
  ROOMS, DOORS, ROOM_COLORS, FLOOR_BORDER,
  TILE, TOWN_W, TOWN_H, roomRect,
} from "./layout.js";
import {
  buildAvatarSprite, updateAvatarTargets, interpolatePosition,
  type AvatarSnapshot, type AvatarSprite,
} from "./Avatars.js";

interface PixiStageProps {
  startupId: string;
  onAvatarClick?: (agentId: string, screenX: number, screenY: number) => void;
}

export function PixiStage({ startupId, onAvatarClick }: PixiStageProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const appRef = useRef<Application | null>(null);
  const spritesRef = useRef<Map<string, AvatarSprite>>(new Map());
  const avatarLayerRef = useRef<Container | null>(null);
  const onClickRef = useRef(onAvatarClick);
  const { state } = useWorld();

  // Keep latest click handler reachable from the (stable) Pixi closures.
  useEffect(() => {
    onClickRef.current = onAvatarClick;
  }, [onAvatarClick]);

  // Initialize Pixi once.
  useEffect(() => {
    let cancelled = false;
    const app = new Application();
    appRef.current = app;
    app
      .init({
        width: TOWN_W,
        height: TOWN_H,
        background: 0xFAFAFA,
        antialias: true,
        autoDensity: true,
        resolution: window.devicePixelRatio || 1,
      })
      .then(() => {
        if (cancelled) return;
        hostRef.current?.appendChild(app.canvas);
        drawTown(app);
        const layer = new Container();
        app.stage.addChild(layer);
        avatarLayerRef.current = layer;

        // Animation loop: lerp each avatar toward its target.
        app.ticker.add(() => {
          const now = performance.now();
          for (const s of spritesRef.current.values()) {
            const [tx, ty] = interpolatePosition(s, now);
            s.container.position.set(
              tx * TILE + TILE / 2,
              ty * TILE + TILE / 2,
            );
          }
        });
      })
      .catch(() => {
        // Pixi init failure is fatal for this view; surface via console so the
        // operator notices in dev. Phase 0 has no formal error UI for this.
        // eslint-disable-next-line no-console
        console.error("[PixiStage] Application.init failed");
      });
    return () => {
      cancelled = true;
      // Pixi 8: destroy the app + GPU resources. `texture: true` evicts
      // textures we created (the rooms/doors graphics).
      app.destroy(true, { children: true, texture: true });
      appRef.current = null;
      spritesRef.current.clear();
      avatarLayerRef.current = null;
    };
  }, []);

  // Sync avatar set whenever the store updates.
  useEffect(() => {
    const layer = avatarLayerRef.current;
    if (!layer) return;
    const ours = Object.values(state.avatars).filter(
      (a) => a.startup_id === startupId || a.agent_id === "__operator__",
    );
    const seen = new Set<string>();
    for (const raw of ours) {
      const a = normalizeAvatar(raw);
      seen.add(a.agent_id);
      const existing = spritesRef.current.get(a.agent_id);
      if (existing) {
        updateAvatarTargets(existing, a.current_pos, a.target_pos);
      } else {
        const sprite = buildAvatarSprite(a, (id, gx, gy) => {
          const cb = onClickRef.current;
          if (!cb) return;
          // Translate canvas-local Pixi coords to viewport coords so the
          // popover can position itself with `position: fixed`.
          const canvas = appRef.current?.canvas;
          const r = canvas?.getBoundingClientRect();
          cb(id, (r?.left ?? 0) + gx, (r?.top ?? 0) + gy);
        });
        spritesRef.current.set(a.agent_id, sprite);
        layer.addChild(sprite.container);
      }
    }
    // Remove stale avatars (despawned, switched startups, etc.).
    for (const [id, sprite] of spritesRef.current.entries()) {
      if (!seen.has(id)) {
        layer.removeChild(sprite.container);
        sprite.container.destroy({ children: true });
        spritesRef.current.delete(id);
      }
    }
  }, [state.avatars, startupId]);

  return (
    <div
      ref={hostRef}
      style={{ width: TOWN_W, height: TOWN_H, marginInline: "auto" }}
    />
  );
}

function drawTown(app: Application): void {
  const bg = new Graphics();
  bg.rect(0, 0, TOWN_W, TOWN_H).fill(0xFAFAFA);
  app.stage.addChild(bg);

  for (const r of ROOMS) {
    const rect = roomRect(r);
    const room = new Graphics();
    room
      .rect(rect.x, rect.y, rect.w, rect.h)
      .fill(ROOM_COLORS[r.type])
      .stroke({ color: FLOOR_BORDER, width: 1 });
    app.stage.addChild(room);
  }

  // Phase 0 simplification: explicit walls aren't drawn — the per-room border
  // gives enough visual separation. Doors are highlighted as small squares so
  // the operator can find the connections between rooms.
  for (const d of DOORS) {
    const door = new Graphics();
    door
      .rect(d.tile[0] * TILE, d.tile[1] * TILE, TILE, TILE)
      .fill(0xFAFAFA)
      .stroke({ color: 0x2A9D8F, width: 1 });
    app.stage.addChild(door);
  }
}

function normalizeAvatar(raw: unknown): AvatarSnapshot {
  const o = (raw ?? {}) as Record<string, unknown>;
  const cp = o.current_pos ?? [0, 0];
  const tp = o.target_pos ?? null;
  return {
    agent_id:    String(o.agent_id ?? ""),
    startup_id:  String(o.startup_id ?? ""),
    role:        String(o.role ?? ""),
    backend:     String(o.backend ?? ""),
    current_pos: posToTuple(cp),
    target_pos:  tp ? posToTuple(tp) : null,
    room_id:     String(o.room_id ?? ""),
    status:      String(o.status ?? ""),
  };
}

function posToTuple(p: unknown): [number, number] {
  if (Array.isArray(p) && p.length >= 2) return [Number(p[0]), Number(p[1])];
  if (p && typeof p === "object") {
    const o = p as Record<string, unknown>;
    if ("x" in o && "y" in o) return [Number(o.x), Number(o.y)];
  }
  return [0, 0];
}

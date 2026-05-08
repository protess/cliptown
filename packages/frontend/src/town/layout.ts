/**
 * Town layout constants — hardcoded mirror of `crates/world/src/seed.rs::default_town()`.
 * Phase 0: single town. M6 will plumb dynamic layouts through the protocol.
 */

export const TILE = 20; // px per tile

export interface RoomDef {
  id: string;
  /** (x, y, w, h) in tiles. */
  bounds: [number, number, number, number];
  /** Type for color theming. */
  type: "suite" | "lobby" | "cafe" | "library";
  /** Phase 0: optional owning startup id (for ownership tinting). */
  private_to_startup_id?: string | null;
}

export interface DoorDef {
  id: string;
  /** Tile coord. */
  tile: [number, number];
  a_room: string;
  b_room: string;
}

export const ROOMS: ReadonlyArray<RoomDef> = [
  { id: "suite_1", bounds: [0, 0, 7, 6], type: "suite" },
  { id: "suite_2", bounds: [0, 6, 7, 6], type: "suite" },
  { id: "suite_3", bounds: [33, 0, 7, 6], type: "suite" },
  { id: "suite_4", bounds: [33, 6, 7, 6], type: "suite" },
  { id: "lobby",   bounds: [7, 4, 26, 4], type: "lobby" },
  { id: "cafe",    bounds: [7, 0, 26, 4], type: "cafe" },
  { id: "library", bounds: [7, 8, 26, 4], type: "library" },
];

export const DOORS: ReadonlyArray<DoorDef> = [
  { id: "door_s1_lobby",      tile: [7, 4],  a_room: "suite_1", b_room: "lobby" },
  { id: "door_s2_lobby",      tile: [7, 7],  a_room: "suite_2", b_room: "lobby" },
  { id: "door_s3_lobby",      tile: [33, 4], a_room: "suite_3", b_room: "lobby" },
  { id: "door_s4_lobby",      tile: [33, 7], a_room: "suite_4", b_room: "lobby" },
  { id: "door_lobby_cafe",    tile: [20, 4], a_room: "lobby",   b_room: "cafe" },
  { id: "door_lobby_library", tile: [20, 8], a_room: "lobby",   b_room: "library" },
];

export const ROOM_COLORS: Record<RoomDef["type"], number> = {
  suite:   0xF4ECDD,
  lobby:   0xE8E4DD,
  cafe:    0xF7E5D5,
  library: 0xE0E5E8,
};

export const WALL_COLOR = 0x9E978D;
export const FLOOR_BORDER = 0xCCC4B8;

/** Pixel rect for a room. */
export function roomRect(r: RoomDef): { x: number; y: number; w: number; h: number } {
  const [x, y, w, h] = r.bounds;
  return { x: x * TILE, y: y * TILE, w: w * TILE, h: h * TILE };
}

/** Town pixel bounds. */
export const TOWN_W = 40 * TILE;
export const TOWN_H = 12 * TILE;

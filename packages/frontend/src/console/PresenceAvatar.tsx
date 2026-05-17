/**
 * PresenceAvatar — P5 Theme A.
 *
 * Tiny circular monogram for an online operator. Used in both the Sidebar
 * (per-startup focus indicator) and the TopBar (online list). Color is a
 * deterministic 8-hue hash on `operator_id` so the same operator reads
 * visually consistent across panels — a primitive that Theme B (per-
 * operator audit visibility) will reuse for chat author tinting.
 */
import type { CSSProperties } from "react";

const HUES = [
  "#E63946",
  "#F4A261",
  "#E9C46A",
  "#2A9D8F",
  "#264653",
  "#A663CC",
  "#FF8FAB",
  "#83A4D4",
] as const;

export function operatorHue(operatorId: string): string {
  let h = 0;
  for (let i = 0; i < operatorId.length; i++) {
    h = (h * 31 + operatorId.charCodeAt(i)) | 0;
  }
  return HUES[Math.abs(h) % HUES.length]!;
}

export function PresenceAvatar({
  operatorId,
  name,
  size = 18,
  title,
}: {
  operatorId: string;
  name: string;
  size?: number;
  title?: string;
}) {
  const mono = (name || operatorId).slice(0, 1).toUpperCase();
  const style: CSSProperties = {
    display: "inline-grid",
    placeItems: "center",
    width: size,
    height: size,
    borderRadius: "50%",
    background: operatorHue(operatorId),
    color: "white",
    fontSize: Math.max(9, Math.floor(size * 0.55)),
    fontWeight: 700,
    border: "1px solid var(--raised)",
    lineHeight: 1,
  };
  return (
    <span
      style={style}
      title={title ?? `${name} (${operatorId.slice(0, 8)})`}
      data-testid={`presence-avatar-${operatorId}`}
    >
      {mono}
    </span>
  );
}

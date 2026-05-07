/**
 * Sidebar: list of startups, sorted by `last_event_ts` desc, with a left-edge
 * hue accent (8-color palette, deterministic per id) and a hand-rolled FLIP
 * animation when rows reorder.
 *
 * Phase 0 / M4.4 — onSelect is optional; M4.7+ wires the click handler. No
 * framer-motion: FLIP runs in useLayoutEffect via Element.animate(). The
 * empty state hints at the "+ New Startup" affordance in the top bar.
 */

import { useLayoutEffect, useRef } from "react";
import type { CSSProperties } from "react";
import { useWorld } from "../hooks/useWorld.js";
import type { StartupVM } from "../store.js";

const HUES = [
  "#E63946", // red
  "#F4A261", // orange
  "#E9C46A", // yellow
  "#2A9D8F", // teal
  "#264653", // deep blue
  "#A663CC", // violet
  "#FF8FAB", // pink
  "#83A4D4", // sky
] as const;

function hueFor(id: string): string {
  let h = 0;
  for (let i = 0; i < id.length; i++) {
    h = (h * 31 + id.charCodeAt(i)) | 0;
  }
  return HUES[Math.abs(h) % HUES.length]!;
}

function recencyOf(s: StartupVM): number {
  return s.last_event_ts ?? 0;
}

export function Sidebar({
  selected,
  onSelect,
}: {
  selected?: string | null;
  onSelect?: (id: string) => void;
}) {
  const { state } = useWorld();
  const sorted = Object.values(state.startups)
    .slice()
    .sort((a, b) => recencyOf(b) - recencyOf(a));

  // FLIP: capture previous rects, then animate to new positions.
  const containerRef = useRef<HTMLDivElement | null>(null);
  const prevRectsRef = useRef<Map<string, DOMRect>>(new Map());

  useLayoutEffect(() => {
    if (!containerRef.current) return;
    const newRects = new Map<string, DOMRect>();
    const rows = containerRef.current.querySelectorAll<HTMLElement>(
      "[data-startup-id]",
    );
    rows.forEach((el) => {
      const id = el.dataset.startupId;
      if (!id) return;
      const rect = el.getBoundingClientRect();
      newRects.set(id, rect);
      const prev = prevRectsRef.current.get(id);
      if (prev) {
        const dy = prev.top - rect.top;
        if (dy !== 0) {
          el.animate(
            [
              { transform: `translateY(${dy}px)` },
              { transform: "translateY(0)" },
            ],
            { duration: 240, easing: "ease-out" },
          );
        }
      }
    });
    prevRectsRef.current = newRects;
  });

  if (sorted.length === 0) {
    return (
      <aside style={asideStyle} aria-label="startups">
        <div style={{ padding: 16, color: "var(--fg-secondary)" }}>
          <p style={{ margin: "0 0 8px" }}>No startups yet.</p>
          <p style={{ fontSize: 13, margin: 0 }}>
            ↑ Use <strong>+ New Startup</strong> in the top bar.
          </p>
        </div>
      </aside>
    );
  }

  return (
    <aside style={asideStyle} aria-label="startups">
      <div ref={containerRef} role="list">
        {sorted.map((s) => (
          <div
            key={s.id}
            data-startup-id={s.id}
            role="listitem"
            onClick={() => onSelect?.(s.id)}
            style={{
              display: "grid",
              gridTemplateColumns: "8px 1fr auto",
              alignItems: "center",
              padding: "10px 12px",
              borderBottom: "1px solid var(--border)",
              cursor: onSelect ? "pointer" : "default",
              gap: 10,
              background:
                selected === s.id ? "rgba(0,0,0,0.04)" : "transparent",
            }}
          >
            <span
              aria-hidden
              style={{
                width: 4,
                height: 32,
                borderRadius: 2,
                background: hueFor(s.id),
              }}
            />
            <div style={{ minWidth: 0 }}>
              <div
                style={{
                  fontWeight: 500,
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                }}
              >
                {s.name || s.id}
              </div>
              <div style={{ fontSize: 12, color: "var(--fg-secondary)" }}>
                {budgetLabel(s)}
              </div>
            </div>
            <code style={{ fontSize: 11, color: "var(--fg-secondary)" }}>
              {s.id}
            </code>
          </div>
        ))}
      </div>
    </aside>
  );
}

const asideStyle: CSSProperties = {
  width: 280,
  borderRight: "1px solid var(--border)",
  background: "var(--raised)",
  overflowY: "auto",
};

function budgetLabel(s: StartupVM): string {
  if (s.budget_cap_usd == null) return "—";
  const spent = s.budget_spent_usd ?? 0;
  return `$${spent.toFixed(2)} / $${s.budget_cap_usd.toFixed(2)}`;
}

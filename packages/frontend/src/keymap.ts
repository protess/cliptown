/**
 * Phase 0 / M4.13 — Global keyboard navigation per spec §3.5.
 *
 * Installs a single `keydown` listener at the App level. Routes vim-style
 * keystrokes to navigation, selection cycling, and component-level reactions
 * via custom events on `window` (avoids prop drilling for modal/popover
 * dismissal, chat open, possess toggle, and the new-startup modal).
 *
 * Keymap (spec §3.5):
 *   j / k        — cycle sidebar selection down/up (console route only)
 *   Enter        — open selected startup (console: no-op stand-in)
 *   Escape       — dismiss any open popover/modal/menu (cliptown:dismiss)
 *   t            — navigate to /town/:id of the currently selected startup
 *   p            — toggle Possess on /town/:id (cliptown:possess-toggle)
 *   c            — open chat panel (cliptown:chat-open)
 *   g c          — go to /console (vim-style two-key sequence, 1s window)
 *   /            — focus search/filter — Phase 0 stand-in: open New Startup
 *                  modal via cliptown:new-startup
 *
 * Suppressed when focus is in <input>, <textarea>, <select>, or any
 * contenteditable element. Modifier-bearing keys (meta/ctrl/alt) are ignored
 * so the browser's own shortcuts continue to work.
 */

import { useEffect, useRef } from "react";
import { useNavigate, useLocation } from "react-router-dom";
import { useWorld } from "./hooks/useWorld.js";

const G_PREFIX_WINDOW_MS = 1_000;

export const KEYMAP_EVENTS = {
  DISMISS: "cliptown:dismiss",
  POSSESS_TOGGLE: "cliptown:possess-toggle",
  CHAT_OPEN: "cliptown:chat-open",
  NEW_STARTUP: "cliptown:new-startup",
} as const;

function isEditable(el: EventTarget | null): boolean {
  if (!el || !(el instanceof Element)) return false;
  const tag = el.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  if ((el as HTMLElement).isContentEditable) return true;
  return false;
}

/**
 * No-render component that installs the global Phase-0 keymap. Place once at
 * the App root inside <BrowserRouter> + <WorldProvider>.
 */
export function KeymapManager(): null {
  useKeymap();
  return null;
}

/**
 * Hook form, in case a future surface wants to re-install the keymap with
 * different scoping. Idempotent at the install level (one window listener).
 */
export function useKeymap(): void {
  const navigate = useNavigate();
  const location = useLocation();
  const world = useWorld();
  const gPressedAtRef = useRef<number>(0);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Always allow Escape to bubble out as a dismiss intent, even from
      // inputs (modal close UX). Otherwise, suppress while typing.
      if (isEditable(e.target)) {
        if (e.key === "Escape") {
          window.dispatchEvent(new CustomEvent(KEYMAP_EVENTS.DISMISS));
        }
        return;
      }
      if (e.metaKey || e.ctrlKey || e.altKey) return;

      const now = performance.now();
      const inGPrefix = now - gPressedAtRef.current < G_PREFIX_WINDOW_MS;

      // Vim-style two-key sequence: "g c" → /console.
      if (inGPrefix && e.key === "c") {
        e.preventDefault();
        gPressedAtRef.current = 0;
        navigate("/console");
        return;
      }
      if (e.key === "g") {
        gPressedAtRef.current = now;
        return;
      }
      // Any other key resets the prefix window.
      gPressedAtRef.current = 0;

      const onConsole = location.pathname.startsWith("/console");
      const onTown = location.pathname.startsWith("/town");

      switch (e.key) {
        case "Escape": {
          window.dispatchEvent(new CustomEvent(KEYMAP_EVENTS.DISMISS));
          return;
        }
        case "j": {
          if (onConsole) {
            cycleStartup(world, +1);
            e.preventDefault();
          }
          return;
        }
        case "k": {
          if (onConsole) {
            cycleStartup(world, -1);
            e.preventDefault();
          }
          return;
        }
        case "Enter": {
          // Console: selection already populates the main pane. Town: no-op.
          // Reserved for future "focus first kanban card" behavior.
          return;
        }
        case "t": {
          if (world.selectedStartupId) {
            navigate(`/town/${world.selectedStartupId}`);
            e.preventDefault();
          }
          return;
        }
        case "p": {
          if (onTown) {
            window.dispatchEvent(
              new CustomEvent(KEYMAP_EVENTS.POSSESS_TOGGLE),
            );
            e.preventDefault();
          }
          return;
        }
        case "c": {
          window.dispatchEvent(new CustomEvent(KEYMAP_EVENTS.CHAT_OPEN));
          e.preventDefault();
          return;
        }
        case "/": {
          // Phase 0 stand-in for global search.
          window.dispatchEvent(new CustomEvent(KEYMAP_EVENTS.NEW_STARTUP));
          e.preventDefault();
          return;
        }
        default:
          return;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate, location, world]);
}

function cycleStartup(
  world: ReturnType<typeof useWorld>,
  delta: 1 | -1,
): void {
  const ids = Object.values(world.state.startups)
    .slice()
    .sort((a, b) => (b.last_event_ts ?? 0) - (a.last_event_ts ?? 0))
    .map((s) => s.id);
  if (ids.length === 0) return;
  const cur = world.selectedStartupId;
  const idx = cur ? ids.indexOf(cur) : -1;
  // From "no selection", j picks first / k picks last.
  if (idx < 0) {
    const first = delta === 1 ? ids[0] : ids[ids.length - 1];
    if (first) world.setSelectedStartupId(first);
    return;
  }
  const next = (idx + delta + ids.length) % ids.length;
  const nextId = ids[next];
  if (nextId) world.setSelectedStartupId(nextId);
}

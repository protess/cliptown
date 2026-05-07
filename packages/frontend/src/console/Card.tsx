/**
 * Kanban Card (M4.6): a draggable summary tile for one task.
 *
 * Layout: [stuck-bar | title + meta | assignee monogram]. The left bar is the
 * "stuck indicator" — amber after `stuck_warn_minutes` and red after
 * `stuck_alert_minutes` (defaults sourced from cliptown.toml [kanban]). The
 * monogram is rendered in a deterministic startup-hue circle; review-round
 * dot lights up after the first round and escalates color through round 3.
 *
 * Phase 0: TaskVM does not yet carry an `updated_at` per-status timestamp,
 * so the stuck indicator stays transparent until the world emits one. This
 * lets us ship the visual now and wire data later without UI churn.
 */

import type { DragEvent } from "react";
import type { TaskVM, AvatarVM } from "../store.js";

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

function hueFor(id: string): string {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) | 0;
  return HUES[Math.abs(h) % HUES.length]!;
}

const STUCK_AMBER_MIN = 5;
const STUCK_RED_MIN = 30;

export function Card({
  task,
  assignee,
  onDragStart,
}: {
  task: TaskVM;
  assignee: AvatarVM | undefined;
  onDragStart: (e: DragEvent, taskId: string, fromColumn: string) => void;
}) {
  const stuckColor = stuckIndicator(task);
  const monogramSrc =
    assignee?.agent_id ?? task.assignee_agent_id ?? "?";
  const mono = monogramSrc.slice(0, 1).toUpperCase();
  const hue = assignee ? hueFor(assignee.startup_id) : "var(--fg-secondary)";
  const reviewRound = (task as { review_round?: number }).review_round;

  return (
    <div
      draggable
      onDragStart={(e) => onDragStart(e, task.id, task.status)}
      onClick={() => {
        // M4.11 will replace this with an agent popover.
        // eslint-disable-next-line no-console
        console.log("[card] clicked task", task.id);
      }}
      style={{
        display: "grid",
        gridTemplateColumns: "4px 1fr auto",
        gap: 8,
        background: "var(--raised)",
        border: "1px solid var(--border)",
        borderRadius: 6,
        padding: "8px 10px",
        cursor: "grab",
        marginBottom: 6,
        userSelect: "none",
      }}
    >
      <span
        aria-hidden
        style={{ background: stuckColor, borderRadius: 2 }}
      />
      <div style={{ minWidth: 0 }}>
        <div
          style={{
            fontWeight: 500,
            fontSize: 13,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
        >
          {task.title}
        </div>
        <div
          style={{
            fontSize: 11,
            color: "var(--fg-secondary)",
            display: "flex",
            gap: 6,
            alignItems: "center",
          }}
        >
          <code>{task.id.slice(0, 6)}</code>
          {task.required_room && <span>· {task.required_room}</span>}
          <ReviewRoundDot round={reviewRound} />
        </div>
      </div>
      <span
        aria-hidden
        style={{
          width: 24,
          height: 24,
          borderRadius: "50%",
          background: hue,
          color: "white",
          display: "grid",
          placeItems: "center",
          fontSize: 12,
          fontWeight: 700,
        }}
        title={assignee ? assignee.role : "unassigned"}
      >
        {mono}
      </span>
    </div>
  );
}

function ReviewRoundDot({ round }: { round?: number }) {
  if (!round || round < 1) return null;
  const color =
    round >= 3 ? "#D62828" : round >= 2 ? "#E69F00" : "#E9C46A";
  return (
    <span
      aria-label={`review round ${round}`}
      style={{
        display: "inline-block",
        width: 6,
        height: 6,
        borderRadius: "50%",
        background: color,
      }}
    />
  );
}

function stuckIndicator(task: TaskVM): string {
  // Phase 0: TaskVM has no per-status timestamp. We optimistically read
  // `updated_at` (seconds since epoch) if the world starts emitting it; in
  // its absence the bar stays transparent.
  const updated = (task as { updated_at?: number }).updated_at;
  if (typeof updated !== "number") return "transparent";
  const ageMin = (Date.now() / 1000 - updated) / 60;
  if (ageMin >= STUCK_RED_MIN) return "#D62828";
  if (ageMin >= STUCK_AMBER_MIN) return "#E69F00";
  return "transparent";
}

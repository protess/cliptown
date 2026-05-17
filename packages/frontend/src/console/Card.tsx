/**
 * Kanban Card (M4.6): a draggable summary tile for one task.
 *
 * Layout: [stuck-bar | title + meta | assignee monogram]. The left bar is the
 * "stuck indicator" — amber after `stuck_warn_minutes`, red after
 * `stuck_alert_minutes` (defaults sourced from cliptown.toml [kanban]), and
 * always red when status is `escalated` (operator must intervene). The
 * monogram is rendered in a deterministic startup-hue circle; the
 * review-round badge lights up after the first round and escalates color
 * through round 3.
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
  highlighted,
}: {
  task: TaskVM;
  assignee: AvatarVM | undefined;
  onDragStart: (e: DragEvent, taskId: string, fromColumn: string) => void;
  /**
   * Theme G slice 3: when set, the card flashes with a colored ring.
   * Kanban toggles this transiently after `task_stolen` system_events
   * so the reassignment lands visibly instead of silently re-rendering
   * the assignee monogram.
   */
  highlighted?: boolean;
}) {
  const stuckColor = stuckIndicator(task);
  const monogramSrc =
    assignee?.agent_id ?? task.assignee_agent_id ?? "?";
  const mono = monogramSrc.slice(0, 1).toUpperCase();
  const hue = assignee ? hueFor(assignee.startup_id) : "var(--fg-secondary)";
  const reviewRound = task.review_round;
  const maxReviewRounds = task.max_review_rounds;

  return (
    <div
      draggable
      data-task-id={task.id}
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
        border: highlighted ? "1px solid #4a90e2" : "1px solid var(--border)",
        boxShadow: highlighted ? "0 0 0 2px rgba(74,144,226,0.4)" : "none",
        borderRadius: 6,
        padding: "8px 10px",
        cursor: "grab",
        marginBottom: 6,
        userSelect: "none",
        transition: "box-shadow 240ms ease, border-color 240ms ease",
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
            flexWrap: "wrap",
          }}
        >
          <code>{task.id.slice(0, 6)}</code>
          {task.required_room && <span>· {task.required_room}</span>}
          <ReviewRoundBadge round={reviewRound} max={maxReviewRounds} />
          <BlockedBadge blockedOn={task.blocked_on} />
          <DeadlineBadge deadlineAt={task.deadline_at} />
        </div>
        {task.artifact_path && (
          <div
            data-artifact-path={task.artifact_path}
            title={task.artifact_path}
            style={{
              fontSize: 11,
              color: "var(--fg-secondary)",
              marginTop: 2,
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
          >
            <code>{task.artifact_path}</code>
          </div>
        )}
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

// Renders the review-round indicator on a task card. Two signals:
//   - dot color: escalates yellow → orange → red as round approaches max
//   - text "R{round}/{max}": the operator can read the exact round, not
//     just sense it from color. Required for ship-gate § 11.6's UI proof.
// Hidden when round is 0 or undefined (task hasn't been bounced back yet).
function ReviewRoundBadge({ round, max }: { round?: number; max?: number }) {
  if (!round || round < 1) return null;
  const cap = typeof max === "number" && max > 0 ? max : 3;
  const color =
    round >= cap ? "#D62828" : round >= cap - 1 ? "#E69F00" : "#E9C46A";
  const label = `R${round}/${cap}`;
  const tooltip = `Review round ${round} of ${cap}${
    round >= cap ? " — at escalation threshold" : ""
  }`;
  return (
    <span
      data-review-round={round}
      title={tooltip}
      aria-label={tooltip}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 4,
      }}
    >
      <span
        aria-hidden
        style={{
          display: "inline-block",
          width: 6,
          height: 6,
          borderRadius: "50%",
          background: color,
        }}
      />
      <span style={{ fontSize: 11, color, fontWeight: 600 }}>{label}</span>
    </span>
  );
}

/**
 * Theme G slice 3 (E2 carry): blocker badge. When a task waits on another
 * task to finish, show "🔒 T_block" so the operator can see the dep at a
 * glance without opening the row.
 */
function BlockedBadge({ blockedOn }: { blockedOn?: string | null }) {
  if (!blockedOn) return null;
  return (
    <span
      data-blocked-on={blockedOn}
      title={`Blocked on ${blockedOn}`}
      aria-label={`Blocked on ${blockedOn}`}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 3,
        fontSize: 11,
        color: "#A04A00",
        background: "rgba(230,159,0,0.15)",
        border: "1px solid rgba(230,159,0,0.4)",
        borderRadius: 4,
        padding: "0 5px",
        lineHeight: "16px",
      }}
    >
      <span aria-hidden>🔒</span>
      <code style={{ fontSize: 10 }}>{blockedOn.slice(0, 6)}</code>
    </span>
  );
}

/**
 * Theme G slice 3 (E2 carry): deadline badge. Renders relative time
 * ("⏰ in 2h" / "⏰ overdue 60s") with red coloring once past the
 * deadline, matching the severity the scheduler uses on
 * `task_overdue` system_events.
 */
function DeadlineBadge({ deadlineAt }: { deadlineAt?: number | null }) {
  if (deadlineAt == null) return null;
  const now = Math.floor(Date.now() / 1000);
  const delta = deadlineAt - now;
  const overdue = delta < 0;
  const rel = formatRelativeSecs(Math.abs(delta));
  const label = overdue ? `overdue ${rel}` : `in ${rel}`;
  const fg = overdue ? "#A30000" : "var(--fg-secondary)";
  const bg = overdue ? "rgba(214,40,40,0.12)" : "transparent";
  const border = overdue ? "1px solid rgba(214,40,40,0.4)" : "1px solid var(--border)";
  return (
    <span
      data-deadline-at={deadlineAt}
      title={`Deadline ${new Date(deadlineAt * 1000).toLocaleString()}`}
      aria-label={`Deadline ${label}`}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 3,
        fontSize: 11,
        color: fg,
        background: bg,
        border,
        borderRadius: 4,
        padding: "0 5px",
        lineHeight: "16px",
        fontWeight: overdue ? 600 : 400,
      }}
    >
      <span aria-hidden>⏰</span>
      <span>{label}</span>
    </span>
  );
}

function formatRelativeSecs(s: number): string {
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

function stuckIndicator(task: TaskVM): string {
  // Escalated tasks burn red regardless of `updated_at`. Manager already
  // bounced the work the maximum number of times, so this card stays in
  // alert until the operator force-accepts or force-fails it.
  if (task.status === "escalated") return "#D62828";

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

/**
 * Kanban (M4.6): five primary columns + a collapsible "Failed" footer drawer
 * with HTML5-native drag-drop wired to the four operator-override commands.
 *
 * Manager-bypass: only operator overrides change task state from the UI.
 * Allowed transitions are enumerated in `dragdrop.ts`; every other drop
 * snaps back with an "agent-driven only" toast (delivered via the local
 * `addToast` stand-in until ConsoleOutbound::Toast is authoritative).
 *
 * Phase 0 caveat: `OperatorAcceptProposal` requires an `assignee_agent_id`
 * which we collect via `window.prompt`. M4.11 will replace this with a
 * proper agent picker.
 */

import { useState } from "react";
import type { CSSProperties, DragEvent } from "react";
import { useWorld } from "../hooks/useWorld.js";
import type { AvatarVM, TaskVM } from "../store.js";
import { Card } from "./Card.js";
import {
  COLUMNS,
  FAILED_COLUMN,
  allowedTransition,
  type ColumnId,
} from "./dragdrop.js";

interface DragState {
  taskId: string;
  fromColumn: string;
}

export function Kanban({ startupId }: { startupId: string | null }) {
  const { state, send, addToast } = useWorld();
  const [drag, setDrag] = useState<DragState | null>(null);
  const [overColumn, setOverColumn] = useState<ColumnId | null>(null);
  const [snapBack, setSnapBack] = useState<string | null>(null);

  if (!startupId) return null;

  const tasks = Object.values(state.tasks).filter(
    (t) => t.startup_id === startupId,
  );
  const byStatus = groupBy(tasks);

  const onDragStart = (
    e: DragEvent,
    taskId: string,
    fromColumn: string,
  ): void => {
    setDrag({ taskId, fromColumn });
    setSnapBack(null);
    e.dataTransfer.effectAllowed = "move";
    e.dataTransfer.setData("text/plain", taskId);
  };

  const onDragOver = (e: DragEvent, col: ColumnId): void => {
    e.preventDefault();
    setOverColumn(col);
  };

  const onDrop = (col: ColumnId): void => {
    const current = drag;
    setOverColumn(null);
    if (!current) return;
    setDrag(null);

    const cmd = allowedTransition(
      current.fromColumn as ColumnId,
      col,
      current.taskId,
    );
    if (!cmd) {
      setSnapBack(current.taskId);
      window.setTimeout(() => setSnapBack(null), 800);
      addToast("warn", "agent-driven only");
      return;
    }

    if (cmd.type === "operator_accept_proposal") {
      const assignee = window.prompt(
        "Assign to agent_id (Phase 0 manual; M4.11 replaces this with a picker):",
      );
      if (!assignee) return;
      send({
        type: "operator_accept_proposal",
        v: 1,
        task_id: cmd.task_id,
        assignee_agent_id: assignee,
      });
      return;
    }

    send({ ...cmd, v: 1 });
  };

  return (
    <div style={{ padding: 16 }}>
      <div
        style={{
          display: "grid",
          gridTemplateColumns: `repeat(${COLUMNS.length}, 1fr)`,
          gap: 12,
        }}
      >
        {COLUMNS.map((c) => (
          <Column
            key={c.id}
            id={c.id}
            label={c.label}
            tasks={byStatus[c.id] ?? []}
            isOver={overColumn === c.id}
            onDragOver={(e) => onDragOver(e, c.id)}
            onDrop={() => onDrop(c.id)}
            onDragStartCard={onDragStart}
            avatars={state.avatars}
            snapBack={snapBack}
          />
        ))}
      </div>
      <FailedDrawer
        tasks={byStatus[FAILED_COLUMN.id] ?? []}
        avatars={state.avatars}
        isOver={overColumn === FAILED_COLUMN.id}
        onDragOver={(e) => onDragOver(e, FAILED_COLUMN.id)}
        onDrop={() => onDrop(FAILED_COLUMN.id)}
        onDragStartCard={onDragStart}
        snapBack={snapBack}
      />
    </div>
  );
}

interface ColumnLikeProps {
  tasks: TaskVM[];
  isOver: boolean;
  onDragOver: (e: DragEvent) => void;
  onDrop: () => void;
  onDragStartCard: (e: DragEvent, taskId: string, fromColumn: string) => void;
  avatars: Record<string, AvatarVM>;
  snapBack: string | null;
}

interface ColumnProps extends ColumnLikeProps {
  id: ColumnId;
  label: string;
}

function Column({
  id,
  label,
  tasks,
  isOver,
  onDragOver,
  onDrop,
  onDragStartCard,
  avatars,
  snapBack,
}: ColumnProps) {
  return (
    <section
      data-column-id={id}
      onDragOver={onDragOver}
      onDrop={onDrop}
      style={columnStyle(isOver)}
    >
      <header style={columnHeaderStyle}>
        <span style={{ fontWeight: 600 }}>{label}</span>
        <span>{tasks.length}</span>
      </header>
      {tasks.map((t) => (
        <div
          key={t.id}
          style={{
            opacity: snapBack === t.id ? 0.4 : 1,
            transition: "opacity 200ms ease",
          }}
        >
          <Card
            task={t}
            assignee={
              t.assignee_agent_id ? avatars[t.assignee_agent_id] : undefined
            }
            onDragStart={onDragStartCard}
          />
        </div>
      ))}
    </section>
  );
}

function FailedDrawer({
  tasks,
  avatars,
  isOver,
  onDragOver,
  onDrop,
  onDragStartCard,
  snapBack,
}: ColumnLikeProps) {
  const [open, setOpen] = useState(false);
  return (
    <section
      data-column-id="failed"
      onDragOver={onDragOver}
      onDrop={onDrop}
      style={{
        marginTop: 16,
        background: isOver ? "rgba(0,0,0,0.04)" : "var(--raised)",
        border: "1px solid var(--border)",
        borderRadius: 8,
        padding: 8,
      }}
    >
      <header
        onClick={() => setOpen((v) => !v)}
        style={{
          display: "flex",
          justifyContent: "space-between",
          cursor: "pointer",
          fontSize: 12,
          color: "var(--fg-secondary)",
          padding: "4px 6px",
        }}
      >
        <span>
          <strong>Failed</strong> · {tasks.length}
        </span>
        <span>{open ? "▼" : "▶"}</span>
      </header>
      {open && (
        <div style={{ marginTop: 8 }}>
          {tasks.length === 0 ? (
            <p style={{ color: "var(--fg-secondary)", fontSize: 12 }}>
              No failed tasks.
            </p>
          ) : (
            tasks.map((t) => (
              <div
                key={t.id}
                style={{ opacity: snapBack === t.id ? 0.4 : 1 }}
              >
                <Card
                  task={t}
                  assignee={
                    t.assignee_agent_id
                      ? avatars[t.assignee_agent_id]
                      : undefined
                  }
                  onDragStart={onDragStartCard}
                />
              </div>
            ))
          )}
        </div>
      )}
    </section>
  );
}

function groupBy(tasks: TaskVM[]): Record<string, TaskVM[]> {
  const out: Record<string, TaskVM[]> = {};
  for (const t of tasks) {
    (out[t.status] ??= []).push(t);
  }
  return out;
}

const columnHeaderStyle: CSSProperties = {
  fontSize: 12,
  color: "var(--fg-secondary)",
  marginBottom: 8,
  display: "flex",
  justifyContent: "space-between",
};

function columnStyle(isOver: boolean): CSSProperties {
  return {
    background: isOver ? "rgba(0,0,0,0.04)" : "transparent",
    border: "1px solid var(--border)",
    borderRadius: 8,
    padding: 8,
    minHeight: 200,
  };
}

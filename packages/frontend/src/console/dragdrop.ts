/**
 * Allowed kanban transitions in operator manager-bypass mode (M4.6).
 *
 * The kanban supports drag-drop only for the four operator overrides:
 *   - proposed -> queued                       ⇒ OperatorAcceptProposal
 *   - proposed -> failed                       ⇒ OperatorRejectProposal
 *   - awaiting_review|escalated -> done        ⇒ OperatorForceAccept
 *   - * -> failed                              ⇒ OperatorForceFail
 *
 * Every other drop is forbidden — the card snaps back and a toast surfaces
 * "agent-driven only". This intentionally mirrors the rule that lifecycle
 * transitions are agent-driven; the operator can only intervene through the
 * narrow set of override commands enumerated above.
 *
 * `escalated` joins the force-accept allowlist because it's the only state
 * where the operator MUST intervene — the manager already bounced the work
 * the maximum number of times and the engineer can't try again.
 */

export type ColumnId =
  | "proposed"
  | "queued"
  | "in_progress"
  | "awaiting_review"
  | "escalated"
  | "done"
  | "failed";

export type Cmd =
  | { type: "operator_accept_proposal"; task_id: string }
  | { type: "operator_reject_proposal"; task_id: string; reason: string }
  | { type: "operator_force_accept"; task_id: string }
  | { type: "operator_force_fail"; task_id: string; note: string };

export function allowedTransition(
  from: ColumnId,
  to: ColumnId,
  task_id: string,
): Cmd | null {
  if (from === to) return null;
  if (to === "failed") {
    if (from === "proposed") {
      return {
        type: "operator_reject_proposal",
        task_id,
        reason: "operator override",
      };
    }
    return { type: "operator_force_fail", task_id, note: "operator override" };
  }
  if (from === "proposed" && to === "queued") {
    return { type: "operator_accept_proposal", task_id };
  }
  if ((from === "awaiting_review" || from === "escalated") && to === "done") {
    return { type: "operator_force_accept", task_id };
  }
  return null;
}

export const COLUMNS: ReadonlyArray<{ id: ColumnId; label: string }> = [
  { id: "proposed", label: "Proposed" },
  { id: "queued", label: "Queued" },
  { id: "in_progress", label: "In progress" },
  { id: "awaiting_review", label: "Awaiting review" },
  { id: "escalated", label: "Escalated" },
  { id: "done", label: "Done" },
];

export const FAILED_COLUMN: { id: ColumnId; label: string } = {
  id: "failed",
  label: "Failed",
};

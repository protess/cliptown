//! Task state machine. The world's `Cmd::HandleWorkerMsg` and `Cmd::HandleConsoleMsg`
//! arms call `next(current_status, &transition)` to compute the new status; if the
//! result is Err, the request is rejected with an audit_trail entry. The proposed
//! state and operator-force transitions are the M1.8-introduced surface.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Proposed,
    Queued,
    InProgress,
    AwaitingReview,
    ChangesRequested,
    Done,
    Failed,
    Escalated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Manager,
    NonManager,
    Operator,
    System,
}

/// Every legal request that can change a task's status.
#[derive(Debug)]
pub enum Transition {
    /// `subtask_create` from any caller. If caller is the manager of the parent task,
    /// status starts at Queued; otherwise Proposed.
    SubtaskCreate { caller: Actor },
    /// Manager (or operator) accepts a proposed subtask: Proposed -> Queued.
    AcceptProposal { caller: Actor },
    /// Manager (or operator) rejects a proposed subtask: Proposed -> Failed.
    RejectProposal { caller: Actor },
    /// Scheduler picks up a queued task: Queued -> InProgress.
    AssignFromQueued,
    /// Worker MCP `task_done`: InProgress|ChangesRequested -> AwaitingReview.
    TaskDoneMcp,
    /// Manager `task_request_changes`: AwaitingReview -> ChangesRequested.
    RequestChanges,
    /// Manager `task_accept`: AwaitingReview -> Done.
    TaskAccept,
    /// Operator force-accepts via /ws/console kanban drag: AwaitingReview -> Done.
    OperatorForceAccept,
    /// Operator force-fails via /ws/console kanban drag: any non-terminal -> Failed.
    OperatorForceFail,
    /// Worker `task_failed`: any non-terminal -> Failed.
    Fail,
    /// World auto-escalation when max_review_rounds exceeded: any non-terminal -> Escalated.
    Escalate,
}

/// Compute the next status for a task given its current status and a transition request.
/// Returns `Err(&'static str)` for illegal transitions; the caller logs to audit_trail
/// with `actor` and `reason`.
pub fn next(current: TaskStatus, t: &Transition) -> Result<TaskStatus, &'static str> {
    use TaskStatus::*;
    match (current, t) {
        // SubtaskCreate is independent of current status (the parent's status is what
        // the caller is acting under; the new task starts fresh).
        (_, Transition::SubtaskCreate { caller: Actor::Manager }) => Ok(Queued),
        (_, Transition::SubtaskCreate { caller: Actor::Operator }) => Ok(Queued),
        (_, Transition::SubtaskCreate { caller: Actor::NonManager }) => Ok(Proposed),
        (_, Transition::SubtaskCreate { caller: Actor::System }) => Ok(Queued),

        (Proposed, Transition::AcceptProposal { .. }) => Ok(Queued),
        (Proposed, Transition::RejectProposal { .. }) => Ok(Failed),

        (Queued, Transition::AssignFromQueued) => Ok(InProgress),
        (InProgress, Transition::TaskDoneMcp) => Ok(AwaitingReview),
        (ChangesRequested, Transition::TaskDoneMcp) => Ok(AwaitingReview),
        (AwaitingReview, Transition::RequestChanges) => Ok(ChangesRequested),
        (AwaitingReview, Transition::TaskAccept) => Ok(Done),
        (AwaitingReview, Transition::OperatorForceAccept) => Ok(Done),

        (s, Transition::OperatorForceFail) if s != Done && s != Failed => Ok(Failed),
        (s, Transition::Fail) if s != Done && s != Failed => Ok(Failed),
        (s, Transition::Escalate) if s != Done && s != Failed => Ok(Escalated),

        _ => Err("illegal transition"),
    }
}

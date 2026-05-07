use cliptown_world::task_sm::*;

#[test]
fn manager_subtask_goes_straight_to_queued() {
    assert_eq!(
        next(TaskStatus::Proposed, &Transition::SubtaskCreate { caller: Actor::Manager }).unwrap(),
        TaskStatus::Queued
    );
}

#[test]
fn nonmanager_subtask_lands_in_proposed() {
    assert_eq!(
        next(TaskStatus::Proposed, &Transition::SubtaskCreate { caller: Actor::NonManager }).unwrap(),
        TaskStatus::Proposed
    );
}

#[test]
fn operator_subtask_goes_straight_to_queued() {
    assert_eq!(
        next(TaskStatus::Proposed, &Transition::SubtaskCreate { caller: Actor::Operator }).unwrap(),
        TaskStatus::Queued
    );
}

#[test]
fn proposed_can_be_accepted_or_rejected() {
    assert_eq!(
        next(TaskStatus::Proposed, &Transition::AcceptProposal { caller: Actor::Manager }).unwrap(),
        TaskStatus::Queued
    );
    assert_eq!(
        next(TaskStatus::Proposed, &Transition::RejectProposal { caller: Actor::Manager }).unwrap(),
        TaskStatus::Failed
    );
}

#[test]
fn queued_to_in_progress_via_scheduler() {
    assert_eq!(
        next(TaskStatus::Queued, &Transition::AssignFromQueued).unwrap(),
        TaskStatus::InProgress
    );
}

#[test]
fn full_review_cycle() {
    assert_eq!(next(TaskStatus::InProgress, &Transition::TaskDoneMcp).unwrap(), TaskStatus::AwaitingReview);
    assert_eq!(next(TaskStatus::AwaitingReview, &Transition::RequestChanges).unwrap(), TaskStatus::ChangesRequested);
    assert_eq!(next(TaskStatus::ChangesRequested, &Transition::TaskDoneMcp).unwrap(), TaskStatus::AwaitingReview);
    assert_eq!(next(TaskStatus::AwaitingReview, &Transition::TaskAccept).unwrap(), TaskStatus::Done);
}

#[test]
fn operator_force_accept_only_from_awaiting_review() {
    assert_eq!(next(TaskStatus::AwaitingReview, &Transition::OperatorForceAccept).unwrap(), TaskStatus::Done);
    assert!(next(TaskStatus::Queued, &Transition::OperatorForceAccept).is_err());
    assert!(next(TaskStatus::InProgress, &Transition::OperatorForceAccept).is_err());
    assert!(next(TaskStatus::Done, &Transition::OperatorForceAccept).is_err());
}

#[test]
fn operator_force_fail_works_from_any_non_terminal() {
    for s in [
        TaskStatus::Proposed, TaskStatus::Queued, TaskStatus::InProgress,
        TaskStatus::AwaitingReview, TaskStatus::ChangesRequested,
    ] {
        assert_eq!(next(s, &Transition::OperatorForceFail).unwrap(), TaskStatus::Failed);
    }
    assert!(next(TaskStatus::Done, &Transition::OperatorForceFail).is_err());
    assert!(next(TaskStatus::Failed, &Transition::OperatorForceFail).is_err());
}

#[test]
fn done_is_terminal() {
    assert!(next(TaskStatus::Done, &Transition::Fail).is_err());
    assert!(next(TaskStatus::Done, &Transition::OperatorForceFail).is_err());
    assert!(next(TaskStatus::Done, &Transition::Escalate).is_err());
    assert!(next(TaskStatus::Done, &Transition::TaskAccept).is_err());
}

#[test]
fn failed_is_terminal() {
    assert!(next(TaskStatus::Failed, &Transition::Fail).is_err());
    assert!(next(TaskStatus::Failed, &Transition::AssignFromQueued).is_err());
}

#[test]
fn escalation_from_review_loop() {
    assert_eq!(
        next(TaskStatus::ChangesRequested, &Transition::Escalate).unwrap(),
        TaskStatus::Escalated
    );
}

#[test]
fn illegal_transition_returns_err() {
    // Cannot go straight from Queued to AwaitingReview.
    assert!(next(TaskStatus::Queued, &Transition::TaskDoneMcp).is_err());
    // Cannot accept a task that's not awaiting review.
    assert!(next(TaskStatus::Queued, &Transition::TaskAccept).is_err());
    // Cannot request changes outside awaiting_review.
    assert!(next(TaskStatus::InProgress, &Transition::RequestChanges).is_err());
}

//! Permission predicates that enforce the multi-tenant isolation invariant
//! (ship-gate invariant 8). Used by the world's command dispatch and by
//! property tests to prove no directive crosses startup boundaries and no
//! agent enters a foreign suite.

#[derive(Debug, Clone)]
pub struct AgentRef<'a> {
    pub agent_id: &'a str,
    pub startup_id: &'a str,
    pub kind: AgentKind,
    pub manager_id: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Agent,
    Operator,
}

/// Spec §5.3: an agent may enter a room iff the room is common (no private owner)
/// OR the room is privately owned by the agent's own startup.
/// The operator may enter ANY room (they own the building).
pub fn can_enter_room(agent: &AgentRef, room_private_to: Option<&str>) -> bool {
    if agent.kind == AgentKind::Operator {
        return true;
    }
    match room_private_to {
        None => true,
        Some(owner) => owner == agent.startup_id,
    }
}

/// Spec §6.2: a directive may be sent from `from` to `to` only when `from` is
/// either the operator (who can directive any agent) or the manager of `to`
/// within the same startup. This is the **structural enforcement** of
/// invariant 8: directives never cross startup boundaries.
pub fn can_send_directive(from: &AgentRef, to: &AgentRef) -> bool {
    if from.kind == AgentKind::Operator {
        return true;
    }
    if from.startup_id != to.startup_id {
        return false;
    }
    to.manager_id == Some(from.agent_id)
}

/// Spec §5.4: chat messages are room-scoped. The listener may receive a `chat`
/// kind message in `room_id` iff the listener has permission to be in that room.
/// (The caller already filters by who is in the room; this is the second gate.)
pub fn can_receive_chat_in_room(
    listener: &AgentRef,
    speaker: &AgentRef,
    room_private_to: Option<&str>,
) -> bool {
    can_enter_room(listener, room_private_to) && can_enter_room(speaker, room_private_to)
}

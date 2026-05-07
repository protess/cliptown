//! Property tests for the multi-tenant isolation invariant (ship-gate invariant 8).
//! Random sequences of agents and rooms must never violate the spec's permission rules.

use cliptown_world::permissions::*;
use proptest::prelude::*;

proptest! {
    /// An agent never enters a suite owned by a different startup.
    #[test]
    fn agent_never_enters_foreign_suite(
        startup_self in "[a-z]{3}",
        startup_owner in "[a-z]{3}"
    ) {
        prop_assume!(startup_self != startup_owner);
        let a = AgentRef {
            agent_id: "x",
            startup_id: &startup_self,
            kind: AgentKind::Agent,
            manager_id: None,
        };
        prop_assert!(!can_enter_room(&a, Some(&startup_owner)));
    }

    /// An agent CAN enter their own startup's suite.
    #[test]
    fn agent_can_enter_own_suite(s in "[a-z]{3}") {
        let a = AgentRef {
            agent_id: "x",
            startup_id: &s,
            kind: AgentKind::Agent,
            manager_id: None,
        };
        prop_assert!(can_enter_room(&a, Some(&s)));
    }

    /// Any agent CAN enter a common room.
    #[test]
    fn any_agent_can_enter_common_room(s in "[a-z]{3}") {
        let a = AgentRef {
            agent_id: "x",
            startup_id: &s,
            kind: AgentKind::Agent,
            manager_id: None,
        };
        prop_assert!(can_enter_room(&a, None));
    }

    /// Operator can enter any room (private or common).
    #[test]
    fn operator_can_always_enter(any_owner in proptest::option::of("[a-z]{3}")) {
        let op = AgentRef {
            agent_id: "op",
            startup_id: "_",
            kind: AgentKind::Operator,
            manager_id: None,
        };
        prop_assert!(can_enter_room(&op, any_owner.as_deref()));
    }

    /// THE INVARIANT 8 PROPERTY: directives never cross startup boundaries.
    #[test]
    fn directive_never_crosses_startup_boundary(
        a_startup in "[a-z]{3}",
        b_startup in "[a-z]{3}"
    ) {
        prop_assume!(a_startup != b_startup);
        let from = AgentRef {
            agent_id: "f",
            startup_id: &a_startup,
            kind: AgentKind::Agent,
            manager_id: None,
        };
        // Even if `to` claims `from` is its manager, cross-startup directive must be rejected.
        let to = AgentRef {
            agent_id: "t",
            startup_id: &b_startup,
            kind: AgentKind::Agent,
            manager_id: Some("f"),
        };
        prop_assert!(!can_send_directive(&from, &to));
    }

    /// Directives within the same startup require the manager relationship.
    #[test]
    fn directive_requires_manager_relationship(s in "[a-z]{3}") {
        let from = AgentRef {
            agent_id: "f",
            startup_id: &s,
            kind: AgentKind::Agent,
            manager_id: None,
        };
        // A non-manager peer can't directive a sibling.
        let peer = AgentRef {
            agent_id: "p",
            startup_id: &s,
            kind: AgentKind::Agent,
            manager_id: Some("someone_else"),
        };
        prop_assert!(!can_send_directive(&from, &peer));
    }

    /// Operator can directive anyone in any startup.
    #[test]
    fn operator_can_directive_anyone(s in "[a-z]{3}") {
        let op = AgentRef {
            agent_id: "op",
            startup_id: "_",
            kind: AgentKind::Operator,
            manager_id: None,
        };
        let target = AgentRef {
            agent_id: "t",
            startup_id: &s,
            kind: AgentKind::Agent,
            manager_id: None,
        };
        prop_assert!(can_send_directive(&op, &target));
    }
}

// Concrete sanity tests outside proptest! for fixed scenarios.

#[test]
fn directive_within_same_startup_to_direct_report() {
    let from = AgentRef {
        agent_id: "f",
        startup_id: "alpha",
        kind: AgentKind::Agent,
        manager_id: None,
    };
    let to = AgentRef {
        agent_id: "t",
        startup_id: "alpha",
        kind: AgentKind::Agent,
        manager_id: Some("f"),
    };
    assert!(can_send_directive(&from, &to));
}

#[test]
fn chat_in_common_room_crosses_startups() {
    let alpha_eng = AgentRef {
        agent_id: "ae",
        startup_id: "alpha",
        kind: AgentKind::Agent,
        manager_id: None,
    };
    let beta_designer = AgentRef {
        agent_id: "bd",
        startup_id: "beta",
        kind: AgentKind::Agent,
        manager_id: None,
    };
    // Common room (None owner): cross-startup chat allowed (this is invariant 7's substrate).
    assert!(can_receive_chat_in_room(&alpha_eng, &beta_designer, None));
}

#[test]
fn chat_in_foreign_suite_is_blocked() {
    let alpha_eng = AgentRef {
        agent_id: "ae",
        startup_id: "alpha",
        kind: AgentKind::Agent,
        manager_id: None,
    };
    let beta_designer = AgentRef {
        agent_id: "bd",
        startup_id: "beta",
        kind: AgentKind::Agent,
        manager_id: None,
    };
    // Beta's suite — alpha can't even be there.
    assert!(!can_receive_chat_in_room(&alpha_eng, &beta_designer, Some("beta")));
}

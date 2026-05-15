//! Operator-side command dispatcher. Called from loop_::spawn's HandleConsoleMsg arm.
//! Parses ConsoleInbound, applies the world mutation + SQLite write, returns a JSON reply.
//!
//! P3 Theme B: `identity` carries the authenticated operator's role.
//! Mutating arms (Directive / Force* / Skill* / proposal accept-reject) require
//! `Manager` or higher; read-ish arms (Possess/Unpossess/Move/Hello) are
//! viewer-OK. Viewer attempts on a manager-gated arm return
//! `{"type":"error","reason":"forbidden"}` and do not touch SQL or the
//! broadcast bus.

use crate::auth::{OperatorIdentity, OperatorRole};
use crate::health::Health;
use crate::persist;
use crate::protocol::ConsoleInbound;
use crate::state::{AvatarView, WorldView};
use crate::task_sm::{next, Actor, TaskStatus, Transition};
use serde_json::json;
use sqlx::SqlitePool;
use std::collections::HashMap;
use tokio::sync::mpsc;

const OPERATOR_AVATAR_ID: &str = "__operator__";

fn forbidden() -> serde_json::Value {
    json!({"type":"error","reason":"forbidden"})
}

/// Dispatch a single console message. Mutates `world` and writes to `pool`.
/// Returns a JSON value that becomes the WS reply.
///
/// `out_bus` is the world's per-agent worker outbound channel map; the
/// OperatorDirective arm pushes a `directive` event to the recipient so the
/// founder's CLI sees the directive on its next session boot (M5.3).
pub async fn dispatch(
    world: &mut WorldView,
    pool: &SqlitePool,
    out_bus: &HashMap<String, mpsc::Sender<serde_json::Value>>,
    event_tx: &tokio::sync::broadcast::Sender<crate::protocol::ConsoleOutbound>,
    identity: &OperatorIdentity,
    msg: serde_json::Value,
) -> serde_json::Value {
    let inbound: ConsoleInbound = match serde_json::from_value(msg.clone()) {
        Ok(v) => v,
        Err(e) => return json!({"type":"error","reason":"parse","detail":e.to_string()}),
    };

    let is_manager = identity.role.at_least(OperatorRole::Manager);

    // P3 Theme D follow-up: trace the command kind + operator so audit
    // replays correlate with structured logs. `Hello` and `Recheck` are
    // chatty; debug level keeps a default-filter subscriber clean while
    // letting `RUST_LOG=cliptown_world::cmd_console=debug` surface them.
    let cmd_start = std::time::Instant::now();
    let command_kind = match &inbound {
        ConsoleInbound::Hello { .. } => "hello",
        ConsoleInbound::OperatorMove { .. } => "operator_move",
        ConsoleInbound::OperatorPossess { .. } => "operator_possess",
        ConsoleInbound::OperatorUnpossess { .. } => "operator_unpossess",
        ConsoleInbound::OperatorDirective { .. } => "operator_directive",
        ConsoleInbound::OperatorAcceptProposal { .. } => "operator_accept_proposal",
        ConsoleInbound::OperatorRejectProposal { .. } => "operator_reject_proposal",
        ConsoleInbound::OperatorForceAccept { .. } => "operator_force_accept",
        ConsoleInbound::OperatorForceFail { .. } => "operator_force_fail",
        ConsoleInbound::OperatorRecheckBackends => "operator_recheck_backends",
        ConsoleInbound::SkillAttach { .. } => "skill_attach",
        ConsoleInbound::SkillDetach { .. } => "skill_detach",
        ConsoleInbound::OperatorList { .. } => "operator_list",
        ConsoleInbound::OperatorCreate { .. } => "operator_create",
        ConsoleInbound::OperatorRevoke { .. } => "operator_revoke",
        ConsoleInbound::OperatorSetRole { .. } => "operator_set_role",
        ConsoleInbound::SkillUpsertOperator { .. } => "skill_upsert_operator",
        ConsoleInbound::SkillDeleteOperator { .. } => "skill_delete_operator",
        ConsoleInbound::SkillSetGlobal { .. } => "skill_set_global",
    };
    tracing::debug!(
        component = "cmd_console",
        event = "enter",
        command_kind,
        operator_id = %identity.id,
        operator_role = identity.role.as_str(),
    );

    let result = match inbound {
        ConsoleInbound::Hello { .. } => {
            // Auth was already validated in http::handle_console; subsequent hello is a no-op echo.
            json!({"type":"ok","kind":"hello"})
        }
        ConsoleInbound::OperatorMove { target_x, target_y, .. } => {
            // The operator avatar must be possessing a town for move to be meaningful;
            // if absent, this is a no-op with an error.
            if let Some(avatar) = world.avatars.get_mut(OPERATOR_AVATAR_ID) {
                avatar.target_pos = Some((target_x, target_y));
                json!({"type":"ok","kind":"operator_move"})
            } else {
                json!({"type":"error","reason":"not_possessing"})
            }
        }
        ConsoleInbound::OperatorPossess { startup_id, .. } => {
            // Spawn operator avatar in the lobby of `startup_id`'s town.
            // For Phase 0 the town id is fixed at "town_default" and lobby is at (20, 5).
            let avatar = AvatarView {
                agent_id: OPERATOR_AVATAR_ID.to_string(),
                startup_id: startup_id.clone(),
                role: "operator".to_string(),
                backend: "operator".to_string(),
                current_pos: (20, 5),
                target_pos: None,
                room_id: "lobby".to_string(),
                status: "idle".to_string(),
                last_seen_at: None,
                health: Health::Online,
            };
            world.avatars.insert(OPERATOR_AVATAR_ID.to_string(), avatar);
            json!({"type":"ok","kind":"operator_possess","startup_id":startup_id})
        }
        ConsoleInbound::OperatorUnpossess { .. } => {
            world.avatars.remove(OPERATOR_AVATAR_ID);
            json!({"type":"ok","kind":"operator_unpossess"})
        }
        ConsoleInbound::OperatorDirective { to_agent_id, body, .. } => {
            if !is_manager { return forbidden(); }
            // Reject overlong bodies before any side effect — symmetric with the
            // worker-side mcp_dispatch::handle_speak/handle_task_request_changes
            // guard so the operator can't bypass the limit and starve the
            // broadcast channel.
            if body.chars().count() > crate::mcp_dispatch::MAX_BODY_LENGTH {
                return json!({"type":"error","reason":"body_too_long"});
            }
            // Prefetch recipient validity + startup_id BEFORE any side effect.
            // Codex M4: returning a clean unknown_recipient error is cheaper than
            // letting an inline-subquery INSERT fail via FK violation.
            let row: Result<Option<(String,)>, _> =
                sqlx::query_as("SELECT startup_id FROM agents WHERE id = ?")
                    .bind(&to_agent_id)
                    .fetch_optional(pool)
                    .await;
            let recipient_startup_id = match row {
                Ok(Some((sid,))) => sid,
                Ok(None) => return json!({"type":"error","reason":"unknown_recipient"}),
                Err(e) => return json!({"type":"error","reason":"sql","detail":e.to_string()}),
            };

            let id = uuid::Uuid::new_v4().to_string();
            let r = sqlx::query(
                "INSERT INTO messages (id, startup_id, room_id, author_id, body, kind, ts) \
                 VALUES (?, ?, NULL, 'operator', ?, 'directive', unixepoch())",
            )
            .bind(&id)
            .bind(&recipient_startup_id)
            .bind(&body)
            .execute(pool)
            .await;
            match r {
                Ok(_) => {
                    // Push to recipient's worker out_bus (existing behavior).
                    if let Some(tx) = out_bus.get(&to_agent_id) {
                        let payload = json!({
                            "type": "directive",
                            "v": 1,
                            "from_agent_id": "operator",
                            "body": body,
                            "message_id": id,
                        });
                        if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(payload) {
                            tracing::warn!(component = "cmd_console",
                                agent_id = %to_agent_id,
                                "out_bus full, dropping operator directive"
                            );
                        }
                    }
                    // Broadcast a Directive frame to all subscribed operator consoles
                    // (god view). After SQL success only.
                    let _ = event_tx.send(crate::protocol::ConsoleOutbound::Directive {
                        v: 1,
                        message_id: id.clone(),
                        ts: chrono::Utc::now().timestamp_millis(),
                        startup_id: recipient_startup_id,
                        author_id: "operator".into(),
                        to_agent_id: to_agent_id.clone(),
                        body: body.clone(),
                        in_response_to_task: None,
                    });
                    json!({"type":"ok","kind":"operator_directive","message_id":id})
                }
                Err(e) => json!({"type":"error","reason":"sql","detail":e.to_string()}),
            }
        }
        ConsoleInbound::OperatorAcceptProposal { task_id, assignee_agent_id, required_room, .. } => {
            if !is_manager { return forbidden(); }
            // Inlined (deviation from plan): the generic `apply_task_transition` closure
            // approach has tricky sqlx::Query lifetime constraints; inlining the
            // accept_proposal flow is simpler and keeps the audit + state-machine call
            // + UPDATE in a single readable block.
            //
            // Codex round-3 P2#4: pull `startup_id` alongside `status` so we can
            // refuse cross-startup assignments before the UPDATE — mirrors the
            // mcp_dispatch path so the operator UI can't bypass the guard.
            let current_row: Result<(String, String), _> =
                sqlx::query_as("SELECT status, startup_id FROM tasks WHERE id = ?")
                    .bind(&task_id)
                    .fetch_one(pool)
                    .await;
            let (current, task_startup_id) = match current_row {
                Ok((s, sid)) => match parse_status(&s) {
                    Some(s) => (s, sid),
                    None => return json!({"type":"error","reason":"unknown_status","status":s}),
                },
                Err(e) => return json!({"type":"error","reason":"sql","detail":e.to_string()}),
            };
            if next(current, &Transition::AcceptProposal { caller: Actor::Operator }).is_err() {
                return json!({"type":"error","reason":"illegal_transition"});
            }
            // Same-startup check: scheduler can't dispatch the task to a
            // foreign worker without this guard (task_done would later
            // reject it, leaving the task wedged in `queued`).
            let assignee_row: Result<Option<(String,)>, _> =
                sqlx::query_as("SELECT startup_id FROM agents WHERE id = ?")
                    .bind(&assignee_agent_id)
                    .fetch_optional(pool)
                    .await;
            match assignee_row {
                Ok(Some((sid,))) if sid == task_startup_id => { /* ok */ }
                Ok(Some(_)) => {
                    return json!({"type":"error","reason":"cross_startup","detail":"assignee in different startup"});
                }
                Ok(None) => {
                    return json!({"type":"error","reason":"unknown_assignee","detail":"no such agent"});
                }
                Err(e) => return json!({"type":"error","reason":"sql","detail":e.to_string()}),
            }
            let r = sqlx::query(
                "UPDATE tasks SET status = 'queued', assignee_agent_id = ?, required_room = ?, updated_at = unixepoch() WHERE id = ?"
            )
            .bind(&assignee_agent_id)
            .bind(&required_room)
            .bind(&task_id)
            .execute(pool)
            .await;
            match r {
                Ok(_) => {
                    let _ = persist::append_audit(
                        pool,
                        &task_id,
                        &json!({"actor":"operator","kind":"accept_proposal"}).to_string(),
                    )
                    .await;
                    json!({"type":"ok","kind":"operator_accept_proposal","task_id":task_id})
                }
                Err(e) => json!({"type":"error","reason":"sql","detail":e.to_string()}),
            }
        }
        ConsoleInbound::OperatorRejectProposal { task_id, reason, .. } => {
            if !is_manager { return forbidden(); }
            let result = apply_status_only_transition(
                pool,
                &task_id,
                &Transition::RejectProposal { caller: Actor::Operator },
                "operator_reject_proposal",
            )
            .await;
            // Only write the audit on a successful transition.
            if result["type"] == "ok" {
                let _ = persist::append_audit(
                    pool,
                    &task_id,
                    &json!({
                        "actor":"operator","kind":"reject_proposal","reason":reason
                    })
                    .to_string(),
                )
                .await;
            }
            result
        }
        ConsoleInbound::OperatorForceAccept { task_id, .. } => {
            if !is_manager { return forbidden(); }
            let result = apply_status_only_transition(
                pool,
                &task_id,
                &Transition::OperatorForceAccept,
                "operator_force_accept",
            )
            .await;
            // Only write the audit on a successful transition.
            if result["type"] == "ok" {
                let _ = persist::append_audit(
                    pool,
                    &task_id,
                    &json!({"actor":"operator","kind":"force_accept"}).to_string(),
                )
                .await;
            }
            result
        }
        ConsoleInbound::OperatorForceFail { task_id, note, .. } => {
            if !is_manager { return forbidden(); }
            let result = apply_status_only_transition(
                pool,
                &task_id,
                &Transition::OperatorForceFail,
                "operator_force_fail",
            )
            .await;
            if result["type"] == "ok" {
                let _ = persist::append_audit(
                    pool,
                    &task_id,
                    &json!({"actor":"operator","kind":"force_fail","note":note}).to_string(),
                )
                .await;
            }
            result
        }
        ConsoleInbound::OperatorRecheckBackends => {
            // The actual probe is fired by the SIGHUP handler / 5-min refresh in main.rs;
            // M1.4 wired the loop's Cmd::BackendCatalogUpdated. Operator-triggered recheck
            // is a future hook; for now reply OK and let the operator fall back to the
            // POST /api/backend-catalog/recheck HTTP endpoint.
            json!({"type":"ok","kind":"operator_recheck_backends","note":"use POST /api/backend-catalog/recheck"})
        }
        ConsoleInbound::SkillAttach { startup_id, agent_id, skill_id, .. } => {
            if !is_manager { return forbidden(); }
            match crate::skills::attach(pool, &startup_id, &agent_id, &skill_id).await {
                Ok(()) => {
                    let _ = event_tx.send(crate::protocol::ConsoleOutbound::SkillChanged {
                        v: 1,
                        startup_id: startup_id.clone(),
                        kind: "attach".to_string(),
                        skill_id: skill_id.clone(),
                        agent_id: Some(agent_id.clone()),
                        skill: None,
                    });
                    json!({"type":"ok","kind":"skill_attach"})
                }
                Err(crate::skills::SkillError::NotFound) => {
                    json!({"type":"error","reason":"not_found"})
                }
                Err(crate::skills::SkillError::CrossStartup) => {
                    json!({"type":"error","reason":"cross_startup"})
                }
                Err(e) => json!({"type":"error","reason":"sql","detail":format!("{e:?}")}),
            }
        }
        ConsoleInbound::SkillDetach { startup_id, agent_id, skill_id, .. } => {
            if !is_manager { return forbidden(); }
            match crate::skills::detach(pool, &startup_id, &agent_id, &skill_id).await {
                Ok(()) => {
                    let _ = event_tx.send(crate::protocol::ConsoleOutbound::SkillChanged {
                        v: 1,
                        startup_id: startup_id.clone(),
                        kind: "detach".to_string(),
                        skill_id: skill_id.clone(),
                        agent_id: Some(agent_id.clone()),
                        skill: None,
                    });
                    json!({"type":"ok","kind":"skill_detach"})
                }
                Err(crate::skills::SkillError::NotFound) => {
                    json!({"type":"error","reason":"not_found"})
                }
                Err(crate::skills::SkillError::CrossStartup) => {
                    json!({"type":"error","reason":"cross_startup"})
                }
                Err(e) => json!({"type":"error","reason":"sql","detail":format!("{e:?}")}),
            }
        }
        ConsoleInbound::SkillUpsertOperator { startup_id, skill_id: _, name, content_md, .. } => {
            // skill_id is ignored — `skills::upsert` resolves by (startup_id, name).
            // Kept on the wire for forward-compat with a future id-keyed UPDATE.
            if !is_manager { return forbidden(); }
            match crate::skills::upsert_with_author(
                pool, &startup_id, &name, &content_md,
                crate::skills::Author::Operator(&identity.id),
            ).await {
                Ok((id, _is_new)) => {
                    let _ = event_tx.send(crate::protocol::ConsoleOutbound::SkillChanged {
                        v: 1,
                        startup_id: startup_id.clone(),
                        kind: "upsert".to_string(),
                        skill_id: id.clone(),
                        agent_id: None,
                        skill: None,
                    });
                    json!({"type":"ok","kind":"skill_upsert","skill_id": id})
                }
                Err(crate::skills::SkillError::BadName) => json!({"type":"error","reason":"bad_name"}),
                Err(crate::skills::SkillError::OversizeContent) => json!({"type":"error","reason":"oversize_content"}),
                Err(crate::skills::SkillError::CrossStartup) => json!({"type":"error","reason":"cross_startup"}),
                Err(e) => json!({"type":"error","reason":"sql","detail":format!("{e:?}")}),
            }
        }
        ConsoleInbound::SkillSetGlobal { skill_id, is_global, .. } => {
            // Admin-only — flipping a skill global affects every startup
            // in the world. Manager has no business changing world-wide
            // visibility for content they may not own.
            if !identity.role.at_least(OperatorRole::Admin) { return forbidden(); }
            match crate::skills::set_global(pool, &skill_id, is_global).await {
                Ok(()) => {
                    // Re-fetch the owning startup so the broadcast carries
                    // it; consoles in any startup fan an update because
                    // global skills are visible everywhere.
                    let owner: Result<Option<(String,)>, _> = sqlx::query_as(
                        "SELECT startup_id FROM skills WHERE id = ?"
                    ).bind(&skill_id).fetch_optional(pool).await;
                    let startup_id = owner.ok().flatten().map(|(s,)| s).unwrap_or_default();
                    let _ = event_tx.send(crate::protocol::ConsoleOutbound::SkillChanged {
                        v: 1,
                        startup_id,
                        kind: if is_global { "set_global".to_string() } else { "clear_global".to_string() },
                        skill_id: skill_id.clone(),
                        agent_id: None,
                        skill: None,
                    });
                    json!({"type":"ok","kind":"skill_set_global","skill_id":skill_id,"is_global":is_global})
                }
                Err(crate::skills::SkillError::NotFound) => json!({"type":"error","reason":"not_found"}),
                Err(e) => json!({"type":"error","reason":"sql","detail":format!("{e:?}")}),
            }
        }
        ConsoleInbound::SkillDeleteOperator { startup_id, skill_id, .. } => {
            if !is_manager { return forbidden(); }
            match crate::skills::delete(pool, &startup_id, &skill_id).await {
                Ok(()) => {
                    let _ = event_tx.send(crate::protocol::ConsoleOutbound::SkillChanged {
                        v: 1,
                        startup_id: startup_id.clone(),
                        kind: "delete".to_string(),
                        skill_id: skill_id.clone(),
                        agent_id: None,
                        skill: None,
                    });
                    json!({"type":"ok","kind":"skill_delete"})
                }
                Err(crate::skills::SkillError::NotFound) => json!({"type":"error","reason":"not_found"}),
                Err(crate::skills::SkillError::CrossStartup) => json!({"type":"error","reason":"cross_startup"}),
                Err(e) => json!({"type":"error","reason":"sql","detail":format!("{e:?}")}),
            }
        }
        ConsoleInbound::OperatorList { .. } => {
            if !identity.role.at_least(OperatorRole::Admin) { return forbidden(); }
            let rows: Result<Vec<(String, String, String, i64)>, _> = sqlx::query_as(
                "SELECT id, name, role, created_at FROM operators ORDER BY created_at"
            ).fetch_all(pool).await;
            match rows {
                Ok(rows) => {
                    let arr: Vec<serde_json::Value> = rows.into_iter().map(|(id, name, role, created_at)| {
                        json!({"id": id, "name": name, "role": role, "created_at": created_at})
                    }).collect();
                    json!({"type":"ok","kind":"operator_list","operators": arr})
                }
                Err(e) => json!({"type":"error","reason":"sql","detail":e.to_string()}),
            }
        }
        ConsoleInbound::OperatorCreate { name, role, .. } => {
            if !identity.role.at_least(OperatorRole::Admin) { return forbidden(); }
            if OperatorRole::from_str(&role).is_none() {
                return json!({"type":"error","reason":"bad_role","detail":role});
            }
            if name.trim().is_empty() {
                return json!({"type":"error","reason":"bad_name"});
            }
            // Mint a token. UUID v4 is plenty random for bearer tokens; the
            // operator copies it from the response, plain text on the wire is
            // fine because the WS itself should be TLS in prod.
            let new_id = format!("op_{}", uuid::Uuid::new_v4().simple());
            let new_token = format!("opt_{}", uuid::Uuid::new_v4().simple());
            let r = sqlx::query(
                "INSERT INTO operators (id, name, token, role, created_at) VALUES (?, ?, ?, ?, unixepoch())"
            )
                .bind(&new_id).bind(&name).bind(&new_token).bind(&role)
                .execute(pool).await;
            match r {
                Ok(_) => json!({
                    "type":"ok","kind":"operator_create",
                    "id": new_id, "name": name, "role": role, "token": new_token,
                }),
                Err(sqlx::Error::Database(e)) if e.message().contains("UNIQUE") => {
                    json!({"type":"error","reason":"name_taken"})
                }
                Err(e) => json!({"type":"error","reason":"sql","detail":e.to_string()}),
            }
        }
        ConsoleInbound::OperatorRevoke { operator_id, .. } => {
            if !identity.role.at_least(OperatorRole::Admin) { return forbidden(); }
            if operator_id == identity.id {
                // Self-revoke would lock the admin out mid-session — refuse.
                return json!({"type":"error","reason":"cannot_revoke_self"});
            }
            let r = sqlx::query("DELETE FROM operators WHERE id = ?")
                .bind(&operator_id)
                .execute(pool).await;
            match r {
                Ok(res) if res.rows_affected() == 0 => {
                    json!({"type":"error","reason":"not_found"})
                }
                Ok(_) => json!({"type":"ok","kind":"operator_revoke","id": operator_id}),
                Err(e) => json!({"type":"error","reason":"sql","detail":e.to_string()}),
            }
        }
        ConsoleInbound::OperatorSetRole { operator_id, role, .. } => {
            if !identity.role.at_least(OperatorRole::Admin) { return forbidden(); }
            if OperatorRole::from_str(&role).is_none() {
                return json!({"type":"error","reason":"bad_role","detail":role});
            }
            if operator_id == identity.id && role != identity.role.as_str() {
                // Self-demotion to non-admin would lock out admin mid-session.
                return json!({"type":"error","reason":"cannot_demote_self"});
            }
            let r = sqlx::query("UPDATE operators SET role = ? WHERE id = ?")
                .bind(&role).bind(&operator_id)
                .execute(pool).await;
            match r {
                Ok(res) if res.rows_affected() == 0 => {
                    json!({"type":"error","reason":"not_found"})
                }
                Ok(_) => json!({"type":"ok","kind":"operator_set_role","id": operator_id, "role": role}),
                Err(e) => json!({"type":"error","reason":"sql","detail":e.to_string()}),
            }
        }
    };
    tracing::debug!(
        component = "cmd_console",
        event = "exit",
        command_kind,
        operator_id = %identity.id,
        elapsed_us = cmd_start.elapsed().as_micros() as u64,
        outcome = result.get("type").and_then(|v| v.as_str()).unwrap_or("?"),
    );
    result
}

/// Generic helper: read current task status, compute new status via task_sm, run UPDATE.
async fn apply_status_only_transition(
    pool: &SqlitePool,
    task_id: &str,
    transition: &Transition,
    audit_kind: &str,
) -> serde_json::Value {
    let current_row: Result<(String,), _> = sqlx::query_as("SELECT status FROM tasks WHERE id = ?")
        .bind(task_id)
        .fetch_one(pool)
        .await;
    let current = match current_row {
        Ok((s,)) => match parse_status(&s) {
            Some(s) => s,
            None => return json!({"type":"error","reason":"unknown_status","status":s}),
        },
        Err(e) => return json!({"type":"error","reason":"sql","detail":e.to_string()}),
    };
    let new_status = match next(current, transition) {
        Ok(s) => s,
        Err(reason) => return json!({"type":"error","reason":"illegal_transition","detail":reason}),
    };
    let new_str = status_to_str(new_status);
    let r = sqlx::query("UPDATE tasks SET status = ?, updated_at = unixepoch() WHERE id = ?")
        .bind(new_str)
        .bind(task_id)
        .execute(pool)
        .await;
    match r {
        Ok(_) => json!({"type":"ok","kind":audit_kind,"task_id":task_id,"new_status":new_str}),
        Err(e) => json!({"type":"error","reason":"sql","detail":e.to_string()}),
    }
}

fn parse_status(s: &str) -> Option<TaskStatus> {
    match s {
        "proposed" => Some(TaskStatus::Proposed),
        "queued" => Some(TaskStatus::Queued),
        "in_progress" => Some(TaskStatus::InProgress),
        "awaiting_review" => Some(TaskStatus::AwaitingReview),
        "changes_requested" => Some(TaskStatus::ChangesRequested),
        "done" => Some(TaskStatus::Done),
        "failed" => Some(TaskStatus::Failed),
        "escalated" => Some(TaskStatus::Escalated),
        _ => None,
    }
}

fn status_to_str(s: TaskStatus) -> &'static str {
    match s {
        TaskStatus::Proposed => "proposed",
        TaskStatus::Queued => "queued",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::AwaitingReview => "awaiting_review",
        TaskStatus::ChangesRequested => "changes_requested",
        TaskStatus::Done => "done",
        TaskStatus::Failed => "failed",
        TaskStatus::Escalated => "escalated",
    }
}

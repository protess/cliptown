/**
 * Console — the /console route. Hosts the top bar, the startup sidebar, the
 * main pane (header + Kanban), and the floating chat panel.
 *
 * Phase 0 / M4.13 — selection is read from the WorldProvider context so the
 * global keymap (j/k cycle, `t` open-town) can mutate it without prop
 * drilling.
 *
 * M12 P2.2 — SkillsPanel added below the Kanban. `possessedStartupId` is
 * derived from the `__operator__` avatar (same convention as ChatPanel).
 */
import { useCallback, useEffect } from "react";
import { useWorld } from "../hooks/useWorld.js";
import { TopBar } from "./TopBar.js";
import { Sidebar } from "./Sidebar.js";
import { MainHeader } from "./MainHeader.js";
import { Kanban } from "./Kanban.js";
import { ChatPanel } from "./ChatPanel.js";
import { SkillsPanel } from "./SkillsPanel.js";
import { AgentsPanel } from "./AgentsPanel.js";
import { OperatorsPanel } from "./OperatorsPanel.js";

const OPERATOR_AVATAR_ID = "__operator__";

export function Console() {
  const { state, send, selectedStartupId, setSelectedStartupId } = useWorld();

  // The operator "possesses" a startup by being present as the __operator__
  // avatar in that startup — same convention as ChatPanel.
  const possessedStartupId = state.avatars[OPERATOR_AVATAR_ID]?.startup_id ?? null;

  const onSkillAttach = useCallback(
    (skillId: string, agentId: string) => {
      if (!possessedStartupId) return;
      send({ type: "skill_attach", v: 1, startup_id: possessedStartupId, agent_id: agentId, skill_id: skillId });
    },
    [send, possessedStartupId],
  );

  const onSkillDetach = useCallback(
    (skillId: string, agentId: string) => {
      if (!possessedStartupId) return;
      send({ type: "skill_detach", v: 1, startup_id: possessedStartupId, agent_id: agentId, skill_id: skillId });
    },
    [send, possessedStartupId],
  );

  // P3 Theme F follow-up: operator-side skill authoring.
  const onSkillUpsert = useCallback(
    (name: string, contentMd: string, skillId: string | null) => {
      if (!possessedStartupId) return;
      send({
        type: "skill_upsert_operator",
        v: 1,
        startup_id: possessedStartupId,
        skill_id: skillId,
        name,
        content_md: contentMd,
      });
    },
    [send, possessedStartupId],
  );

  const onSkillDelete = useCallback(
    (skillId: string) => {
      if (!possessedStartupId) return;
      send({
        type: "skill_delete_operator",
        v: 1,
        startup_id: possessedStartupId,
        skill_id: skillId,
      });
    },
    [send, possessedStartupId],
  );

  const onSkillSetGlobal = useCallback(
    (skillId: string, isGlobal: boolean) => {
      send({ type: "skill_set_global", v: 1, skill_id: skillId, is_global: isGlobal });
    },
    [send],
  );

  // Theme G slice 4: operator-side revision history + revert wiring.
  const onSkillListRevisions = useCallback(
    (skillId: string) => {
      if (!possessedStartupId) return;
      send({
        type: "skill_list_revisions_operator",
        v: 1,
        startup_id: possessedStartupId,
        skill_id: skillId,
      });
    },
    [send, possessedStartupId],
  );

  const onSkillRevert = useCallback(
    (skillId: string, revSeq: number) => {
      if (!possessedStartupId) return;
      send({
        type: "skill_revert_operator",
        v: 1,
        startup_id: possessedStartupId,
        skill_id: skillId,
        rev_seq: revSeq,
      });
    },
    [send, possessedStartupId],
  );

  // P5 Theme A: presence heartbeat. Sent immediately on focus change and
  // every 30s thereafter. Server-side TTL is 90s so missing one beat
  // (laptop sleep, brief tab switch) doesn't immediately drop us.
  useEffect(() => {
    send({
      type: "presence_heartbeat",
      v: 1,
      focused_startup_id: selectedStartupId,
    });
    const id = window.setInterval(() => {
      send({
        type: "presence_heartbeat",
        v: 1,
        focused_startup_id: selectedStartupId,
      });
    }, 30_000);
    return () => window.clearInterval(id);
  }, [send, selectedStartupId]);

  return (
    <div
      style={{ display: "flex", flexDirection: "column", minHeight: "100vh" }}
    >
      <TopBar />
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "280px 1fr",
          flex: 1,
          minHeight: 0,
        }}
      >
        <Sidebar
          selected={selectedStartupId}
          onSelect={setSelectedStartupId}
        />
        <main
          style={{
            display: "flex",
            flexDirection: "column",
            minHeight: 0,
            overflow: "hidden",
          }}
        >
          <MainHeader startupId={selectedStartupId} />
          <div style={{ overflow: "auto", flex: 1 }}>
            <Kanban startupId={selectedStartupId} />
          </div>
          <SkillsPanel
            state={state}
            possessedStartupId={possessedStartupId}
            onAttach={onSkillAttach}
            onDetach={onSkillDetach}
            onUpsert={onSkillUpsert}
            onDelete={onSkillDelete}
            onSetGlobal={onSkillSetGlobal}
            onListRevisions={onSkillListRevisions}
            onRevert={onSkillRevert}
          />
          <AgentsPanel startupId={selectedStartupId} />
          <OperatorsPanel />
        </main>
      </div>
      <ChatPanel />
    </div>
  );
}

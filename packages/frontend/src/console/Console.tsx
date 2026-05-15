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
import { useCallback } from "react";
import { useWorld } from "../hooks/useWorld.js";
import { TopBar } from "./TopBar.js";
import { Sidebar } from "./Sidebar.js";
import { MainHeader } from "./MainHeader.js";
import { Kanban } from "./Kanban.js";
import { ChatPanel } from "./ChatPanel.js";
import { SkillsPanel } from "./SkillsPanel.js";
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
          />
          <OperatorsPanel />
        </main>
      </div>
      <ChatPanel />
    </div>
  );
}

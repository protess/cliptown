/**
 * SkillsPanel — P2.2 minimal skills view (M12).
 *
 * Lists skills for the currently-possessed startup with attach/detach
 * controls. Skill content authoring (create/edit/delete) stays on MCP
 * tools / SQL for now; this panel covers the operator-facing attachment
 * mutation surface only.
 */
import { useMemo, type CSSProperties } from "react";
import type { WorldState, SkillVM, AvatarVM } from "../store.js";

interface Props {
  state: WorldState;
  possessedStartupId: string | null;
  onAttach: (skillId: string, agentId: string) => void;
  onDetach: (skillId: string, agentId: string) => void;
}

export function SkillsPanel({ state, possessedStartupId, onAttach, onDetach }: Props) {
  const skills = useMemo(() => {
    if (!possessedStartupId) return [];
    const inner = state.skills?.[possessedStartupId] ?? {};
    return Object.values(inner).sort((a, b) => a.name.localeCompare(b.name));
  }, [state.skills, possessedStartupId]);

  // Agents in the possessed startup (for the attach dropdown).
  const agents = useMemo(() => {
    if (!possessedStartupId) return [];
    return Object.values(state.avatars ?? {})
      .filter((a) => a.startup_id === possessedStartupId && a.role !== "operator")
      .sort((a, b) => a.role.localeCompare(b.role));
  }, [state.avatars, possessedStartupId]);

  if (!possessedStartupId) {
    return (
      <div data-testid="skills-panel" style={panelStyle}>
        <h3 style={headingStyle}>Skills</h3>
        <p style={emptyStyle}>Possess a startup to see its skills.</p>
      </div>
    );
  }

  return (
    <div data-testid="skills-panel" style={panelStyle}>
      <h3 style={headingStyle}>Skills</h3>
      {skills.length === 0 ? (
        <p style={emptyStyle}>
          No skills yet. Use the <code>skill_upsert</code> MCP tool or SQL to create one.
        </p>
      ) : (
        <ul style={listStyle}>
          {skills.map((s) => (
            <SkillRow
              key={s.id}
              skill={s}
              agents={agents}
              onAttach={(agentId) => onAttach(s.id, agentId)}
              onDetach={(agentId) => onDetach(s.id, agentId)}
            />
          ))}
        </ul>
      )}
    </div>
  );
}

interface SkillRowProps {
  skill: SkillVM;
  agents: AvatarVM[];
  onAttach: (agentId: string) => void;
  onDetach: (agentId: string) => void;
}

function SkillRow({ skill, agents, onAttach, onDetach }: SkillRowProps) {
  const unattached = agents.filter((a) => !skill.attachments.includes(a.agent_id));
  return (
    <li style={rowStyle} data-testid={`skill-row-${skill.name}`}>
      <div style={rowHeaderStyle}>
        <strong>{skill.name}</strong>
        <span style={{ color: "var(--fg-secondary)", fontSize: 11 }}>{skill.len} bytes</span>
      </div>
      <div style={attachmentsStyle}>
        {skill.attachments.length === 0 ? (
          <span style={emptyStyle}>no attachments</span>
        ) : (
          skill.attachments.map((aid) => {
            const a = agents.find((x) => x.agent_id === aid);
            const label = a ? `${a.role} (${aid.slice(0, 8)})` : aid.slice(0, 8);
            return (
              <button
                key={aid}
                style={chipStyle}
                title={`Detach ${aid}`}
                onClick={() => onDetach(aid)}
                data-testid={`skill-detach-${skill.name}-${aid}`}
              >
                {label} ×
              </button>
            );
          })
        )}
      </div>
      {unattached.length > 0 && (
        <select
          style={selectStyle}
          defaultValue=""
          onChange={(e) => {
            if (e.target.value) {
              onAttach(e.target.value);
              e.currentTarget.value = "";
            }
          }}
          data-testid={`skill-attach-${skill.name}`}
        >
          <option value="">Attach to…</option>
          {unattached.map((a) => (
            <option key={a.agent_id} value={a.agent_id}>
              {a.role} ({a.agent_id.slice(0, 8)})
            </option>
          ))}
        </select>
      )}
    </li>
  );
}

const panelStyle: CSSProperties = {
  padding: "12px 16px",
  borderTop: "1px solid var(--border)",
  background: "var(--raised)",
};

const headingStyle: CSSProperties = {
  fontSize: 12,
  fontWeight: 600,
  color: "var(--fg-secondary)",
  margin: "0 0 8px 0",
  textTransform: "uppercase",
  letterSpacing: "0.04em",
};

const emptyStyle: CSSProperties = {
  fontSize: 12,
  color: "var(--fg-secondary)",
  margin: 0,
};

const listStyle: CSSProperties = {
  listStyle: "none",
  margin: 0,
  padding: 0,
  display: "flex",
  flexDirection: "column",
  gap: 8,
};

const rowStyle: CSSProperties = {
  background: "var(--bg)",
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: "6px 10px",
  display: "flex",
  flexDirection: "column",
  gap: 4,
};

const rowHeaderStyle: CSSProperties = {
  display: "flex",
  justifyContent: "space-between",
  alignItems: "baseline",
  fontSize: 13,
};

const attachmentsStyle: CSSProperties = {
  display: "flex",
  flexWrap: "wrap",
  gap: 4,
};

const chipStyle: CSSProperties = {
  font: "inherit",
  fontSize: 11,
  background: "var(--raised)",
  border: "1px solid var(--border)",
  borderRadius: 999,
  padding: "2px 8px",
  cursor: "pointer",
  color: "var(--fg)",
};

const selectStyle: CSSProperties = {
  font: "inherit",
  fontSize: 12,
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: "2px 6px",
  background: "var(--bg)",
  color: "var(--fg)",
  cursor: "pointer",
};

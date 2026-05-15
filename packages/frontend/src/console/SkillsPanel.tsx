/**
 * SkillsPanel — P2.2 minimal skills view + P3 Theme F authoring.
 *
 * Lists skills for the currently-possessed startup with attach/detach
 * controls. P3 adds inline create / edit / delete via the new
 * `skill_upsert_operator` / `skill_delete_operator` ConsoleInbound
 * variants. Content authoring uses a plain <textarea> — markdown rendering
 * is the agent CLI's job, not the operator's preview surface.
 */
import { useMemo, useState, type CSSProperties } from "react";
import type { WorldState, SkillVM, AvatarVM } from "../store.js";

interface Props {
  state: WorldState;
  possessedStartupId: string | null;
  onAttach: (skillId: string, agentId: string) => void;
  onDetach: (skillId: string, agentId: string) => void;
  onUpsert: (name: string, contentMd: string, skillId: string | null) => void;
  onDelete: (skillId: string) => void;
}

export function SkillsPanel({
  state,
  possessedStartupId,
  onAttach,
  onDetach,
  onUpsert,
  onDelete,
}: Props) {
  const [creating, setCreating] = useState(false);
  const skills = useMemo(() => {
    if (!possessedStartupId) return [];
    const inner = state.skills?.[possessedStartupId] ?? {};
    return Object.values(inner).sort((a, b) => a.name.localeCompare(b.name));
  }, [state.skills, possessedStartupId]);

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
      <div style={headerRowStyle}>
        <h3 style={headingStyle}>Skills</h3>
        {!creating && (
          <button
            style={newButtonStyle}
            onClick={() => setCreating(true)}
            data-testid="skill-new"
          >
            + New skill
          </button>
        )}
      </div>
      {creating && (
        <SkillEditor
          mode="create"
          onSubmit={(name, content) => {
            onUpsert(name, content, null);
            setCreating(false);
          }}
          onCancel={() => setCreating(false)}
        />
      )}
      {skills.length === 0 && !creating ? (
        <p style={emptyStyle}>No skills yet. Click + New skill to add one.</p>
      ) : (
        <ul style={listStyle}>
          {skills.map((s) => (
            <SkillRow
              key={s.id}
              skill={s}
              agents={agents}
              onAttach={(agentId) => onAttach(s.id, agentId)}
              onDetach={(agentId) => onDetach(s.id, agentId)}
              onSave={(name, content) => onUpsert(name, content, s.id)}
              onDelete={() => onDelete(s.id)}
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
  onSave: (name: string, contentMd: string) => void;
  onDelete: () => void;
}

function SkillRow({ skill, agents, onAttach, onDetach, onSave, onDelete }: SkillRowProps) {
  const [editing, setEditing] = useState(false);
  const unattached = agents.filter((a) => !skill.attachments.includes(a.agent_id));
  return (
    <li style={rowStyle} data-testid={`skill-row-${skill.name}`}>
      <div style={rowHeaderStyle}>
        <strong>{skill.name}</strong>
        <div style={{ display: "flex", gap: 6 }}>
          <span style={{ color: "var(--fg-secondary)", fontSize: 11 }}>{skill.len} bytes</span>
          <button
            style={iconButtonStyle}
            onClick={() => setEditing((v) => !v)}
            data-testid={`skill-edit-${skill.name}`}
            title="Edit content"
          >
            ✎
          </button>
          <button
            style={{ ...iconButtonStyle, color: "var(--danger, #c33)" }}
            onClick={() => {
              if (confirm(`Delete skill "${skill.name}"? This cascades to all attachments.`)) {
                onDelete();
              }
            }}
            data-testid={`skill-delete-${skill.name}`}
            title="Delete skill"
          >
            ✕
          </button>
        </div>
      </div>
      {editing && (
        // Note: the WS snapshot ships skill metadata only (`len` + `updated_at`),
        // not `content_md` — re-fetching content per skill would inflate every
        // snapshot. The editor starts blank and the operator re-types or pastes
        // the new content; server's upsert is keyed by (startup_id, name) so
        // the existing skill row is updated in place.
        <SkillEditor
          mode="edit"
          initialName={skill.name}
          initialContent=""
          onSubmit={(name, content) => {
            onSave(name, content);
            setEditing(false);
          }}
          onCancel={() => setEditing(false)}
        />
      )}
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

interface SkillEditorProps {
  mode: "create" | "edit";
  initialName?: string;
  initialContent?: string;
  onSubmit: (name: string, contentMd: string) => void;
  onCancel: () => void;
}

function SkillEditor({ mode, initialName = "", initialContent = "", onSubmit, onCancel }: SkillEditorProps) {
  const [name, setName] = useState(initialName);
  const [content, setContent] = useState(initialContent);
  const trimmed = name.trim();
  const valid = trimmed.length > 0 && content.length > 0;
  return (
    <div style={editorStyle} data-testid={`skill-editor-${mode}`}>
      {mode === "create" && (
        <input
          style={inputStyle}
          placeholder="Skill name (a-z, 0-9, -)"
          value={name}
          onChange={(e) => setName(e.target.value)}
          data-testid="skill-editor-name"
        />
      )}
      <textarea
        style={textareaStyle}
        placeholder="Markdown content — describe when + how the skill applies"
        value={content}
        rows={6}
        onChange={(e) => setContent(e.target.value)}
        data-testid="skill-editor-content"
      />
      <div style={{ display: "flex", gap: 6, justifyContent: "flex-end" }}>
        <button style={smallButtonStyle} onClick={onCancel} data-testid="skill-editor-cancel">
          Cancel
        </button>
        <button
          style={{ ...smallButtonStyle, fontWeight: 600 }}
          disabled={!valid}
          onClick={() => onSubmit(trimmed, content)}
          data-testid="skill-editor-save"
        >
          Save
        </button>
      </div>
    </div>
  );
}

const panelStyle: CSSProperties = {
  padding: "12px 16px",
  borderTop: "1px solid var(--border)",
  background: "var(--raised)",
};

const headerRowStyle: CSSProperties = {
  display: "flex",
  justifyContent: "space-between",
  alignItems: "center",
  marginBottom: 8,
};

const headingStyle: CSSProperties = {
  fontSize: 12,
  fontWeight: 600,
  color: "var(--fg-secondary)",
  margin: 0,
  textTransform: "uppercase",
  letterSpacing: "0.04em",
};

const newButtonStyle: CSSProperties = {
  font: "inherit",
  fontSize: 11,
  background: "var(--bg)",
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: "2px 8px",
  cursor: "pointer",
  color: "var(--fg)",
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

const iconButtonStyle: CSSProperties = {
  font: "inherit",
  fontSize: 11,
  background: "transparent",
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: "0 6px",
  cursor: "pointer",
  color: "var(--fg)",
  lineHeight: "16px",
};

const editorStyle: CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 4,
  padding: 6,
  background: "var(--raised)",
  border: "1px dashed var(--border)",
  borderRadius: 6,
};

const inputStyle: CSSProperties = {
  font: "inherit",
  fontSize: 12,
  background: "var(--bg)",
  color: "var(--fg)",
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: "4px 8px",
};

const textareaStyle: CSSProperties = {
  font: "inherit",
  fontSize: 12,
  fontFamily: "monospace",
  background: "var(--bg)",
  color: "var(--fg)",
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: "4px 8px",
  resize: "vertical",
};

const smallButtonStyle: CSSProperties = {
  font: "inherit",
  fontSize: 11,
  background: "var(--bg)",
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: "3px 10px",
  cursor: "pointer",
  color: "var(--fg)",
};

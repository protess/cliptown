/**
 * SkillsPanel — P2.2 minimal skills view + P3 Theme F authoring.
 *
 * Lists skills for the currently-possessed startup with attach/detach
 * controls. P3 adds inline create / edit / delete via the new
 * `skill_upsert_operator` / `skill_delete_operator` ConsoleInbound
 * variants. Content authoring uses a plain <textarea> — markdown rendering
 * is the agent CLI's job, not the operator's preview surface.
 */
import { useEffect, useMemo, useState, type CSSProperties } from "react";
import type { WorldState, SkillVM, AvatarVM, SkillRevisionVM } from "../store.js";

interface Props {
  state: WorldState;
  possessedStartupId: string | null;
  onAttach: (skillId: string, agentId: string) => void;
  onDetach: (skillId: string, agentId: string) => void;
  onUpsert: (name: string, contentMd: string, skillId: string | null) => void;
  onDelete: (skillId: string) => void;
  onSetGlobal: (skillId: string, isGlobal: boolean) => void;
  /**
   * Theme G slice 4: lazy-fetch this skill's revision history. The reply
   * lands in `state.skillRevisions[skillId]` via the store reducer.
   */
  onListRevisions: (skillId: string) => void;
  /** Theme G slice 4: revert this skill to `rev_seq` (manager+ only). */
  onRevert: (skillId: string, revSeq: number) => void;
}

export function SkillsPanel({
  state,
  possessedStartupId,
  onAttach,
  onDetach,
  onUpsert,
  onDelete,
  onSetGlobal,
  onListRevisions,
  onRevert,
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
              revisions={state.skillRevisions[s.id]}
              onAttach={(agentId) => onAttach(s.id, agentId)}
              onDetach={(agentId) => onDetach(s.id, agentId)}
              onSave={(name, content) => onUpsert(name, content, s.id)}
              onDelete={() => onDelete(s.id)}
              onToggleGlobal={() => onSetGlobal(s.id, !s.is_global)}
              onListRevisions={() => onListRevisions(s.id)}
              onRevert={(revSeq) => onRevert(s.id, revSeq)}
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
  revisions: SkillRevisionVM[] | undefined;
  onAttach: (agentId: string) => void;
  onDetach: (agentId: string) => void;
  onSave: (name: string, contentMd: string) => void;
  onDelete: () => void;
  onToggleGlobal: () => void;
  onListRevisions: () => void;
  onRevert: (revSeq: number) => void;
}

function SkillRow({
  skill,
  agents,
  revisions,
  onAttach,
  onDetach,
  onSave,
  onDelete,
  onToggleGlobal,
  onListRevisions,
  onRevert,
}: SkillRowProps) {
  const [editing, setEditing] = useState(false);
  const [historyOpen, setHistoryOpen] = useState(false);
  const unattached = agents.filter((a) => !skill.attachments.includes(a.agent_id));

  // Theme G slice 4: lazy-fetch on first open. The reducer clears the
  // cache on any skill mutation, so re-opening after an edit triggers
  // a fresh fetch automatically.
  useEffect(() => {
    if (historyOpen && revisions === undefined) {
      onListRevisions();
    }
  }, [historyOpen, revisions, onListRevisions]);

  return (
    <li style={rowStyle} data-testid={`skill-row-${skill.name}`}>
      <div style={rowHeaderStyle}>
        <strong>
          {skill.name}
          {skill.is_global && (
            <span title="Visible to every agent in every startup" style={globeBadgeStyle}>
              🌐
            </span>
          )}
        </strong>
        <div style={{ display: "flex", gap: 6 }}>
          <span style={{ color: "var(--fg-secondary)", fontSize: 11 }}>{skill.len} bytes</span>
          <button
            style={{
              ...iconButtonStyle,
              color: skill.is_global ? "var(--accent, #4a90e2)" : "var(--fg-secondary)",
            }}
            onClick={onToggleGlobal}
            data-testid={`skill-global-toggle-${skill.name}`}
            title={skill.is_global ? "Clear global flag (admin)" : "Mark global (admin)"}
          >
            🌐
          </button>
          <button
            style={{
              ...iconButtonStyle,
              color: historyOpen ? "var(--accent, #4a90e2)" : "var(--fg-secondary)",
            }}
            onClick={() => setHistoryOpen((v) => !v)}
            data-testid={`skill-history-${skill.name}`}
            title="View revision history (manager+ may revert)"
          >
            ⏱
          </button>
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
      {historyOpen && (
        <RevisionsPanel
          skillName={skill.name}
          revisions={revisions}
          onRevert={(revSeq) => {
            if (
              confirm(
                `Revert "${skill.name}" to rev ${revSeq}? A new revision will be appended pointing at this historical content.`,
              )
            ) {
              onRevert(revSeq);
            }
          }}
        />
      )}
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

/**
 * Theme G slice 4: collapsible revision-history sub-panel inside a SkillRow.
 * Shows up to 20 revisions newest-first with a "Revert" button on each
 * non-current row. Author column shows the operator or agent id (short).
 * `revisions === undefined` means we're still waiting for the WS reply.
 */
function RevisionsPanel({
  skillName,
  revisions,
  onRevert,
}: {
  skillName: string;
  revisions: SkillRevisionVM[] | undefined;
  onRevert: (revSeq: number) => void;
}) {
  if (revisions === undefined) {
    return (
      <div style={revisionsPanelStyle} data-testid={`skill-revisions-${skillName}`}>
        <p style={emptyStyle}>loading history…</p>
      </div>
    );
  }
  if (revisions.length === 0) {
    return (
      <div style={revisionsPanelStyle} data-testid={`skill-revisions-${skillName}`}>
        <p style={emptyStyle}>no revisions</p>
      </div>
    );
  }
  const currentSeq = revisions.reduce((max, r) => Math.max(max, r.rev_seq), 0);
  return (
    <div style={revisionsPanelStyle} data-testid={`skill-revisions-${skillName}`}>
      <ul style={revListStyle}>
        {revisions.slice(0, 20).map((r) => {
          const isCurrent = r.rev_seq === currentSeq;
          const author = r.created_by_agent_id ?? r.created_by_operator_id ?? "?";
          return (
            <li
              key={r.id}
              style={revRowStyle}
              data-testid={`skill-revision-${skillName}-${r.rev_seq}`}
            >
              <span style={revSeqStyle}>r{r.rev_seq}</span>
              <span style={{ flex: 1, fontSize: 11, color: "var(--fg-secondary)" }}>
                {new Date(r.created_at * 1000).toLocaleString()} · {author.slice(0, 10)}
                {isCurrent && <em style={currentMarkerStyle}> · current</em>}
              </span>
              <span style={revLenStyle}>{r.content_md.length}b</span>
              {!isCurrent && (
                <button
                  style={revertButtonStyle}
                  onClick={() => onRevert(r.rev_seq)}
                  data-testid={`skill-revert-${skillName}-${r.rev_seq}`}
                  title="Restore this revision as the live content"
                >
                  Revert
                </button>
              )}
            </li>
          );
        })}
      </ul>
    </div>
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

const globeBadgeStyle: CSSProperties = {
  marginLeft: 4,
  fontSize: 11,
  opacity: 0.7,
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

const revisionsPanelStyle: CSSProperties = {
  padding: 6,
  background: "var(--raised)",
  border: "1px dashed var(--border)",
  borderRadius: 6,
};

const revListStyle: CSSProperties = {
  listStyle: "none",
  margin: 0,
  padding: 0,
  display: "flex",
  flexDirection: "column",
  gap: 3,
};

const revRowStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  fontSize: 11,
  padding: "2px 4px",
};

const revSeqStyle: CSSProperties = {
  fontFamily: "monospace",
  fontWeight: 600,
  color: "var(--fg)",
  minWidth: 28,
};

const revLenStyle: CSSProperties = {
  fontSize: 10,
  color: "var(--fg-secondary)",
};

const currentMarkerStyle: CSSProperties = {
  fontStyle: "normal",
  color: "var(--accent, #4a90e2)",
  fontWeight: 500,
};

const revertButtonStyle: CSSProperties = {
  font: "inherit",
  fontSize: 10,
  background: "var(--bg)",
  border: "1px solid var(--border)",
  borderRadius: 4,
  padding: "1px 8px",
  cursor: "pointer",
  color: "var(--fg)",
};

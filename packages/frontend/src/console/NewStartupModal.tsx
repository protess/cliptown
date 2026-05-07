/**
 * NewStartupModal: Phase 0 / M4.7 — template picker + per-role backend
 * selector that mirrors the live BackendCatalog (unavailable backends render
 * strikethrough with an install hint via title attr).
 *
 * Submit POSTs to `/api/startups`. That endpoint lands in M5.1; until then
 * the request will 404 and we surface a toast — by design so the modal flow
 * is end-to-end testable the moment M5 lands.
 *
 * The catalog shape comes from `packages/protocol/dist/BackendInfo.ts`
 * (`{ id, available, version, install_hint, ... }`); we only consume
 * `available` and `install_hint` here.
 */

import { useState } from "react";
import type { CSSProperties, ReactNode } from "react";
import { useNavigate } from "react-router-dom";
import { useWorld } from "../hooks/useWorld.js";

const API_BASE: string =
  (import.meta.env.VITE_WORLD_HTTP_URL as string | undefined) ??
  "http://127.0.0.1:8080";

interface Template {
  id: string;
  title: string;
  description: string;
  goal_text: string;
}

const TEMPLATES: ReadonlyArray<Template> = [
  {
    id: "spec_writer",
    title: "Spec writer",
    description: "Founder + engineer write a single spec.md to a goal.",
    goal_text: "Write spec.md describing how to achieve: <fill-me-in>",
  },
  {
    id: "research_brief",
    title: "Research brief",
    description: "Designer + engineer survey a topic and produce brief.md.",
    goal_text: "Research and produce brief.md on: <fill-me-in>",
  },
  {
    id: "prototype",
    title: "Prototype",
    description: "Engineer prototypes a small program; designer reviews.",
    goal_text: "Prototype: <fill-me-in>",
  },
];

const ROLES: ReadonlyArray<"founder" | "engineer" | "designer"> = [
  "founder",
  "engineer",
  "designer",
];

export function NewStartupModal({ onClose }: { onClose: () => void }) {
  const { state, addToast } = useWorld();
  const navigate = useNavigate();

  const catalogEntries = Object.entries(state.backendCatalog);
  const availableBackends: string[] = catalogEntries
    .filter(([, info]) => isAvailable(info))
    .map(([k]) => k);
  const allBackends: string[] = catalogEntries.map(([k]) => k);

  const initialBackend = availableBackends[0] ?? allBackends[0] ?? "claude_code";

  const [tpl, setTpl] = useState<Template | null>(TEMPLATES[0] ?? null);
  const [name, setName] = useState("alpha");
  const [goal, setGoal] = useState(TEMPLATES[0]?.goal_text ?? "");
  const [budget, setBudget] = useState(10);
  const [backends, setBackends] = useState<Record<string, string>>({
    founder: initialBackend,
    engineer: initialBackend,
    designer: initialBackend,
  });
  const [submitting, setSubmitting] = useState(false);

  const onPickTemplate = (t: Template | null) => {
    setTpl(t);
    if (t) setGoal(t.goal_text);
  };

  const submit = async () => {
    setSubmitting(true);
    try {
      const res = await fetch(`${API_BASE}/api/startups`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          name,
          goal_text: goal,
          budget_cap_usd: budget,
          backends,
        }),
      });
      if (!res.ok) {
        const detail = await res.text().catch(() => "");
        addToast(
          "warn",
          `Create failed (${res.status}): ${detail || "see logs"}`,
        );
        return;
      }
      const body = (await res.json()) as { id?: string };
      const id = body.id;
      onClose();
      if (id) navigate(`/town/${id}`);
    } catch (e) {
      addToast(
        "warn",
        `Create failed: ${e instanceof Error ? e.message : String(e)}`,
      );
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div
      onClick={onClose}
      role="dialog"
      aria-label="New startup"
      style={overlayStyle}
    >
      <div onClick={(e) => e.stopPropagation()} style={panelStyle}>
        <header
          style={{
            display: "flex",
            justifyContent: "space-between",
            alignItems: "baseline",
            marginBottom: 12,
          }}
        >
          <h2 style={{ margin: 0, fontWeight: 700 }}>New startup</h2>
          <button onClick={onClose} style={closeBtn}>
            Close
          </button>
        </header>

        <section style={{ marginBottom: 16 }}>
          <label style={labelStyle}>Template</label>
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(auto-fill, minmax(180px, 1fr))",
              gap: 8,
            }}
          >
            {TEMPLATES.map((t) => (
              <TemplateCard
                key={t.id}
                tpl={t}
                active={tpl?.id === t.id}
                onClick={() => onPickTemplate(t)}
              />
            ))}
            <TemplateCard
              tpl={null}
              active={tpl === null}
              onClick={() => onPickTemplate(null)}
            />
          </div>
        </section>

        <section
          style={{
            display: "grid",
            gridTemplateColumns: "1fr 1fr",
            gap: 12,
            marginBottom: 16,
          }}
        >
          <Field label="Name">
            <input
              value={name}
              onChange={(e) => setName(e.target.value)}
              style={inputStyle}
            />
          </Field>
          <Field label="Budget cap (USD)">
            <input
              type="number"
              min="0"
              step="1"
              value={budget}
              onChange={(e) => setBudget(parseFloat(e.target.value || "0"))}
              style={inputStyle}
            />
          </Field>
        </section>

        <section style={{ marginBottom: 16 }}>
          <Field label="Goal">
            <textarea
              value={goal}
              onChange={(e) => setGoal(e.target.value)}
              rows={3}
              style={{ ...inputStyle, fontFamily: "inherit" }}
            />
          </Field>
        </section>

        <section style={{ marginBottom: 16 }}>
          <label style={labelStyle}>Backends</label>
          {allBackends.length === 0 && (
            <p style={{ color: "var(--fg-secondary)", fontSize: 12 }}>
              Catalog empty — try Recheck Backends in the top bar.
            </p>
          )}
          {ROLES.map((role) => (
            <div
              key={role}
              style={{
                display: "grid",
                gridTemplateColumns: "100px 1fr",
                marginBottom: 6,
                alignItems: "center",
              }}
            >
              <span style={{ fontSize: 13, color: "var(--fg-secondary)" }}>
                {role}
              </span>
              <div style={{ display: "flex", gap: 12, flexWrap: "wrap" }}>
                {allBackends.map((b) => {
                  const info = state.backendCatalog[b];
                  const enabled = isAvailable(info);
                  const hint = installHint(info);
                  return (
                    <label
                      key={b}
                      title={
                        enabled
                          ? b
                          : hint
                            ? `${b} not installed — ${hint}`
                            : `${b} not installed`
                      }
                      style={{
                        display: "inline-flex",
                        alignItems: "center",
                        gap: 4,
                        textDecoration: enabled ? "none" : "line-through",
                        color: enabled ? "inherit" : "var(--fg-secondary)",
                        fontSize: 13,
                      }}
                    >
                      <input
                        type="radio"
                        name={`backend-${role}`}
                        value={b}
                        disabled={!enabled}
                        checked={backends[role] === b}
                        onChange={() =>
                          setBackends((p) => ({ ...p, [role]: b }))
                        }
                      />
                      <code>{b}</code>
                    </label>
                  );
                })}
              </div>
            </div>
          ))}
        </section>

        <footer
          style={{ display: "flex", justifyContent: "flex-end", gap: 8 }}
        >
          <button onClick={onClose} style={secondaryBtn}>
            Cancel
          </button>
          <button
            onClick={submit}
            disabled={
              submitting ||
              !name ||
              !goal ||
              availableBackends.length === 0
            }
            style={primaryBtn}
          >
            {submitting ? "Creating…" : "Create"}
          </button>
        </footer>
      </div>
    </div>
  );
}

function TemplateCard({
  tpl,
  active,
  onClick,
}: {
  tpl: Template | null;
  active: boolean;
  onClick: () => void;
}) {
  const title = tpl ? tpl.title : "Start blank";
  const desc = tpl ? tpl.description : "Free-form goal text.";
  return (
    <button
      onClick={onClick}
      style={{
        textAlign: "left",
        background: active ? "rgba(0,0,0,0.04)" : "var(--raised)",
        border: `1px solid ${active ? "var(--fg)" : "var(--border)"}`,
        borderRadius: 6,
        padding: 8,
        cursor: "pointer",
        font: "inherit",
      }}
    >
      <div style={{ fontWeight: 600, marginBottom: 4 }}>{title}</div>
      <div style={{ fontSize: 12, color: "var(--fg-secondary)" }}>{desc}</div>
    </button>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div>
      <div style={labelStyle}>{label}</div>
      {children}
    </div>
  );
}

function isAvailable(info: unknown): boolean {
  if (info && typeof info === "object" && "available" in info) {
    return Boolean((info as { available?: unknown }).available);
  }
  return false;
}

function installHint(info: unknown): string | null {
  if (info && typeof info === "object" && "install_hint" in info) {
    const v = (info as { install_hint?: unknown }).install_hint;
    return typeof v === "string" ? v : null;
  }
  return null;
}

const overlayStyle: CSSProperties = {
  position: "fixed",
  inset: 0,
  background: "rgba(0,0,0,0.4)",
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  zIndex: 100,
};

const panelStyle: CSSProperties = {
  background: "var(--raised)",
  borderRadius: 8,
  padding: 20,
  width: "min(640px, 92vw)",
  maxHeight: "92vh",
  overflow: "auto",
  boxShadow: "0 8px 24px rgba(0,0,0,0.16)",
};

const labelStyle: CSSProperties = {
  display: "block",
  fontSize: 12,
  color: "var(--fg-secondary)",
  marginBottom: 4,
};

const inputStyle: CSSProperties = {
  width: "100%",
  boxSizing: "border-box",
  font: "inherit",
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: "6px 8px",
  background: "var(--bg)",
};

const closeBtn: CSSProperties = {
  font: "inherit",
  background: "transparent",
  border: "none",
  cursor: "pointer",
};

const primaryBtn: CSSProperties = {
  font: "inherit",
  background: "var(--fg)",
  color: "var(--bg)",
  border: "none",
  borderRadius: 6,
  padding: "6px 14px",
  cursor: "pointer",
};

const secondaryBtn: CSSProperties = {
  font: "inherit",
  background: "var(--raised)",
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: "6px 14px",
  cursor: "pointer",
};

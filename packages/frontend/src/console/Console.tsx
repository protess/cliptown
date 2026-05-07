import { useWorld } from "../hooks/useWorld.js";

export function Console() {
  const { state } = useWorld();
  const startupCount = Object.keys(state.startups).length;
  const avatarCount = Object.keys(state.avatars).length;
  return (
    <div style={{ padding: 24 }}>
      <h1 style={{ fontWeight: 700 }}>cliptown</h1>
      <p style={{ color: "var(--fg-secondary)" }}>
        Status: <code>{state.status}</code> · {startupCount} startup(s) ·{" "}
        {avatarCount} avatar(s)
      </p>
      {state.systemEvents.length > 0 && (
        <ul style={{ paddingLeft: 18, color: "var(--fg-secondary)" }}>
          {state.systemEvents.slice(0, 3).map((e, i) => (
            <li key={i}>
              <code>{e.severity}</code> {e.kind}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

import { useWorld } from "../hooks/useWorld.js";
import { TopBar } from "./TopBar.js";

export function Console() {
  const { state } = useWorld();
  const startupCount = Object.keys(state.startups).length;
  const avatarCount = Object.keys(state.avatars).length;
  return (
    <div style={{ minHeight: "100vh" }}>
      <TopBar />
      <main style={{ padding: 24 }}>
        <p style={{ color: "var(--fg-secondary)" }}>
          Status: <code>{state.status}</code> · {startupCount} startup(s) ·{" "}
          {avatarCount} avatar(s)
        </p>
      </main>
    </div>
  );
}

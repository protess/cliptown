import { useWorld } from "../hooks/useWorld.js";
import { TopBar } from "./TopBar.js";
import { Sidebar } from "./Sidebar.js";

export function Console() {
  const { state } = useWorld();
  const startupCount = Object.keys(state.startups).length;
  const avatarCount = Object.keys(state.avatars).length;
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
        <Sidebar />
        <main style={{ padding: 24, overflow: "auto" }}>
          <p style={{ color: "var(--fg-secondary)" }}>
            Status: <code>{state.status}</code> · {startupCount} startup(s) ·{" "}
            {avatarCount} avatar(s)
          </p>
        </main>
      </div>
    </div>
  );
}

import { useState } from "react";
import { useWorld } from "../hooks/useWorld.js";
import { TopBar } from "./TopBar.js";
import { Sidebar } from "./Sidebar.js";
import { MainHeader } from "./MainHeader.js";

export function Console() {
  const { state } = useWorld();
  const [selected, setSelected] = useState<string | null>(null);
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
        <Sidebar selected={selected} onSelect={setSelected} />
        <main
          style={{
            display: "flex",
            flexDirection: "column",
            minHeight: 0,
            overflow: "hidden",
          }}
        >
          <MainHeader startupId={selected} />
          <div style={{ padding: 24, overflow: "auto", flex: 1 }}>
            <p style={{ color: "var(--fg-secondary)" }}>
              Status: <code>{state.status}</code> · Kanban lands in M4.6.
            </p>
          </div>
        </main>
      </div>
    </div>
  );
}

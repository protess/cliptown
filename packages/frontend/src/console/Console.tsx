/**
 * Console — the /console route. Hosts the top bar, the startup sidebar, the
 * main pane (header + Kanban), and the floating chat panel.
 *
 * Phase 0 / M4.13 — selection is read from the WorldProvider context so the
 * global keymap (j/k cycle, `t` open-town) can mutate it without prop
 * drilling.
 */
import { useWorld } from "../hooks/useWorld.js";
import { TopBar } from "./TopBar.js";
import { Sidebar } from "./Sidebar.js";
import { MainHeader } from "./MainHeader.js";
import { Kanban } from "./Kanban.js";
import { ChatPanel } from "./ChatPanel.js";

export function Console() {
  const { selectedStartupId, setSelectedStartupId } = useWorld();
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
        </main>
      </div>
      <ChatPanel />
    </div>
  );
}

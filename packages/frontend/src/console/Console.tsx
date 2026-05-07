import { useState } from "react";
import { TopBar } from "./TopBar.js";
import { Sidebar } from "./Sidebar.js";
import { MainHeader } from "./MainHeader.js";
import { Kanban } from "./Kanban.js";

export function Console() {
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
          <div style={{ overflow: "auto", flex: 1 }}>
            <Kanban startupId={selected} />
          </div>
        </main>
      </div>
    </div>
  );
}

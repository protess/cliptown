import { useState } from "react";
import { useParams } from "react-router-dom";
import { TownTopBar } from "./TownTopBar.js";
import { PixiStage } from "./PixiStage.js";
import { ChatPanel } from "../console/ChatPanel.js";
import { AgentPopover } from "./AgentPopover.js";

interface PopoverState {
  agentId: string;
  x: number;
  y: number;
}

export function Town() {
  const { id } = useParams<{ id: string }>();
  const [popover, setPopover] = useState<PopoverState | null>(null);
  if (!id) return <p style={{ padding: 24 }}>Missing town id.</p>;
  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        minHeight: "100vh",
      }}
    >
      <TownTopBar startupId={id} />
      <main style={{ flex: 1, padding: 24, background: "var(--bg)" }}>
        <PixiStage
          startupId={id}
          onAvatarClick={(agentId, x, y) => setPopover({ agentId, x, y })}
        />
      </main>
      <ChatPanel selectedAgentId={popover?.agentId ?? null} />
      {popover && (
        <AgentPopover
          agentId={popover.agentId}
          anchorX={popover.x}
          anchorY={popover.y}
          onClose={() => setPopover(null)}
        />
      )}
    </div>
  );
}

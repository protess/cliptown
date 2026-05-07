import { useParams } from "react-router-dom";
import { TownTopBar } from "./TownTopBar.js";
import { PixiStage } from "./PixiStage.js";

export function Town() {
  const { id } = useParams<{ id: string }>();
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
          onAvatarClick={(agentId) => {
            // M4.11 wires the agent popover; for now we just log so the click
            // wire-through is observable in dev.
            // eslint-disable-next-line no-console
            console.log("[town] avatar clicked:", agentId);
          }}
        />
      </main>
    </div>
  );
}

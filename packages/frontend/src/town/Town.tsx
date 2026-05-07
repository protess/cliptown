import { useParams } from "react-router-dom";
import { TownTopBar } from "./TownTopBar.js";

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
      <main
        style={{ flex: 1, padding: 24, color: "var(--fg-secondary)" }}
      >
        Pixi canvas lands in M4.9.
      </main>
    </div>
  );
}

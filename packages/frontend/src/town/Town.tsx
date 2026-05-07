import { useParams } from "react-router-dom";

export function Town() {
  const { id } = useParams<{ id: string }>();
  return (
    <div style={{ padding: 24 }}>
      <h1 style={{ fontWeight: 700 }}>town/{id}</h1>
      <p style={{ color: "var(--fg-secondary)" }}>
        Pixi canvas lands in M4.9.
      </p>
    </div>
  );
}

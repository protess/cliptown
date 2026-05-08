import { Routes, Route, Navigate } from "react-router-dom";
import { Console } from "./console/Console.js";
import { Town } from "./town/Town.js";
import { KeymapManager } from "./keymap.js";

export function App() {
  return (
    <>
      <KeymapManager />
      <Routes>
        <Route path="/" element={<Navigate to="/console" replace />} />
        <Route path="/console" element={<Console />} />
        <Route path="/town/:id" element={<Town />} />
      </Routes>
    </>
  );
}

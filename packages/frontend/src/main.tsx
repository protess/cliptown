import React from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import { App } from "./App.js";
import { WorldProvider } from "./hooks/useWorld.js";
import "./styles/focus.css";

const root = document.getElementById("root");
if (!root) throw new Error("missing #root");
createRoot(root).render(
  <React.StrictMode>
    <BrowserRouter>
      <WorldProvider>
        <App />
      </WorldProvider>
    </BrowserRouter>
  </React.StrictMode>,
);

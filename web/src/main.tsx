import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { Providers } from "./app/Providers";
import "./styles/index.css";

const root = document.getElementById("root");
if (!root) {
  throw new Error("missing #root element");
}

createRoot(root).render(
  <StrictMode>
    <Providers />
  </StrictMode>,
);

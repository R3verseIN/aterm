/**
 * main.tsx — Application entry point for the aterm frontend.
 *
 * This file is referenced by the root `index.html` (`<script type="module" src="/src/main.tsx">`)
 * and is responsible for bootstrapping React. It:
 * 1. Imports the global stylesheet (`styles.css`) which includes Tailwind and terminal styles.
 * 2. Locates the `#root` container in the DOM (created by index.html).
 * 3. Creates a React 18 root via `ReactDOM.createRoot` and renders `<App />` inside
 *    `<React.StrictMode>` for additional dev checks (double-invoke of effects, etc.).
 *
 * The guard `if (rootElement)` ensures the app does not crash if the HTML is misconfigured
 * (e.g., during Vite HMR or if index.html is missing #root). In that case nothing renders,
 * and the error will be visible in the console rather than an uncaught exception.
 */
import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import "./styles.css";

// Locate the root container created by index.html. The background color is pre-set
// inline in index.html to avoid a flash of white before React mounts.
const rootElement = document.getElementById("root");
if (rootElement) {
  ReactDOM.createRoot(rootElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>
  );
}

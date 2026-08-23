/**
 * vite.config.ts — Vite build and dev server configuration for aterm.
 *
 * Key decisions:
 * - `@vitejs/plugin-react` enables Fast Refresh (HMR) for React components during `vite dev`
 *   and JSX transformation during `vite build`. Required for Tauri's devUrl (http://localhost:1420).
 * - `@tailwindcss/vite` is the official Tailwind 4 Vite plugin; it processes `@import "tailwindcss"`
 *   in styles.css without needing a separate PostCSS config. This keeps the build minimal.
 * - `clearScreen: false` prevents Vite from clearing the terminal on rebuilds, which would hide
 *   Rust/Tauri logs when running `tauri dev` (which spawns both vite and cargo).
 * - `server.port: 1420` + `strictPort: true` matches `src-tauri/tauri.conf.json` devUrl.
 *   Tauri expects the frontend exactly at 1420; strictPort ensures a collision fails fast
 *   rather than silently binding to another port where Tauri would not find it.
 * - `server.watch.ignored` excludes Rust/target and Tauri build output from Vite's file watcher
 *   to avoid OOM or infinite rebuild loops (target can contain 100k+ files).
 * - `envPrefix: ["VITE_"]` ensures only VITE_ prefixed env vars are exposed to the client
 *   (default Vite behavior). We intentionally do not expose TAURI_ vars to the frontend bundle.
 */
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  // Force pre-bundle html2canvas-pro (Tailwind 4 oklch fix) — Vite 6 caches
  // deps in node_modules/.vite; swapping html2canvas → html2canvas-pro
  // leaves stale html2canvas.js without --force. Explicit include ensures
  // the new ESM is optimized correctly.
  optimizeDeps: { include: ["html2canvas-pro"] },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**", "**/build/**", "**/target/**", "**/.github/**", "**/.git/**"],
    },
  },
  envPrefix: ["VITE_"],
});

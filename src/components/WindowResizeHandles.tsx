import React, { useCallback } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * WindowResizeHandles — frameless window resize affordance for aterm.
 *
 * Problem:
 * - `tauri.conf.json` sets `decorations:false` (frameless) + `resizable:true`.
 *   On Linux/Wayland the compositor draws no server-side resize border, so the
 *   OS hit-test region is effectively 0 px. The window appears non-resizable
 *   even though `resizable:true` is set. The top 38 px `Tabbar` is also a
 *   `data-tauri-drag-region` that swallows top-edge resize attempts.
 * - Tauri's `resizable:true` alone only sets `GTK_WINDOW_RESIZABLE`; without
 *   explicit handles the user has nowhere to grab.
 *
 * Solution:
 * - Render 8 invisible hit areas (4 edges + 4 corners) as `position:fixed`
 *   strips with `6 px` thickness (8 px for corners) sitting on the viewport
 *   edges. Each has `-webkit-app-region: no-drag` so Tauri's drag region
 *   does not swallow the resize gesture, and the correct CSS cursor
 *   (`ns-resize`, `ew-resize`, `nwse-resize`, `nesw-resize`).
 * - On `mousedown` (left button only) we call Tauri's native
 *   `getCurrentWindow().startResizeDragging(direction)` with the matching
 *   `ResizeDirection` (`North`/`South`/`East`/`West`/`NorthEast` etc.).
 *   The OS then takes over the drag — no manual `setSize` tracking, no
 *   `mousemove` listeners, no jank. This is the Tauri 2 recommended path
 *   (`Window.startResizeDragging` added in #8537).
 * - Falls back silently in browser preview (Vite) where `@tauri-apps/api`
 *   is absent — `getCurrentWindow()` throws, we catch and ignore.
 *
 * Placement notes:
 * - `fixed` (not `absolute`) so the handles stay on the *viewport* edges even
 *   though `App` is `overflow:hidden` flex. They sit above the terminal but
 *   below the context menu (`z-index:40`, vs menu `z-60`). The container is
 *   `pointer-events:none`; each handle re-enables `pointer-events:auto`.
 * - `zIndex:40` is above `#terminal-container` flex but below `TabContextMenu`
 *   (`zIndex:60` in TabContextMenu.tsx) so right-click menu still receives
 *   clicks. Edges are 6 px, corners 12 px for easier grab (Fitts).
 *
 * Permissions:
 * - Requires `core:window:allow-start-resize-dragging` in
 *   `src-tauri/capabilities/default.json`. Without it Tauri will deny
 *   `startResizeDragging` with `permission denied` in devtools.
 *
 * Security/layout:
 * - Handles are purely local window chrome — no data handling, no network.
 * - `app-region:no-drag` ensures terminal scrollbar drag is not swallowed
 *   (`styles.css` already forces `.terminal-wrapper` etc. to `no-drag`).
 */
type ResizeDir = "North" | "South" | "East" | "West" | "NorthEast" | "NorthWest" | "SouthEast" | "SouthWest";

export const WindowResizeHandles: React.FC = () => {
  /**
   * Trigger native OS resize drag for a given direction.
   * Left-button only; preventDefault/stopPropagation so the event doesn't
   * also trigger `Tabbar` drag or `TerminalView` xterm selection.
   */
  const start = useCallback((dir: ResizeDir, e: React.MouseEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    try {
      const win = getCurrentWindow();
      // Tauri's startResizeDragging is async but we don't await — the OS
      // takes over synchronously; awaiting is only for error handling.
      void win.startResizeDragging(dir as unknown as Parameters<typeof win.startResizeDragging>[0]).catch(() => {});
    } catch {
      // Browser/Vite preview — Tauri API absent, silently ignore.
    }
  }, []);

  // Common style base: invisible but hit-testable, no-drag so Tauri doesn't
  // treat it as window-move drag.
  const base: React.CSSProperties = {
    position: "fixed",
    zIndex: 40,
    pointerEvents: "auto",
  } as const;

  return (
    <div
      aria-hidden
      style={{
        position: "fixed",
        inset: 0,
        zIndex: 40,
        pointerEvents: "none",
      }}
    >
      {/* North edge — 6 px strip, ns-resize. Sits above Tabbar but with lower z
          than tab buttons so buttons remain clickable; we inset left/right by
          8 px so corners own the intersection. */}
      <div
        role="presentation"
        aria-label="Resize north"
        onMouseDown={(e) => start("North", e)}
        style={{ ...base, top: 0, left: 12, right: 12, height: 6, cursor: "ns-resize" }}
        className="resize-handle no-drag"
      />
      {/* South edge */}
      <div
        role="presentation"
        aria-label="Resize south"
        onMouseDown={(e) => start("South", e)}
        style={{ ...base, bottom: 0, left: 12, right: 12, height: 6, cursor: "ns-resize" }}
        className="resize-handle no-drag"
      />
      {/* West edge */}
      <div
        role="presentation"
        aria-label="Resize west"
        onMouseDown={(e) => start("West", e)}
        style={{ ...base, top: 12, bottom: 12, left: 0, width: 6, cursor: "ew-resize" }}
        className="resize-handle no-drag"
      />
      {/* East edge */}
      <div
        role="presentation"
        aria-label="Resize east"
        onMouseDown={(e) => start("East", e)}
        style={{ ...base, top: 12, bottom: 12, right: 0, width: 6, cursor: "ew-resize" }}
        className="resize-handle no-drag"
      />
      {/* Corners — 12×12 px for easier grab, diagonal cursors. Corner handles
          take precedence over edge strips at intersections. */}
      <div
        role="presentation"
        aria-label="Resize north-west"
        onMouseDown={(e) => start("NorthWest", e)}
        style={{ ...base, top: 0, left: 0, width: 12, height: 12, cursor: "nwse-resize" }}
        className="resize-handle no-drag"
      />
      <div
        role="presentation"
        aria-label="Resize north-east"
        onMouseDown={(e) => start("NorthEast", e)}
        style={{ ...base, top: 0, right: 0, width: 12, height: 12, cursor: "nesw-resize" }}
        className="resize-handle no-drag"
      />
      <div
        role="presentation"
        aria-label="Resize south-west"
        onMouseDown={(e) => start("SouthWest", e)}
        style={{ ...base, bottom: 0, left: 0, width: 12, height: 12, cursor: "nesw-resize" }}
        className="resize-handle no-drag"
      />
      <div
        role="presentation"
        aria-label="Resize south-east"
        onMouseDown={(e) => start("SouthEast", e)}
        style={{ ...base, bottom: 0, right: 0, width: 12, height: 12, cursor: "nwse-resize" }}
        className="resize-handle no-drag"
      />
    </div>
  );
};

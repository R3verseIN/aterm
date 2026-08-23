import React, { useCallback } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

/** Frameless resize handles — 8 invisible `fixed` strips (edges 6px, corners 12px) calling `startResizeDragging`. */
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

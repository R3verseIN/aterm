/**
 * App.tsx — Main application component for aterm.
 *
 * Responsibilities:
 * - Maintains global state: tabs (sessions), active tab, config (theme/font), settings drawer visibility.
 * - Orchestrates Tauri IPC: get_config/save_config, create_session/close_session/write/resize, get_cwd.
 * - Provides chrome: Tabbar (tabs + window controls + drag region) and SettingsDrawer.
 * - Renders one TerminalView per tab, showing only the active one (display:none for others to keep PTY buffers alive).
 * - Handles global keyboard and wheel shortcuts: Ctrl+Shift+T/W for tabs, Ctrl+/- / Ctrl+Wheel for font zoom,
 *   Ctrl+0 to reset, plus window minimize/maximize/close via Tauri window API.
 *
 * Design notes:
 * - Frameless window (decorations:false in tauri.conf.json) — all chrome is custom.
 * - `parseConfig` ensures backend config (camelCase, possibly old snake_case or corrupt) is always valid Zod.
 * - `handleNewTab` inherits cwd from the active tab via `get_cwd` so `+` opens "here".
 * - `TerminalView` owns the xterm instance; App only passes config/theme and handles tab lifecycle.
 */
import React, { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Tabbar } from "./components/Tabbar";
import { TerminalView } from "./components/TerminalView";
import { SettingsDrawer } from "./components/SettingsDrawer";
import { TabContextMenu } from "./components/TabContextMenu";
import { WindowResizeHandles } from "./components/WindowResizeHandles";
import { Tab, ThemeColors, ThemeName } from "./types/terminal";
import { ConfigType, parseConfig } from "./schemas/configSchema";
import { buildAgentPrompt } from "./utils/agentPrompt";

/**
 * SharedInfo — mirrors Rust `server::SharedInfo` (state.rs).
 * - id: PTY UUID this HTTP server is scoped to (port is the capability)
 * - port: random high port assigned via 127.0.0.1:0 (e.g., 42817)
 * - url: full base URL `http://127.0.0.1:{port}` scoped to that tab
 * The frontend keeps a Map<id, SharedInfo> so the tab bar can show a badge
 * and the context menu can offer Copy URL / Copy Agent Prompt without
 * re-invoking the backend.
 */
interface SharedInfo {
  id: string;
  port: number;
  url: string;
}

/**
 * ContextMenuState — position and target for the right-click tab menu.
 * - id: tab UUID that was right-clicked
 * - x/y: viewport coordinates from the `contextmenu` MouseEvent (clientX/Y)
 * Null when no menu is open. `TabContextMenu` renders as fixed at (x,y) and
 * clamps to viewport (innerWidth-220, innerHeight-180).
 */
interface ContextMenuState {
  id: string;
  x: number;
  y: number;
}

/**
 * Static theme palettes — maps ThemeName to xterm ITheme-compatible colors.
 * Each palette defines ANSI 16 + cursor/selection/background to fully theme the terminal.
 * `currentThemeColors` is derived from `config.theme` and passed to every TerminalView.
 */
const themes: Record<ThemeName, ThemeColors> = {
  "aterm-dark": {
    background: "#141416",
    foreground: "#e4e4e7",
    cursor: "#60a5fa",
    cursorAccent: "#141416",
    selectionBackground: "#27272a",
    black: "#18181b",
    red: "#f87171",
    green: "#4ade80",
    yellow: "#facc15",
    blue: "#60a5fa",
    magenta: "#c084fc",
    cyan: "#38bdf8",
    white: "#e4e4e7",
    brightBlack: "#52525b",
    brightRed: "#f87171",
    brightGreen: "#4ade80",
    brightYellow: "#facc15",
    brightBlue: "#60a5fa",
    brightMagenta: "#c084fc",
    brightCyan: "#38bdf8",
    brightWhite: "#ffffff",
  },
  "aterm-light": {
    background: "#f4f4f5",
    foreground: "#18181b",
    cursor: "#2563eb",
    cursorAccent: "#ffffff",
    selectionBackground: "#e4e4e7",
    black: "#18181b",
    red: "#dc2626",
    green: "#16a34a",
    yellow: "#ca8a04",
    blue: "#2563eb",
    magenta: "#9333ea",
    cyan: "#0891b2",
    white: "#d4d4d8",
    brightBlack: "#71717a",
    brightRed: "#ef4444",
    brightGreen: "#22c55e",
    brightYellow: "#eab308",
    brightBlue: "#3b82f6",
    brightMagenta: "#a855f7",
    brightCyan: "#06b6d4",
    brightWhite: "#ffffff",
  },
  nord: {
    background: "#2e3440",
    foreground: "#eceff4",
    cursor: "#88c0d0",
    cursorAccent: "#2e3440",
    selectionBackground: "#434c5e",
    black: "#3b4252",
    red: "#bf616a",
    green: "#a3be8c",
    yellow: "#ebcb8b",
    blue: "#81a1c1",
    magenta: "#b48ead",
    cyan: "#88c0d0",
    white: "#e5e9f0",
    brightBlack: "#4c566a",
    brightRed: "#bf616a",
    brightGreen: "#a3be8c",
    brightYellow: "#ebcb8b",
    brightBlue: "#81a1c1",
    brightMagenta: "#b48ead",
    brightCyan: "#8fbcbb",
    brightWhite: "#eceff4",
  },
};

export const App: React.FC = () => {
  // List of open tabs/sessions — each has id (PTY UUID) and title (updated via onTitleChange).
  const [tabs, setTabs] = useState<Tab[]>([]);
  // Currently active tab id — controls which TerminalView is display:block vs none.
  const [activeId, setActiveId] = useState<string | null>(null);
  // Global config — theme, fontSize, shell, fontFamily, scrollback. Loaded from backend on mount,
  // mutated via zoom shortcuts or SettingsDrawer save, persisted via save_config.
  const [config, setConfig] = useState<ConfigType>(() => parseConfig({}));
  // Settings drawer visibility
  const [isSettingsOpen, setIsSettingsOpen] = useState<boolean>(false);
  /**
   * Per-tab share state — Map from tab id to `{port, url}` for tabs that are
   * currently exposed via right-click Share (server/share.rs `share_tab`).
   * The port IS the capability: `http://127.0.0.1:{port}` controls only that tab.
   * Stored as a Map for O(1) lookup in Tabbar/TabItem badge and context menu.
   * Hydrated on mount via `list_shares` and mutated by handleShare/handleUnshare.
   * No auth — localhost-only binding (127.0.0.1:0) is the security boundary.
   */
  const [shares, setShares] = useState<Map<string, SharedInfo>>(new Map());
  /**
   * Right-click context menu state — null when closed, or `{id,x,y}` for the
   * tab that was right-clicked. `onContextMenu` in TabItem sets this; Tabbar
   * forwards it to App so we can render the single global `TabContextMenu`.
   * Menu offers Share / Copy Terminal URL / Copy Agent Prompt / Unshare.
   */
  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null);

  /** Lazy getter for the Tauri window — safe to call in browser (vite preview) where Tauri is absent. */
  const getAppWindow = useCallback(() => {
    try {
      return getCurrentWindow();
    } catch {
      return null;
    }
  }, []);

  /** Load persisted config on mount. Backend returns camelCase (serde rename_all); parseConfig
   * handles validation, defaults, and migration from old snake_case files. */
  useEffect(() => {
    invoke<unknown>("get_config")
      .then((cfg) => {
        if (cfg) setConfig(parseConfig(cfg));
      })
      .catch(console.warn);
  }, []);

  /**
   * Hydrate per-tab share state on mount — recovers which tabs are already
   * exposed if the frontend reloaded (e.g., HMR or Tauri reload) while the
   * Rust `SHARES` map still holds live listeners. `list_shares` returns
   * `Vec<SharedInfo>` for all currently bound ports; we store as Map for
   * badge rendering in TabItem without extra IPC per tab.
   */
  useEffect(() => {
    invoke<SharedInfo[]>("list_shares")
      .then((list) => {
        if (Array.isArray(list) && list.length > 0) {
          setShares(new Map(list.map((s) => [s.id, s])));
        }
      })
      .catch(() => {});
  }, []);

  /**
   * Create a new PTY session tab — inherits cwd from the active tab.
   *
   * 1. Computes cols/rows from window dimensions and current fontSize (charW ≈ 0.6*fontSize,
   *    charH ≈ 1.2*fontSize+4 — empirical xterm metrics). Falls back to 80x24.
   * 2. Queries `get_cwd` for the active tab's shell cwd (/proc/<pid>/cwd on Linux). If that
   *    fails (no active tab, session not found, shell exited), cwd stays null and the backend
   *    spawns in the parent process cwd (HOME when launched from desktop).
   * 3. Invokes `create_session` with cols/rows/cwd, receives a UUID, appends a Tab, and activates it.
   * 4. On IPC failure, creates a fallback offline tab so the UI remains usable.
   */
  const handleNewTab = useCallback(async () => {
    let cols = 80;
    let rows = 24;
    try {
      const charW = Math.max(1, config.fontSize * 0.6);
      const charH = Math.max(1, config.fontSize * 1.0);
      cols = Math.max(20, Math.floor(window.innerWidth / charW));
      rows = Math.max(10, Math.floor((window.innerHeight - 38) / charH));
    } catch {}
    cols = Math.floor(Number.isFinite(cols) && cols > 0 ? cols : 80);
    rows = Math.floor(Number.isFinite(rows) && rows > 0 ? rows : 24);

    // Inherit cwd from the active tab so `+` opens "here" (like GNOME Terminal)
    let cwd: string | null = null;
    if (activeId) {
      try {
        cwd = await invoke<string>("get_cwd", { id: activeId });
      } catch {}
    }

    try {
      const id = await invoke<string>("create_session", { cols, rows, cwd });
      // Use functional update for title so concurrent rapid creates (Ctrl+Shift+T
      // hammer) don't both compute `tab 1` from a stale `tabs.length` closure.
      // `prev.length+1` is always the next visible index even if two
      // `handleNewTab` calls interleave before React commits.
      setTabs((prev) => [...prev, { id, title: `tab ${prev.length + 1}` }]);
      setActiveId(id);
    } catch (e) {
      console.error("create_session failed:", e);
      const fallbackId = `fallback-${Date.now()}`;
      setTabs((prev) => [...prev, { id: fallbackId, title: `tab (offline)` }]);
      setActiveId(fallbackId);
    }
    // `tabs.length` is intentionally NOT a dep for correctness via functional
    // updates above — adding it would recreate `handleNewTab` on every new tab
    // and, if ever listed in a deps array, cause the boot effect to loop.
  }, [config.fontSize, activeId]);

  /** StrictMode double-mount guard — `hasBootedRef` ensures `create_session` runs once. */
  const hasBootedRef = React.useRef(false);
  useEffect(() => {
    if (hasBootedRef.current) return;
    hasBootedRef.current = true;
    handleNewTab();
  }, []);

  /**
   * Close a tab: stops propagation (so tab select doesn't fire), invokes `close_session`
   * to kill the PTY and remove it from the Rust SESSIONS map, then filters it from state.
   * If the closed tab was active, activates the last remaining tab (or null if none).
   * Also used as the handler for `pty:exit:{id}` (shell exited on its own).
   * Side-effect: also removes any per-tab share entry (port/unshare) so the badge
   * and context menu reflect the closed state. `close_session` already unshares
   * on the Rust side (pty::close_session → server::unshare_tab), but we also
   * purge the local `shares` Map optimistically for instant UI feedback.
   */
  const handleCloseTab = useCallback(
    async (id: string, e?: React.MouseEvent) => {
      if (e) e.stopPropagation();
      try {
        await invoke("close_session", { id });
      } catch (err) {
        console.warn(err);
      }

      // Optimistically remove share entry — Rust already aborted the listener.
      setShares((prev) => {
        if (!prev.has(id)) return prev;
        const next = new Map(prev);
        next.delete(id);
        return next;
      });

      setTabs((prevTabs) => {
        const nextTabs = prevTabs.filter((t) => t.id !== id);
        if (nextTabs.length > 0 && activeId === id) {
          setActiveId(nextTabs[nextTabs.length - 1].id);
        }
        return nextTabs;
      });
    },
    [activeId]
  );

  /**
   * Share a tab — binds a dedicated Axum listener on a random high port
   * (127.0.0.1:0) scoped to that tab's PTY. The port IS the capability:
   * `http://127.0.0.1:{port}/input` writes to that tab only, `GET /output`
   * polls its OUTPUTS ring (512KB). No auth — localhost-only is the boundary.
   * Invokes `share_tab` (server/share.rs) which is idempotent: if already
   * shared returns existing `{port,url}` without rebinding. Discovery files
   * `~/.config/aterm/shares/{id}.json` and `/tmp/aterm-{id}.port` are written
   * by Rust for `curl $(cat /tmp/aterm-*.port)/health` convenience.
   */
  const handleShareTab = useCallback(async (id: string) => {
    try {
      const info = await invoke<SharedInfo>("share_tab", { id });
      setShares((prev) => new Map(prev).set(info.id, info));
      // Copy URL to clipboard immediately for convenience, then agent prompt
      // is available via Copy Agent Prompt in the context menu.
      await navigator.clipboard.writeText(info.url).catch(() => {});
    } catch (e) {
      console.error("share_tab failed:", e);
    }
  }, []);

  /**
   * Unshare a tab — aborts its dedicated Axum listener and removes discovery
   * files. Idempotent: if not shared, no-op. Updates local Map so badge
   * disappears and menu flips back to "Share terminal".
   */
  const handleUnshareTab = useCallback(async (id: string) => {
    try {
      await invoke("unshare_tab", { id });
    } catch (e) {
      console.warn("unshare_tab failed:", e);
    }
    setShares((prev) => {
      if (!prev.has(id)) return prev;
      const next = new Map(prev);
      next.delete(id);
      return next;
    });
  }, []);

  /**
   * Copy the plain Terminal URL for human `curl` usage.
   * Example: `http://127.0.0.1:42817` — then `curl -s $URL/output?since=0 | jq .data`
   */
  const handleCopyUrl = useCallback(async (url: string) => {
    try {
      await navigator.clipboard.writeText(url);
    } catch (e) {
      console.warn("clipboard write failed:", e);
    }
  }, []);

  /**
   * Copy the markdown Agent Prompt with live BASE_URL/port interpolated.
   * Built via `buildAgentPrompt({baseUrl, port, id})` so an LLM (Claude Code)
   * can immediately drive the PTY: `GET /output?since=0`, `POST /input {"data":"ls\n"}`,
   * poll `next_offset` until idle. Keeps frontend wording hot-reloadable (Vite)
   * without Rust rebuild.
   */
  const handleCopyPrompt = useCallback(async (id: string, port: number, url: string) => {
    try {
      const prompt = buildAgentPrompt({ baseUrl: url, port, id });
      await navigator.clipboard.writeText(prompt);
    } catch (e) {
      console.warn("clipboard write failed:", e);
    }
  }, []);

  /** Update a tab's title when the shell emits an OSC title (e.g., `nvim` sets "nvim: file"). */
  const handleTitleChange = useCallback((id: string, title: string) => {
    setTabs((prevTabs) =>
      prevTabs.map((t) => (t.id === id ? { ...t, title } : t))
    );
  }, []);

  /** Persist new config from SettingsDrawer, close the drawer, and invoke save_config. */
  const handleSaveSettings = useCallback((newConfig: ConfigType) => {
    setConfig(newConfig);
    setIsSettingsOpen(false);
    invoke("save_config", { config: newConfig }).catch(console.warn);
  }, []);

  /**
   * Update font size by delta (+1/-1) or reset, via Zod validation and persistence.
   * Clamps to 8..32 via Math.min/max before parseConfig, then saves so zoom persists
   * across restarts. Used by Ctrl+/- and Ctrl+Wheel handlers.
   */
  const updateFontSize = useCallback((updater: (prev: number) => number) => {
    setConfig((prev) => {
      const nextSize = updater(prev.fontSize);
      const parsed = parseConfig({ ...prev, fontSize: nextSize });
      invoke("save_config", { config: parsed }).catch(console.warn);
      return parsed;
    });
  }, []);

  // Window control actions — require `core:window:allow-*` permissions in capabilities/default.json.
  // Errors are logged with context so missing permissions are obvious in devtools (Tauri devtools).
  const handleMinimize = useCallback(() => {
    getAppWindow()?.minimize().catch((e) => console.error("minimize failed - missing permission?", e));
  }, [getAppWindow]);

  const handleMaximize = useCallback(() => {
    getAppWindow()?.toggleMaximize().catch((e) => console.error("toggleMaximize failed - missing permission?", e));
  }, [getAppWindow]);

  const handleCloseWindow = useCallback(() => {
    getAppWindow()?.close().catch((e) => console.error("close failed - missing permission?", e));
  }, [getAppWindow]);

  /**
   * Global keyboard shortcuts:
   * - Ctrl+Shift+T: new tab (inherits cwd)
   * - Ctrl+Shift+W: close active tab
   * - Ctrl+Plus/Equals or Cmd+Plus/Equals: increase font size (clamped 32)
   * - Ctrl+Minus or Cmd+Minus: decrease font size (clamped 8)
   * - Ctrl+0 or Cmd+0: reset font size to 12 (default)
   * All zoom shortcuts preventDefault so the WebView does not also page-zoom.
   * MetaKey included for macOS Cmd.
   */
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "t") {
        e.preventDefault();
        handleNewTab();
      } else if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "w") {
        e.preventDefault();
        if (activeId) handleCloseTab(activeId);
      } else if ((e.ctrlKey || e.metaKey) && !e.shiftKey && (e.key === "+" || e.key === "=")) {
        e.preventDefault();
        updateFontSize((prev) => Math.min(32, prev + 1));
      } else if ((e.ctrlKey || e.metaKey) && !e.shiftKey && e.key === "-") {
        e.preventDefault();
        updateFontSize((prev) => Math.max(8, prev - 1));
      } else if ((e.ctrlKey || e.metaKey) && e.key === "0") {
        e.preventDefault();
        setConfig((prev) => {
          const parsed = parseConfig({ ...prev, fontSize: 12 });
          invoke("save_config", { config: parsed }).catch(console.warn);
          return parsed;
        });
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [activeId, handleNewTab, handleCloseTab, updateFontSize]);

  /**
   * Ctrl+Wheel zoom — mirrors browser and VS Code behavior.
   * When Ctrl or Cmd is held and the wheel moves, adjust font size by one step
   * in the wheel direction (up = larger, down = smaller), clamped 8..32.
   * passive:false + preventDefault is required to block the WebView's native
   * page zoom which would otherwise scale the entire window rather than just
   * the terminal font.
   */
  useEffect(() => {
    const handleWheel = (e: WheelEvent) => {
      if (!e.ctrlKey && !e.metaKey) return;
      e.preventDefault();
      const delta = e.deltaY < 0 ? 1 : -1;
      updateFontSize((prev) => Math.min(32, Math.max(8, prev + delta)));
    };
    window.addEventListener("wheel", handleWheel, { passive: false });
    return () => window.removeEventListener("wheel", handleWheel);
  }, [updateFontSize]);

  /** Middle-click paste suppression — blocks PRIMARY paste after tab close. */
  useEffect(() => {
    const shouldSuppress = () =>
      Date.now() < ((window as unknown as Record<string, number>).__suppressMiddlePasteUntil ?? 0);

    const onMouse = (e: MouseEvent) => {
      if (e.button === 1 && shouldSuppress()) {
        e.preventDefault();
        e.stopPropagation();
        (e as unknown as { stopImmediatePropagation?: () => void }).stopImmediatePropagation?.();
      }
    };

    const onPaste = (e: ClipboardEvent) => {
      if (shouldSuppress()) {
        e.preventDefault();
        e.stopPropagation();
        (e as unknown as { stopImmediatePropagation?: () => void }).stopImmediatePropagation?.();
      }
    };

    document.addEventListener("mousedown", onMouse as EventListener, true);
    document.addEventListener("mouseup", onMouse as EventListener, true);
    document.addEventListener("auxclick", onMouse as EventListener, true);
    document.addEventListener("click", onMouse as EventListener, true);
    document.addEventListener("paste", onPaste as EventListener, true);

    return () => {
      document.removeEventListener("mousedown", onMouse as EventListener, true);
      document.removeEventListener("mouseup", onMouse as EventListener, true);
      document.removeEventListener("auxclick", onMouse as EventListener, true);
      document.removeEventListener("click", onMouse as EventListener, true);
      document.removeEventListener("paste", onPaste as EventListener, true);
    };
  }, []);

  const currentThemeColors = themes[config.theme] || themes["aterm-dark"];

  return (
    <div className="relative flex flex-col h-full w-full bg-[#141416] text-zinc-100 font-sans overflow-hidden">
      {/* Frameless resize affordance — 8 edge/corner handles (fixed, 6 px) that call
          Tauri's native `startResizeDragging` so `decorations:false` + Wayland still
          has a grab area. See WindowResizeHandles.tsx for direction/cursor docs. */}
      <WindowResizeHandles />
      <Tabbar
        tabs={tabs}
        activeId={activeId}
        shares={shares}
        onSelectTab={setActiveId}
        onCloseTab={handleCloseTab}
        onNewTab={handleNewTab}
        onContextMenu={(id, x, y) => setContextMenu({ id, x, y })}
        onToggleSettings={() => setIsSettingsOpen((prev) => !prev)}
        onMinimize={handleMinimize}
        onMaximize={handleMaximize}
        onCloseWindow={handleCloseWindow}
      />
      {/* Right-click context menu for tabs — offers Share / Copy URL / Copy Prompt / Unshare.
          Rendered globally in App so it can overlay the entire window and manage clipboard.
          Port/url derived from shares Map; null when no menu open or tab not shared. */}
      {contextMenu && (() => {
        const share = shares.get(contextMenu.id) || null;
        return (
          <TabContextMenu
            id={contextMenu.id}
            x={contextMenu.x}
            y={contextMenu.y}
            port={share?.port ?? null}
            url={share?.url ?? null}
            onShare={handleShareTab}
            onUnshare={handleUnshareTab}
            onCopyUrl={handleCopyUrl}
            onCopyPrompt={handleCopyPrompt}
            onClose={() => setContextMenu(null)}
          />
        );
      })()}

      <main id="terminal-container" style={{ backgroundColor: currentThemeColors.background }}>
        {tabs.map((tab) => (
          <TerminalView
            key={tab.id}
            id={tab.id}
            isActive={tab.id === activeId}
            config={config}
            themeColors={currentThemeColors}
            onTitleChange={handleTitleChange}
            onExit={handleCloseTab}
          />
        ))}
      </main>

      <SettingsDrawer
        isOpen={isSettingsOpen}
        config={config}
        onSave={handleSaveSettings}
        onClose={() => setIsSettingsOpen(false)}
      />
    </div>
  );
};

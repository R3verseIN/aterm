import React from "react";
import { Plus, Settings, Minus, Square, X } from "lucide-react";
import { Tab } from "../types/terminal";
import { TabItem } from "./TabItem";

/**
 * Props for the Tabbar component (frameless window header).
 * - tabs: list of open tabs to render via TabItem
 * - activeId: id of the currently selected tab (for active styling)
 * - shares: Map from tab id to {port,url} for tabs exposed via per-tab Share
 *   (right-click Share binds 127.0.0.1:0, port is the capability). Used to show
 *   a green dot badge in TabItem and to feed the context menu port/url.
 * - onSelectTab: activate a tab on click
 * - onCloseTab: close a tab (via X button or middle-click)
 * - onNewTab: create a new tab inheriting cwd from the active tab
 * - onContextMenu: open the right-click menu for a tab (id, clientX, clientY)
 *   — App owns the menu state so it can overlay the whole window and handle
 *   clipboard writes via `navigator.clipboard`.
 * - onToggleSettings: show/hide the SettingsDrawer
 * - onMinimize/onMaximize/onCloseWindow: Tauri window controls (decorations:false)
 */
interface TabbarProps {
  tabs: Tab[];
  activeId: string | null;
  shares: Map<string, { port: number; url: string }>;
  onSelectTab: (id: string) => void;
  onCloseTab: (id: string, e: React.MouseEvent) => void;
  onNewTab: () => void;
  onContextMenu: (id: string, x: number, y: number) => void;
  onToggleSettings: () => void;
  onMinimize: () => void;
  onMaximize: () => void;
  onCloseWindow: () => void;
}

/**
 * Tabbar — custom frameless window header for aterm.
 *
 * Layout (flex row, 38px height via CSS):
 * [ Left: tabs + new-tab button ] [ Center: draggable spacer ] [ Right: settings + window controls ]
 *
 * Tauri drag handling:
 * - The entire header (`header#tabbar`) and its left section and center spacer carry
 *   `data-tauri-drag-region`, which via CSS `[data-tauri-drag-region] { -webkit-app-region: drag }`
 *   makes the window draggable from any empty area (matching native titlebar behavior).
 * - All interactive elements (buttons, tabs) must be `no-drag` so clicks are not swallowed
 *   as drags. This is achieved via `.no-drag` class + global `button { -webkit-app-region: no-drag }`
 *   in styles.css. Explicit `no-drag` on each window control is critical for WebKitGTK
 *   where CSS inheritance of -webkit-app-region is not automatic.
 *
 * Permissions:
 * - Minimize/maximize/close require `core:window:allow-*` in capabilities/default.json.
 * - Drag requires `core:window:allow-start-dragging`.
 */
export const Tabbar: React.FC<TabbarProps> = ({
  tabs,
  activeId,
  shares,
  onSelectTab,
  onCloseTab,
  onNewTab,
  onContextMenu,
  onToggleSettings,
  onMinimize,
  onMaximize,
  onCloseWindow,
}) => {
  return (
    <header id="tabbar" className="tabbar-container" data-tauri-drag-region>
      {/* Left section: horizontally scrollable tabs + new-tab button.
          Both the container and the tabs area are drag regions, but each TabItem
          is `no-drag` so selecting/closing a tab does not drag the window. */}
      <div className="tabbar-left" data-tauri-drag-region>
        <div id="tabs" className="tabs-scroll-area" data-tauri-drag-region>
          {tabs.map((tab, idx) => {
            // Lookup per-tab share info for badge — O(1) via Map; null if not shared.
            const share = shares.get(tab.id) || null;
            return (
              <TabItem
                key={tab.id}
                tab={tab}
                index={idx}
                isActive={tab.id === activeId}
                isShared={share !== null}
                sharePort={share?.port ?? null}
                onSelect={onSelectTab}
                onClose={onCloseTab}
                onContextMenu={onContextMenu}
              />
            );
          })}
        </div>
        {/* New Tab button: creates a tab inheriting cwd from the active tab (App.handleNewTab).
            Must be no-drag so click registers. Tooltip documents keyboard shortcut. */}
        <button
          id="new-tab"
          className="no-drag window-action-btn"
          title="New Tab (Ctrl+Shift+T)"
          onClick={onNewTab}
          type="button"
        >
          <Plus className="w-4 h-4" />
        </button>
      </div>

      {/* Center spacer: flexes to fill remaining horizontal space and is the primary
          drag handle when tabs don't fill the bar. Without this, empty bar area would
          not be draggable. */}
      <div data-tauri-drag-region style={{ flex: "1 1 auto", height: "100%" }} />

      {/* Right cluster: settings + divider + window controls. Entire cluster is no-drag
          so none of its buttons initiate a drag. Each button also has explicit no-drag
          for robustness on platforms where parent no-drag is not inherited. */}
      <div className="window-action-cluster no-drag">
        <button
          id="settings-btn"
          className="window-action-btn no-drag"
          title="Settings"
          onClick={onToggleSettings}
          type="button"
        >
          <Settings className="w-4 h-4 text-zinc-300" />
        </button>
        {/* Thin vertical divider between app actions and window chrome */}
        <div className="no-drag" style={{ width: "1px", height: "16px", backgroundColor: "#2e3036", margin: "0 4px" }} />
        <button
          id="win-minimize"
          className="window-action-btn no-drag"
          title="Minimize"
          onClick={onMinimize}
          type="button"
        >
          <Minus className="w-3.5 h-3.5 text-zinc-300" />
        </button>
        <button
          id="win-maximize"
          className="window-action-btn no-drag"
          title="Maximize"
          onClick={onMaximize}
          type="button"
        >
          <Square className="w-3.5 h-3.5 text-zinc-300" />
        </button>
        <button
          id="win-close"
          className="window-action-btn close-btn no-drag"
          title="Close"
          onClick={onCloseWindow}
          type="button"
        >
          <X className="w-4 h-4 text-zinc-300" />
        </button>
      </div>
    </header>
  );
};

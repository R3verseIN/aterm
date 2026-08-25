import React from "react";
import { Radio, X } from "lucide-react";
import { Tab, ThemeColors } from "../types/terminal";

/**
 * Props for a single tab item in the tab bar.
 * - tab: the tab metadata (id + title) to render
 * - index: zero-based index used for badge display (1-based visible)
 * - isActive: whether this tab is the currently selected tab
 * - isShared: whether this tab is currently exposed via per-tab HTTP Share
 *   (right-click Share binds 127.0.0.1:0, port is the capability). When true
 *   a Radio (broadcast) icon is shown before the title instead of a generic dot
 *   and the context menu offers Copy URL / Copy Prompt / Unshare instead of Share.
 *   Radio was chosen over green dot because a color-only dot fails WCAG contrast
 *   and is ambiguous at 8 px; the Radio glyph is explicit "on-air / broadcasting"
 *   (like VS Code Live Share) and remains legible at 14 px with emerald-500.
 * - sharePort: the random high port if shared (e.g., 42817), or null. Shown
 *   in debug title tooltip as `:{port}` for quick `curl` reference.
 * - onSelect: callback invoked on left-click to activate the tab
 * - onClose: callback invoked on close-button click or middle-click
 * - onContextMenu: callback invoked on right-click (button 2) to open the
 *   tab's context menu at (clientX, clientY). App owns the menu state.
 */
interface TabItemProps {
  tab: Tab;
  index: number;
  isActive: boolean;
  isShared: boolean;
  sharePort: number | null;
  onSelect: (id: string) => void;
  onClose: (id: string, e: React.MouseEvent) => void;
  onContextMenu: (id: string, x: number, y: number) => void;
  themeColors?: ThemeColors;
}

/**
 * TabItem — renders a single tab pill with index badge, title and close button.
 *
 * Interaction model:
 * - Left click on the tab container selects the tab (onSelect).
 * - Left click on the X button closes the tab.
 * - Middle click (mouse button 1 / wheel-click) on the tab container also closes the tab.
 *   This matches native browser and VS Code behavior where wheel-click closes a tab.
 *   We handle both `onAuxClick` (spec for non-primary buttons) and `onMouseDown`
 *   as a fallback for WebKitGTK where auxclick may not fire on divs.
 *
 * The container has `no-drag` so Tauri's drag region (data-tauri-drag-region on
 * the header) does not swallow clicks. The active state applies `.active` styling
 * via CSS (blue top border, distinct background).
 */
export const TabItem: React.FC<TabItemProps> = ({
  tab,
  index,
  isActive,
  isShared,
  sharePort,
  onSelect,
  onClose,
  onContextMenu,
  themeColors,
}) => {
  const activeStyle = isActive && themeColors ? ({
    backgroundColor: themeColors.black,
    color: themeColors.foreground,
    borderColor: themeColors.brightBlack,
    borderTopColor: themeColors.blue,
  } as React.CSSProperties) : undefined;
  // Debounce guard — prevents rapid double middle-clicks from firing close twice
  // or from re-triggering close after the tab has already been removed.
  // Also used to suppress the Linux primary-selection paste that fires on
  // middle mouseup in X11/WebKitGTK.
  const lastMiddleClickRef = React.useRef<number>(0);
  const MIDDLE_CLICK_DEBOUNCE_MS = 350;
  const PASTE_SUPPRESS_MS = 900;

  /**
   * Mark that a middle-click close just happened so the global document-level
   * paste suppressor in App.tsx can block the subsequent middle-click paste
   * that Linux/X11 would otherwise dispatch to the newly focused terminal
   * (mouse-up lands on the terminal canvas after the tab bar reflows). The flag
   * is global because the tab and the terminal are siblings in the React tree.
   * The 900ms window covers the async close_session + React commit + xterm's
   * auxclick -> textarea move + WebKit's native `paste` delay.
   */
  const suppressNextMiddlePaste = () => {
    (window as unknown as Record<string, number>).__suppressMiddlePasteUntil =
      Date.now() + PASTE_SUPPRESS_MS;
  };

  /**
   * Handle middle-click (wheel button) to close the tab.
   * button === 1 is the middle/wheel button per MouseEvent spec.
   * We preventDefault to avoid auto-scroll, stopPropagation + stopImmediatePropagation
   * to keep the event from reaching xterm/WebKit handlers that would paste. Debounced
   * so holding or double-tapping the wheel doesn't fire twice. Even on debounced hits
   * we re-arm the suppress window so the paste from the first close stays blocked.
   */
  const handleMouseDown = (e: React.MouseEvent) => {
    if (e.button === 1) {
      const now = Date.now();
      if (now - lastMiddleClickRef.current < MIDDLE_CLICK_DEBOUNCE_MS) {
        e.preventDefault();
        e.stopPropagation();
        (e.nativeEvent as unknown as { stopImmediatePropagation?: () => void }).stopImmediatePropagation?.();
        // Re-arm suppress window so the original close's paste stays blocked
        suppressNextMiddlePaste();
        return;
      }
      lastMiddleClickRef.current = now;
      e.preventDefault();
      e.stopPropagation();
      (e.nativeEvent as unknown as { stopImmediatePropagation?: () => void }).stopImmediatePropagation?.();
      suppressNextMiddlePaste();
      onClose(tab.id, e);
    }
  };

  /**
   * onAuxClick is the standardized event for non-primary clicks (middle/right).
   * Some engines fire auxclick for middle-click on divs, so we handle it as well
   * for cross-platform robustness. The debounce here shares the same timestamp
   * so mousedown+auxclick for a single physical click only fires once, but we
   * still re-arm the suppress window on debounced hits to cover the async focus race.
   */
  const handleAuxClick = (e: React.MouseEvent) => {
    if (e.button === 1) {
      const now = Date.now();
      if (now - lastMiddleClickRef.current < MIDDLE_CLICK_DEBOUNCE_MS) {
        e.preventDefault();
        e.stopPropagation();
        (e.nativeEvent as unknown as { stopImmediatePropagation?: () => void }).stopImmediatePropagation?.();
        suppressNextMiddlePaste();
        return;
      }
      // If mousedown already handled, this will be within debounce and ignored;
      // otherwise treat as the primary middle-click entry point.
      lastMiddleClickRef.current = now;
      e.preventDefault();
      e.stopPropagation();
      (e.nativeEvent as unknown as { stopImmediatePropagation?: () => void }).stopImmediatePropagation?.();
      suppressNextMiddlePaste();
      onClose(tab.id, e);
    }
  };

  /**
   * Handle mouseup for middle button — needed because WebKitGTK synthesizes
   * the primary-selection paste on mouseup, not just mousedown/auxclick. If we
   * only block mousedown, the later mouseup (often after reflow on the terminal)
   * still triggers paste. So we block mouseup as well and keep the suppress window alive.
   */
  const handleMouseUp = (e: React.MouseEvent) => {
    if (e.button === 1) {
      const now = Date.now();
      // If this mouseup is part of a middle-click close sequence, block it
      if (now - lastMiddleClickRef.current < PASTE_SUPPRESS_MS) {
        e.preventDefault();
        e.stopPropagation();
        (e.nativeEvent as unknown as { stopImmediatePropagation?: () => void }).stopImmediatePropagation?.();
      }
    }
  };

  /**
   * Right-click handler — opens the per-tab context menu.
   * We use `onContextMenu` (button 2) rather than click so left/middle
   * close logic is unaffected. Prevent default to suppress the native
   * browser menu and forward (id, clientX, clientY) to App which owns
   * the global `TabContextMenu` overlay and the `shares` Map.
   */
  const handleContextMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    onContextMenu(tab.id, e.clientX, e.clientY);
  };

  return (
    <div
      className={`tabby-tab no-drag ${isActive ? "active" : ""}`}
      onClick={() => onSelect(tab.id)}
      onMouseDown={handleMouseDown}
      onMouseUp={handleMouseUp}
      onAuxClick={handleAuxClick}
      onContextMenu={handleContextMenu}
      title={isShared && sharePort ? `${tab.title} — shared :${sharePort}` : tab.title}
      style={activeStyle}
    >
      {/* Numeric badge showing 1-based tab index */}
      <span className="tab-index-badge">{index + 1}</span>
      {/* Shared indicator — Radio (broadcast) icon when this tab is exposed via
          127.0.0.1:{port}. Replaces the old w-2 h-2 emerald dot which was only
          8 px, color-only (WCAG fail), and ambiguous. Radio at 14 px with
          emerald-500 is legible even when multiple tabs are shared and keeps
          the port in the tooltip for `curl` convenience. */}
      {isShared && (
        <span
          title={`Shared on :${sharePort}`}
          aria-label={`Shared on :${sharePort}`}
          role="img"
          className="flex-shrink-0 inline-flex"
        >
          <Radio className="w-3.5 h-3.5 text-emerald-500" aria-hidden="true" />
        </span>
      )}
      {/* Tab title, falls back to generic label if empty */}
      <span className="tab-title-text">{tab.title || `Terminal ${index + 1}`}</span>
      {/* Close button: stops propagation so container onClick (select) does not fire */}
      <button
        className="tab-close-btn"
        title="Close Tab (Ctrl+Shift+W or middle-click)"
        onClick={(e) => onClose(tab.id, e)}
        type="button"
      >
        <X className="w-3.5 h-3.5" />
      </button>
    </div>
  );
};

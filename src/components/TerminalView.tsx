import React, { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { ConfigType } from "../schemas/configSchema";
import { ThemeColors } from "../types/terminal";

/**
 * Props for a single terminal view (one tab's xterm instance).
 * - id: PTY session UUID (from create_session) used for all IPC and event channels
 * - isActive: whether this tab is currently visible (controls display:none and focus)
 * - config: global config (fontSize/fontFamily) — changes trigger live xterm reconfiguration
 * - themeColors: resolved palette for the current theme — applied to xterm theme
 * - onTitleChange: callback when the shell emits an OSC title (e.g., "nvim: file.txt")
 * - onExit: callback when the PTY exits (pty:exit:{id}) — used to close the tab
 */
interface TerminalViewProps {
  id: string;
  isActive: boolean;
  config: ConfigType;
  themeColors: ThemeColors;
  onTitleChange: (id: string, title: string) => void;
  onExit: (id: string) => void;
}

/**
 * TerminalView — wraps a single xterm.js instance bound to a Rust PTY session.
 *
 * Lifecycle:
 * 1. On mount (id change): creates Terminal + FitAddon + WebLinksAddon, opens into
 *    wrapperRef, attaches onData (user input → write_to_session), onTitleChange,
 *    and Tauri event listeners (pty:data:{id} → term.write, pty:exit:{id} → onExit).
 *    A ResizeObserver watches the wrapper and forwards proposed cols/rows to
 *    resize_session so the PTY and xterm stay in sync.
 * 2. On config/theme change: mutates term.options (fontSize, fontFamily, theme) and
 *    re-fits. This avoids destroying the PTY buffer — only the renderer updates.
 * 3. On isActive change: re-fits and focuses so the hidden tab gets correct dimensions
 *    when re-shown (display:none tabs have 0 size until visible).
 * 4. On unmount: disposes listeners, observer, and terminal to free WebGL/canvas resources.
 *
 * Notes:
 * - FitAddon is the official @xterm/addon-fit helper that computes cols/rows from
 *   container size and font metrics. It must be loaded before term.open.
 * - `allowProposedApi: true` enables smoothScrollDuration and other proposed APIs used
 *   here. Safe because xterm 5.5.0 documents it as opt-in.
 */
export const TerminalView: React.FC<TerminalViewProps> = ({
  id,
  isActive,
  config,
  themeColors,
  onTitleChange,
  onExit,
}) => {
  // Wrapper div that xterm will attach its DOM (canvas/rows) into
  const wrapperRef = useRef<HTMLDivElement | null>(null);
  // Persistent xterm instance — mutated on config changes, not recreated
  const termRef = useRef<Terminal | null>(null);
  // Fit addon for cols/rows calculation
  const fitAddonRef = useRef<FitAddon | null>(null);

  // NOTE: middle-click paste suppression is now handled at document level
  // in App.tsx (global capture on mousedown/auxclick/mouseup/paste). The previous
  // per-wrapper capture was removed because: (a) wrapperRef is display:none when
  // inactive so hit-test misses, (b) mouseup after tab close lands on the tabbar
  // drag-spacer or document, not the wrapper, and (c) the real data path is a native
  // `paste` ClipboardEvent on xterm's textarea (not a mouse event) which we now block
  // in App.tsx. See App.tsx suppress logic for details.

  // Initialize xterm instance once per session id
  useEffect(() => {
    if (!wrapperRef.current) return;

    const fitAddon = new FitAddon();
    fitAddonRef.current = fitAddon;

    // Create the terminal with initial font/theme/scrollback from current config.
    // scrollback was previously omitted (defaulted to 1000), now respects config.scrollback (1500).
    // scrollOnUserInput ensures typing doesn't snap away from user scroll position unnecessarily.
    const term = new Terminal({
      fontSize: config.fontSize,
      fontFamily: config.fontFamily || "'JetBrains Mono', 'Fira Code', monospace",
      theme: themeColors,
      cursorBlink: true,
      allowProposedApi: true,
      smoothScrollDuration: 100,
      scrollback: config.scrollback ?? 1500,
      scrollOnUserInput: true,
    });
    termRef.current = term;

    // FitAddon must be loaded before open so proposeDimensions works
    term.loadAddon(fitAddon);
    term.loadAddon(new WebLinksAddon());

    // Attach the terminal to the DOM and fit to container size
    term.open(wrapperRef.current);
    fitAddon.fit();

    // Forward user keystrokes to the PTY (Rust handles actual shell input)
    const dataDisposable = term.onData((data: string) => {
      invoke("write_to_session", { id, data }).catch(console.error);
    });

    // Propagate shell's dynamic title (OSC 0/2) to the tab bar
    const titleDisposable = term.onTitleChange((newTitle: string) => {
      if (newTitle) {
        onTitleChange(id, newTitle);
      }
    });

    // Receive PTY output from Rust (pty:data:{id} events) and write to terminal
    let unlistenData: UnlistenFn | null = null;
    listen<string>(`pty:data:${id}`, (event) => {
      term.write(event.payload);
    }).then((fn) => {
      unlistenData = fn;
    }).catch(console.error);

    // Handle PTY exit (shell terminated) — notify App to remove the tab
    let unlistenExit: UnlistenFn | null = null;
    listen(`pty:exit:${id}`, () => {
      onExit(id);
    }).then((fn) => {
      unlistenExit = fn;
    }).catch(console.error);

    // Watch wrapper size (window resize, font zoom, drawer open, window maximize) and resize PTY.
    // Debounced via requestAnimationFrame to avoid fighting scrollbar thumb drag (fit() mutates
    // scrollTop). Also guards isActive: hidden wrappers have 0 size and would compute 0 cols.
    let rafId: number | null = null;
    const resizeObserver = new ResizeObserver(() => {
      if (rafId !== null) cancelAnimationFrame(rafId);
      rafId = requestAnimationFrame(() => {
        // Only fit when visible — hidden tabs have 0 dimensions and would send 0x0 to PTY
        const el = wrapperRef.current;
        if (!el || el.clientWidth === 0 || el.clientHeight === 0) return;
        try {
          fitAddon.fit();
          const dims = fitAddon.proposeDimensions();
          if (dims && Number.isFinite(dims.cols) && Number.isFinite(dims.rows) && dims.cols > 0 && dims.rows > 0) {
            const cols = Math.floor(dims.cols);
            const rows = Math.floor(dims.rows);
            invoke("resize_session", { id, cols, rows }).catch(console.error);
          }
        } catch {}
      });
    });

    resizeObserver.observe(wrapperRef.current);

    // Cleanup on session switch or unmount: remove listeners, observer, terminal
    return () => {
      if (rafId !== null) cancelAnimationFrame(rafId);
      dataDisposable.dispose();
      titleDisposable.dispose();
      if (unlistenData) unlistenData();
      if (unlistenExit) unlistenExit();
      resizeObserver.disconnect();
      term.dispose();
    };
  }, [id]);

  // Sync theme/font/scrollback live without recreating the terminal.
  // Mutating term.options triggers an internal renderer refresh. We also update
  // scrollback if it changed (xterm allows dynamic scrollback resize).
  useEffect(() => {
    if (!termRef.current || !fitAddonRef.current) return;
    termRef.current.options.fontSize = config.fontSize;
    termRef.current.options.theme = themeColors;
    termRef.current.options.scrollback = config.scrollback ?? 1500;
    if (config.fontFamily) {
      termRef.current.options.fontFamily = config.fontFamily;
    }
    try {
      fitAddonRef.current.fit();
    } catch {}
  }, [config, themeColors]);

  // When this tab becomes active (was hidden via display:none), re-fit and focus.
  // Hidden terminals have 0 dimensions until shown, so we recompute and notify Rust.
  useEffect(() => {
    if (isActive && termRef.current && fitAddonRef.current) {
      setTimeout(() => {
        try {
          fitAddonRef.current?.fit();
          const dims = fitAddonRef.current?.proposeDimensions();
          if (dims && Number.isFinite(dims.cols) && Number.isFinite(dims.rows) && dims.cols > 0 && dims.rows > 0) {
            const cols = Math.floor(dims.cols);
            const rows = Math.floor(dims.rows);
            invoke("resize_session", { id, cols, rows }).catch(console.error);
          }
        } catch {}
        termRef.current?.focus();
      }, 0);
    }
  }, [isActive, id]);

  return (
    <div
      ref={wrapperRef}
      className={`terminal-wrapper ${isActive ? "" : "hidden"}`}
      style={{ display: isActive ? "block" : "none" }}
    />
  );
};

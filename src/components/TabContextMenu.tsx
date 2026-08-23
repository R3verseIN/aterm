import React, { useEffect, useRef } from "react";

/**
 * TabContextMenu — right-click menu for a tab.
 *
 * Two separate clipboard actions as requested:
 * - Copy Terminal URL — plain `http://127.0.0.1:{port}` for human `curl`.
 * - Copy Agent Prompt — markdown block with live BASE_URL + port interpolated for LLM agents
 *   (Claude Code). Both are separate menu items so one clipboard write doesn't overwrite the other.
 *
 * The menu is rendered as a fixed-position div at (x,y) from the contextmenu event.
 * It auto-closes on click outside, Escape, or scroll.
 */
interface TabContextMenuProps {
  id: string;
  x: number;
  y: number;
  port: number | null;
  url: string | null;
  onShare: (id: string) => void;
  onUnshare: (id: string) => void;
  onCopyUrl: (url: string) => void;
  onCopyPrompt: (id: string, port: number, url: string) => void;
  onClose: () => void;
}

export const TabContextMenu: React.FC<TabContextMenuProps> = ({
  id,
  x,
  y,
  port,
  url,
  onShare,
  onUnshare,
  onCopyUrl,
  onCopyPrompt,
  onClose,
}) => {
  const ref = useRef<HTMLDivElement | null>(null);

  // Close on click outside, Escape, or scroll
  useEffect(() => {
    const onDocClick = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    const onScroll = () => onClose();
    document.addEventListener("mousedown", onDocClick);
    document.addEventListener("keydown", onKey);
    window.addEventListener("scroll", onScroll, true);
    return () => {
      document.removeEventListener("mousedown", onDocClick);
      document.removeEventListener("keydown", onKey);
      window.removeEventListener("scroll", onScroll, true);
    };
  }, [onClose]);

  // Keep menu inside viewport
  const style: React.CSSProperties = {
    position: "fixed",
    left: Math.min(x, window.innerWidth - 220),
    top: Math.min(y, window.innerHeight - 180),
    zIndex: 60,
  };

  const isShared = port !== null && url !== null;

  return (
    <div
      ref={ref}
      style={style}
      className="w-56 bg-[#18191c] border border-zinc-800 rounded-md shadow-2xl py-1 flex flex-col"
      role="menu"
    >
      {!isShared ? (
        <button
          className="text-left px-3 py-2 text-xs text-zinc-200 hover:bg-zinc-800 hover:text-white"
          onClick={() => {
            onShare(id);
            onClose();
          }}
        >
          Share terminal
        </button>
      ) : (
        <>
          <div className="px-3 py-1.5 text-[10px] text-zinc-500 font-mono truncate">
            :{port}
          </div>
          <button
            className="text-left px-3 py-2 text-xs text-zinc-200 hover:bg-zinc-800 hover:text-white"
            onClick={() => {
              if (url) onCopyUrl(url);
              onClose();
            }}
          >
            Copy Terminal URL
          </button>
          <button
            className="text-left px-3 py-2 text-xs text-zinc-200 hover:bg-zinc-800 hover:text-white"
            onClick={() => {
              if (port && url) onCopyPrompt(id, port, url);
              onClose();
            }}
          >
            Copy Agent Prompt
          </button>
          <div className="border-t border-zinc-800 my-1" />
          <button
            className="text-left px-3 py-2 text-xs text-red-300 hover:bg-zinc-800 hover:text-red-200"
            onClick={() => {
              onUnshare(id);
              onClose();
            }}
          >
            Unshare
          </button>
        </>
      )}
    </div>
  );
};

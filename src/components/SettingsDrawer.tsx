import React, { useEffect } from "react";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { ChevronDown, Settings, X } from "lucide-react";
import { ConfigSchema, ConfigType } from "../schemas/configSchema";
import { ThemeColors } from "../types/terminal";

/**
 * Props for the settings drawer.
 * - isOpen: controls visibility — when false the component returns null (unmounted)
 * - config: current global config used to populate defaultValues and to reset the form
 *           when the backend config loads asynchronously (parseConfig)
 * - onSave: callback with the validated new config — App persists via save_config
 * - onClose: hide the drawer without saving
 */
interface SettingsDrawerProps {
  isOpen: boolean;
  config: ConfigType;
  themeColors?: ThemeColors;
  onSave: (newConfig: ConfigType) => void;
  onClose: () => void;
}

/**
 * SettingsDrawer — slide-over panel for editing theme, font, shell, and font family.
 *
 * Implementation notes:
 * - Uses react-hook-form with zodResolver(ConfigSchema) for validation. The schema
 *   enforces fontSize 8..32 and scrollback 100..10000, but scrollback is not exposed
 *   in the UI (it persists with defaults so backend config is not accidentally cleared).
 * - defaultValues is set from `config` on init; a useEffect resets the form whenever
 *   `config` changes (e.g., after get_config loads or after a Ctrl+/- zoom updates fontSize).
 *   This keeps the drawer in sync if the user opens it after zooming.
 * - `isOpen` uses conditional `return null` rather than CSS visibility so the drawer
 *   does not trap focus or affect layout when hidden. Position is `fixed top-[38px]`
 *   to sit directly under the 38px tabbar (styles.css/.tabbar-container) and full viewport
 *   height minus header (`h-[calc(100vh-38px)]`). Fixed positioning escapes the parent
 *   `overflow-hidden` container; z-50 ensures it overlays terminals.
 * - Save uses `handleSubmit(onSave)` which validates before invoking the callback — the
 *   Save button is inside a `<form>` so Enter also submits.
 */
export const SettingsDrawer: React.FC<SettingsDrawerProps> = ({
  isOpen,
  config,
  themeColors,
  onSave,
  onClose,
}) => {
  const panelBg = themeColors?.background ?? "#18191c";
  const panelBorder = themeColors?.brightBlack ?? "#27272a";
  const panelFg = themeColors?.foreground ?? "#e4e4e7";
  const isLight = ["aterm-light", "gruvbox-light", "catppuccin-latte", "solarized-light"].includes(config.theme);
  const inputBg = isLight ? "#ffffff" : "#18181b";
  const inputBorder = isLight ? "#d4d4d8" : "#3f3f46";
  const inputText = isLight ? "#18181b" : "#e4e4e7";
  const labelColor = isLight ? "#52525b" : "#a1a1aa";
  const headerBorder = isLight ? "#e4e4e7" : panelBorder;
  const {
    register,
    handleSubmit,
    reset,
    formState: { errors },
  } = useForm<ConfigType>({
    resolver: zodResolver(ConfigSchema),
    defaultValues: config,
  });

  // Keep form values in sync with external config changes (backend load, zoom).
  useEffect(() => {
    reset(config);
  }, [config, reset]);

  /**
   * Smooth drawer mount — instead of `if (!isOpen) return null` which pops
   * in one frame, we keep the DOM mounted and drive `transform` + `opacity`
   * via CSS. The scrim fades (`opacity 0→1`, 200 ms ease-out) and the panel
   * slides (`translateX(100%)→0`, 240 ms spring-like) — both GPU-composited.
   * `pointer-events:none` when closed prevents invisible click capture. Reduced
   * motion is handled globally in styles.css (`prefers-reduced-motion`).
   */
  const scrimClass = isOpen ? "opacity-100 pointer-events-auto" : "opacity-0 pointer-events-none";
  const panelClass = isOpen ? "translate-x-0" : "translate-x-full";

  return (
    <>
      {/* Scrim — dims terminal behind drawer and closes on click. Using fixed
          inset-0 with bg-black/30 keeps focus on the drawer without trapping
          scroll; click-through is prevented by z-index ordering (scrim z-40,
          drawer z-50). Fades via opacity for smooth open/close. */}
      <div
        className={`fixed inset-0 top-9.5 bg-black/30 z-40 transition-opacity duration-200 ease-out drawer-scrim ${scrimClass}`}
        onClick={onClose}
        aria-hidden="true"
      />
      <aside
        className={`fixed top-9.5 right-0 w-80 max-w-[90vw] h-[calc(100dvh-38px)] border-l p-5 flex flex-col gap-4 z-50 shadow-2xl transition-transform duration-240 ease-out drawer-panel will-change-transform ${panelClass}`}
        style={{
          backgroundColor: panelBg,
          borderColor: panelBorder,
          color: panelFg,
          transitionTimingFunction: "var(--ease-spring, cubic-bezier(0.32,0.72,0,1))",
        }}
        aria-hidden={!isOpen}
      >
      {/* Header with title and close button */}
      <div className="flex items-center justify-between pb-3 border-b" style={{ borderColor: headerBorder }}>
        <h3 className="text-sm font-semibold flex items-center gap-2" style={{ color: panelFg }}>
          <Settings className="w-4 h-4" style={{ color: themeColors?.blue ?? "#60a5fa" }} /> Settings
        </h3>
        <button
          className="p-1 rounded hover:bg-zinc-800"
          style={{ color: labelColor }}
          onClick={onClose}
          type="button"
        >
          <X className="w-4 h-4" />
        </button>
      </div>

      {/* Form: react-hook-form handles validation and submission */}
      <form onSubmit={handleSubmit(onSave)} className="flex flex-col gap-4 flex-1 min-h-0">
        <div className="flex flex-col gap-3 flex-1 overflow-y-auto min-h-0 pr-1">
          {/* Theme selector — enum validated by Zod. Uses appearance-none + custom
              ChevronDown so the dark theme dropdown has a visible, consistent chevron
              (native WebKit arrow is near-invisible on zinc-900). pr-8 + truncate
              prevents "Aterm Dark (Tabby Style)" from clipping at w-80. */}
          <label className="flex flex-col gap-1.5 text-xs font-medium" style={{ color: labelColor }}>
            Color Theme
            <div className="relative">
              <select
                id="settings-theme"
                {...register("theme")}
                className="w-full appearance-none border text-xs rounded-md p-2 pr-8 outline-none focus:border-blue-500 focus:ring-1 focus:ring-blue-500 truncate"
                style={{ backgroundColor: inputBg, borderColor: inputBorder, color: inputText }}
              >
                <option value="aterm-dark">Aterm Dark (Tabby Style)</option>
                <option value="aterm-light">Aterm Light</option>
                <option value="nord">Nord Theme</option>
                <option value="pitchblack">Pitch Black (#000000)</option>
                <option value="dracula">Dracula</option>
                <option value="gruvbox-dark">Gruvbox Dark</option>
                <option value="gruvbox-light">Gruvbox Light</option>
                <option value="tokyo-night">Tokyo Night</option>
                <option value="catppuccin-mocha">Catppuccin Mocha</option>
                <option value="catppuccin-latte">Catppuccin Latte</option>
                <option value="solarized-dark">Solarized Dark</option>
                <option value="solarized-light">Solarized Light</option>
                <option value="monokai">Monokai</option>
                <option value="one-dark">One Dark</option>
              </select>
              <ChevronDown className="pointer-events-none absolute right-2 top-1/2 -translate-y-1/2 w-4 h-4" style={{ color: labelColor }} />
            </div>
          </label>

          {/* Font size — numeric input with valueAsNumber so Zod receives a number, not string.
              Validation 8..32 comes from ConfigSchema; error message shown inline.
              min/max/step/inputMode clamp the native spinner and mobile keyboard;
              [appearance:textfield] hides the unstyled WebKit inner-spin-button that
              otherwise overlaps the input as a tiny "icon" on the right (seen in screenshot). */}
          <label className="flex flex-col gap-1.5 text-xs font-medium" style={{ color: labelColor }} htmlFor="settings-fontSize">
            Font Size (px)
            <input
              id="settings-fontSize"
              type="number"
              min={8}
              max={32}
              step={1}
              inputMode="numeric"
              aria-invalid={!!errors.fontSize}
              aria-describedby={errors.fontSize ? "settings-fontSize-error" : undefined}
              {...register("fontSize", { valueAsNumber: true })}
              className="border text-xs rounded-md p-2 outline-none focus:border-blue-500 focus:ring-1 focus:ring-blue-500 [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none placeholder:text-zinc-500 aria-invalid:border-red-500"
              style={{ backgroundColor: inputBg, borderColor: inputBorder, color: inputText }}
            />
            {errors.fontSize && (
              <span id="settings-fontSize-error" className="text-red-400 text-[11px]">{errors.fontSize.message}</span>
            )}
          </label>

          {/* Shell path — empty means use $SHELL fallback chain on the backend
              (pty::create_session checks $SHELL → /bin/bash → /bin/sh). */}
          <label className="flex flex-col gap-1.5 text-xs font-medium" style={{ color: labelColor }} htmlFor="settings-shell">
            Shell Command Path
            <input
              id="settings-shell"
              type="text"
              placeholder="/bin/bash"
              spellCheck={false}
              autoComplete="off"
              autoCorrect="off"
              title="Empty uses $SHELL → /bin/bash → /bin/sh fallback on the backend"
              {...register("shell")}
              className="border text-xs rounded-md p-2 outline-none focus:border-blue-500 focus:ring-1 focus:ring-blue-500 font-mono placeholder:text-zinc-500 truncate"
              style={{ backgroundColor: inputBg, borderColor: inputBorder, color: inputText }}
            />
            <span className="text-[11px]" style={{ color: labelColor }}>Empty uses $SHELL fallback</span>
          </label>

          {/* Font family — freeform CSS font-family string, live-applied via
              TerminalView's term.options.fontFamily mutation. The value is often
              long ('JetBrains Mono','Fira Code','Cascadia Code', monospace) so we
              use font-mono + truncate + title for hover, and allow horizontal scroll
              without wrapping. Previously the input clipped without ellipsis. */}
          <label className="flex flex-col gap-1.5 text-xs font-medium" style={{ color: labelColor }} htmlFor="settings-fontFamily">
            Font Family
            <input
              id="settings-fontFamily"
              type="text"
              spellCheck={false}
              autoComplete="off"
              autoCorrect="off"
              placeholder="JetBrains Mono, monospace"
              {...register("fontFamily")}
              className="border text-xs rounded-md p-2 outline-none focus:border-blue-500 focus:ring-1 focus:ring-blue-500 font-mono placeholder:text-zinc-500 truncate"
              style={{ backgroundColor: inputBg, borderColor: inputBorder, color: inputText }}
              title={config.fontFamily}
            />
          </label>
        </div>

        {/* Save action — sticky bottom bar so it stays visible even when the form
            scrolls. Previously flex-1 + border-t scrolled away; now sticky with
            mt-auto + bg matching the drawer prevents overlap. Includes Cancel
            (ghost) + Save (primary) for explicit discard vs commit. */}
        <div className="sticky bottom-0 mt-auto pt-3 border-t flex gap-2" style={{ backgroundColor: panelBg, borderColor: panelBorder }}>
          <button
            type="button"
            onClick={onClose}
            className="flex-1 bg-zinc-800 hover:bg-zinc-700 text-zinc-200 text-xs font-semibold py-2 px-3 rounded-md transition-colors"
          >
            Cancel
          </button>
          <button
            type="submit"
            className="flex-1 bg-blue-600 hover:bg-blue-500 text-white text-xs font-semibold py-2 px-3 rounded-md transition-colors"
          >
            Save Changes
          </button>
        </div>
      </form>
    </aside>
    </>
  );
};

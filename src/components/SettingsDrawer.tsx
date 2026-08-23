import React, { useEffect } from "react";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { Settings, X } from "lucide-react";
import { ConfigSchema, ConfigType } from "../schemas/configSchema";

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
  onSave,
  onClose,
}) => {
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

  // Unmount when closed — avoids rendering hidden form and prevents tab-order issues.
  if (!isOpen) return null;

  return (
    <aside className="fixed top-[38px] right-0 w-80 h-[calc(100vh-38px)] bg-[#18191c] border-l border-zinc-800 p-5 flex flex-col gap-4 z-50 shadow-2xl">
      {/* Header with title and close button */}
      <div className="flex items-center justify-between pb-3 border-b border-zinc-800">
        <h3 className="text-sm font-semibold text-zinc-100 flex items-center gap-2">
          <Settings className="w-4 h-4 text-blue-400" /> Settings
        </h3>
        <button
          className="text-zinc-400 hover:text-zinc-100 p-1 rounded hover:bg-zinc-800"
          onClick={onClose}
          type="button"
        >
          <X className="w-4 h-4" />
        </button>
      </div>

      {/* Form: react-hook-form handles validation and submission */}
      <form onSubmit={handleSubmit(onSave)} className="flex flex-col gap-4 flex-1">
        <div className="flex flex-col gap-3 flex-1 overflow-y-auto">
          {/* Theme selector — enum validated by Zod */}
          <label className="flex flex-col gap-1.5 text-xs text-zinc-400 font-medium">
            Color Theme
            <select
              {...register("theme")}
              className="bg-zinc-900 border border-zinc-700 text-zinc-200 text-xs rounded-md p-2 outline-none focus:border-blue-500"
            >
              <option value="aterm-dark">Aterm Dark (Tabby Style)</option>
              <option value="aterm-light">Aterm Light</option>
              <option value="nord">Nord Theme</option>
            </select>
          </label>

          {/* Font size — numeric input with valueAsNumber so Zod receives a number, not string.
              Validation 8..32 comes from ConfigSchema; error message shown inline. */}
          <label className="flex flex-col gap-1.5 text-xs text-zinc-400 font-medium">
            Font Size (px)
            <input
              type="number"
              {...register("fontSize", { valueAsNumber: true })}
              className="bg-zinc-900 border border-zinc-700 text-zinc-200 text-xs rounded-md p-2 outline-none focus:border-blue-500"
            />
            {errors.fontSize && (
              <span className="text-red-400 text-[11px]">{errors.fontSize.message}</span>
            )}
          </label>

          {/* Shell path — empty means use $SHELL fallback chain on the backend
              (pty::create_session checks $SHELL → /bin/bash → /bin/sh). */}
          <label className="flex flex-col gap-1.5 text-xs text-zinc-400 font-medium">
            Shell Command Path
            <input
              type="text"
              placeholder="/bin/bash"
              {...register("shell")}
              className="bg-zinc-900 border border-zinc-700 text-zinc-200 text-xs rounded-md p-2 outline-none focus:border-blue-500"
            />
          </label>

          {/* Font family — freeform CSS font-family string, live-applied via
              TerminalView's term.options.fontFamily mutation. */}
          <label className="flex flex-col gap-1.5 text-xs text-zinc-400 font-medium">
            Font Family
            <input
              type="text"
              {...register("fontFamily")}
              className="bg-zinc-900 border border-zinc-700 text-zinc-200 text-xs rounded-md p-2 outline-none focus:border-blue-500"
            />
          </label>
        </div>

        {/* Save action — validates then calls onSave which persists via invoke("save_config") */}
        <div className="pt-3 border-t border-zinc-800 flex gap-2">
          <button
            type="submit"
            className="flex-1 bg-blue-600 hover:bg-blue-500 text-white text-xs font-semibold py-2 px-3 rounded-md transition-colors"
          >
            Save Changes
          </button>
        </div>
      </form>
    </aside>
  );
};

import { z } from "zod";

/**
 * ConfigSchema — Zod validation schema for the user configuration.
 *
 * This schema is the single source of truth for frontend validation and
 * provides defaults that mirror the Rust backend (src-tauri/src/config.rs).
 * - theme: enum of supported themes, defaults to "aterm-dark" (matches Rust default_theme).
 * - fontSize: integer pixel size clamped 8..32, default 12 (matches Rust default_font_size).
 *   The 8-32 range ensures xterm remains readable and FitAddon can compute cols/rows safely
 *   (charW = fontSize*0.6, charH = fontSize*1.2+4 in App.handleNewTab).
 * - shell: string path to the shell binary (e.g., "/bin/bash", "/usr/bin/zsh").
 *   Empty string means "use $SHELL or fallback chain" on the backend (pty::create_session).
 * - fontFamily: CSS font-family string, defaults to "JetBrains Mono, monospace" (must match
 *   Rust default_font_family for persistence round-trip).
 * - scrollback: number of lines kept in xterm scrollback, 100..10000, default 1500.
 *   Not yet exposed in SettingsDrawer UI but persisted so backend scrollback config is not lost
 *   when the user saves (invoke("save_config") sends the full object).
 *
 * Note: Uses z.number() (not z.coerce.number()) because SettingsDrawer registers
 * fontSize with valueAsNumber:true, so the form already coerces strings to numbers.
 * Keep min/max aligned with Rust Config (font_size: u8, scrollback: u32) to avoid
 * serde deserialization failures (float or out-of-range would reject on Rust side).
 */
export const ConfigSchema = z
  .object({
    theme: z.enum(["aterm-dark", "aterm-light", "nord"]).default("aterm-dark"),
    fontSize: z.number().min(8).max(32).default(12),
    shell: z.string().default(""),
    fontFamily: z.string().default("JetBrains Mono, monospace"),
    scrollback: z.number().min(100).max(10000).default(1500),
  })
  .passthrough();

/**
 * ConfigType — inferred TypeScript type from the Zod schema.
 * Used as the React state type in App (config) and as props in TerminalView/SettingsDrawer.
 * Ensures compile-time safety when passing config to invoke("save_config") which expects
 * camelCase fields (serde rename_all="camelCase" on Rust side).
 */
export type ConfigType = z.infer<typeof ConfigSchema>;

/**
 * parseConfig — safe parsing with fallback to defaults.
 *
 * The backend may return:
 * - A full Config object (camelCase) from get_config (serde serialized)
 * - An old snake_case file (font_family/font_size) that parseConfig must still accept
 *   (Rust handles alias, but frontend may receive either during migration)
 * - Empty/invalid data on first launch or corrupted file
 *
 * Strategy: try safeParse on rawConfig (or {} if null/undefined). If success, return data
 * (Zod fills missing fields with defaults). If failure (e.g., theme is "foo"), fall back
 * to ConfigSchema.parse({}) which returns all defaults. This prevents the UI from crashing
 * on corrupted config and ensures handleNewTab and TerminalView always have valid fontSize.
 */
export function parseConfig(rawConfig: unknown): ConfigType {
  const result = ConfigSchema.safeParse(rawConfig || {});
  if (result.success) {
    return result.data;
  }
  return ConfigSchema.parse({});
}

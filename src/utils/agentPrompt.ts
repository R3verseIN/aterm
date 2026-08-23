/**
 * agentPrompt.ts — Builds the agent-facing prompt copied on Share.
 *
 * Two separate clipboard items are offered on right-click Share:
 * 1. `Copy Terminal URL` — plain `http://127.0.0.1:{port}` for human curl.
 * 2. `Copy Agent Prompt` — markdown block with live BASE_URL + port interpolated
 *    so an LLM (Claude Code, etc.) can immediately drive the shared PTY via HTTP.
 *
 * Keep the template here (frontend constant) rather than in Rust so it hot-reloads
 * via Vite and doesn't require a Rust rebuild to tweak wording. The port is live
 * from `share_tab` which binds 127.0.0.1:0 and returns the OS-assigned high free port.
 */

/**
 * Build a markdown agent prompt for a shared tab.
 * - baseUrl: live URL like `http://127.0.0.1:42817` (port is the id)
 * - opts.id is optional — included if you want the agent to know the tab UUID.
 */
export function buildAgentPrompt(opts: {
  baseUrl: string;
  port: number;
  id?: string;
}): string {
  const { baseUrl, port, id } = opts;
  const idLine = id ? `SESSION_ID: ${id}\n` : "";
  return [
    `You have programmatic control of the user's visible terminal in aterm (Tauri+PTY) via localhost HTTP.`,
    ``,
    `BASE_URL: ${baseUrl}`,
    idLine + `PORT: ${port}`,
    ``,
    `RULES`,
    `- This is the SAME PTY the user sees in the Tauri window — don't spawn a new shell, reuse this tab.`,
    `- Poll output with the byte-offset cursor \`since\`; \`next_offset\` is the next cursor. The server keeps the last 512KB.`,
    `- No auth, localhost only (127.0.0.1). CORS is permissive.`,
    ``,
    `API (port is the tab id — no :id in path; /screenshot is image/png, others JSON)`,
    `- GET ${baseUrl}/health -> {"version","id","alive","pid"}`,
    `- GET ${baseUrl}/output?since=0&limit=32768 -> {"data","next_offset","total","truncated"}`,
    `  Poll every 300-800ms: since = next_offset; if truncated re-fetch. Limit capped 256KB.`,
    `- POST ${baseUrl}/input  body {"data":"ls -la\\n"}  (\\n or \\r = Enter, \\x03 = Ctrl-C)`,
    `- GET ${baseUrl}/cwd -> {"cwd":"/path","id"}`,
    `- POST ${baseUrl}/resize {"cols":80,"rows":24}`,
    `- GET ${baseUrl}/screenshot -> image/png (binary) or ?format=base64 -> {"image":"data:image/png;base64,...","id"}`,
    `  Cross-compatible single way: frontend html2canvas of that tab's xterm DOM (cloned offscreen if hidden) -> cached PNG per tab, even when not focused (port is tab id). Use for vision/docs.`,
    ``,
    `WORKFLOW`,
    `1. GET ${baseUrl}/output?since=0 to see current screen (text).`,
    `2. POST ${baseUrl}/input to run a command.`,
    `3. Poll ${baseUrl}/output?since=<prev_next_offset> until idle.`,
    `4. GET ${baseUrl}/screenshot for visual verification (even if tab not focused — port is that tab's screenshot).`,
    `5. Repeat. Don't DELETE unless the user says so.`,
    ``,
    `EXAMPLE`,
    `curl -s ${baseUrl}/output?since=0 | jq -r .data`,
    `curl -s -X POST -H "Content-Type: application/json" -d '{"data":"echo hi\\n"}' ${baseUrl}/input`,
    `curl -s "${baseUrl}/output?since=0&limit=1024" | jq .`,
    `curl -s ${baseUrl}/screenshot -o /tmp/term.png && file /tmp/term.png  # per-tab image even when not focused`,
    `# base64 for LLM vision: curl -s "${baseUrl}/screenshot?format=base64" | jq -r .image`,
    ``,
    `SAFETY: localhost only. Keep the URL private — anyone with it can inject keystrokes.`,
  ].join("\n");
}



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
    `RULES — SIMPLE ONE-WAY`,
    `- This is the SAME PTY the user sees — don't spawn new shell, reuse this tab.`,
    `- One-way exec: POST /input writes and holds till output (version bumps). No separate poll needed.`,
    `- Dump-all history: GET /history returns whole 512KB ring as raw PTY bytes. Use jq -r .data | sed strip.`,
    `- Raw ANSI: strip before parsing. total is byte length, version is monotonic per append/clear.`,
    `- Ring caps at 512KB — drain oldest. For big cat/cargo, POST /clear first then cmd | tail -n 300.`,
    `- Clear sync: shell clear/Ctrl-L auto-wipes both xterm and GET /history ring. POST /clear does same.`,
    `- No auth, localhost only (127.0.0.1). CORS Any, port IS secret. All GET are no-store.`,
    `- Screenshot: GET /screenshot holds 10s for fresh capture (no cache). GET /screenshot/current returns last cached immediately. 503 = timeout.`,
    ``,
    `API (port is tab id — no :id in path)`,
    `- GET ${baseUrl}/health -> {"version","id","alive":bool,"pid":number|null}`,
    `- POST ${baseUrl}/input body {"data":"ls -la\\n","wait":2} -> {"data","total","version","id"}  one-way, holds till version changes (wait 0..10, default 2). Auto \\n→\\r and ghost fix.`,
    `- GET ${baseUrl}/history -> {"data","total","version","id"}  current dump-all (max 512KB), no hold. Alias /output for compat.`,
    `- GET ${baseUrl}/screenshot -> image/png (binary) or ?format=base64 -> {"image":"data:image/png;base64,...","id"}  holds 10s fresh.`,
    `- GET ${baseUrl}/screenshot/current -> same but immediate cached, 404 if none yet.`,
    `- POST ${baseUrl}/clear -> {"ok":true,"cleared":true,"id"}  wipes ring + screenshot.`,
    `- GET ${baseUrl}/cwd -> {"cwd":"/path","id"}  Linux only via /proc/<pid>/cwd.`,
    `- POST ${baseUrl}/resize {"cols":120,"rows":30}  SIGWINCH, use if screenshot wraps.`,
    ``,
    `WORKFLOW — ONE-WAY SIMPLE`,
    `0. GET ${baseUrl}/health | jq . abort if alive==false`,
    `1. POST ${baseUrl}/clear if history large (GET /history | jq .total)`,
    `2. POST ${baseUrl}/input -d '{"data":"ls -la\\n","wait":2}' | jq -r .data | sed strip | tail -n 200  — one call, holds till done, returns history`,
    `3. GET ${baseUrl}/history | jq -r .data | tail -n 100  to browse without executing`,
    `4. GET ${baseUrl}/screenshot --max-time 15 for visual, or /screenshot/current for last cached`,
    `5. After cd: POST /input '{"data":"cd /tmp\\n","wait":1}' + GET /cwd | jq -r .cwd`,
    `6. On hang: POST /input '{"data":"\\u0003","wait":1}' (Ctrl-C)`,
    ``,
    `EXAMPLE — ONE-WAY COPY-PASTE`,
    `BASE="${baseUrl}"`,
    `curl -sSf "$BASE/health" | jq .`,
    `curl -sSf -X POST "$BASE/clear" | jq .`,
    `# one-way exec — holds till output, no poll loop`,
    `curl -sSf -X POST -H "Content-Type: application/json" -d "$(jq -n --arg data "ls -la\\n" '{data:$data,wait:2}')" "$BASE/input" | jq -r .data | sed -E 's/\\x1B\\[[0-9;]*[a-zA-Z]//g; s/\\x1B\\]0;.*\\x07//g' | tail -n 100`,
    `# browse current history without exec`,
    `curl -sSf "$BASE/history" | jq -r .data | tail -n 100`,
    `# screenshot hold (fresh) vs current (cached)`,
    `curl -sSf --max-time 15 "$BASE/screenshot" -o /tmp/term.png && file /tmp/term.png || curl -s "$BASE/screenshot" | jq .logs`,
    `curl -sSf "$BASE/screenshot/current?format=base64" | jq -r '.image // .error' | cut -d, -f2 | base64 -d > /tmp/term.png && file /tmp/term.png`,
    `# Ctrl-C if hung`,
    `curl -sSf -X POST -H "Content-Type: application/json" -d "$(jq -n --arg data $'\\x03' '{data:$data,wait:1}')" "$BASE/input" | jq .`,
    ``,
    `SAFETY: localhost only, no auth, CORS Any. Port IS secret. Don't log URL. POST /clear after secrets. Discovery /tmp/aterm-*.port leak port — chmod 700.`,
  ].join("\n");
}

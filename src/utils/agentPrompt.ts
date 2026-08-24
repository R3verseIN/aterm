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
    `RULES — STABILITY FIRST`,
    `- This is the SAME PTY the user sees in the Tauri window — don't spawn a new shell, reuse this tab. Output you read is exactly what the user sees.`,
    `- Dump-all: GET /output returns the whole 512KB ring as raw PTY bytes (ANSI colors, \\r, ESC[2J, OSC titles). No cursor, no since/next_offset. Poll and diff locally.`,
    `- Raw ANSI: strip before parsing. Use jq -r .data | sed -E 's/\\x1B\\[[0-9;]*[a-zA-Z]//g; s/\\x1B\\]0;.*\\x07//g' or col -b. total is byte length of raw.`,
    `- Ring caps at 512KB — drain oldest on overflow. Large cat/cargo build over 512KB silently loses head. For big commands run POST /clear first, then cmd 2>&1 | tail -n 300. If total==524288 you hit cap.`,
    `- Clear sync: shell clear/Ctrl-L (ESC[2J/ESC[3J/\\x0c/ESC c) auto-wipes both xterm scrollback and GET /output ring. POST /clear does same via emit clear-terminal:{id}. Don't expect pre-clear history.`,
    `- No auth, localhost only (127.0.0.1). CORS Any, port IS the secret. All GET are no-store (Cache-Control: no-store) — add -H "Cache-Control: no-cache" if you cache.`,
    `- Screenshot is fresh-only: every GET emits request-screenshot:{id} and holds up to 10s via wait_for_screenshot(10). No stale cache. 503 means timeout — read logs.frontend_last_error. Pinned to bottom after write — top kali no longer hidden via lineHeight 1.0 + scrollToBottom + fonts.ready.`,
    `- Discovery if you lost URL: cat /tmp/aterm-{id}.port or ~/.config/aterm/shares/{id}.json. Port is tab id — no :id in path.`,
    ``,
    `API (port is the tab id — no :id in path; /screenshot is image/png, others JSON)`,
    `- GET ${baseUrl}/health -> {"version","id","alive":bool,"pid":number|null}  alive:false means tab closed (pty:exit). Check first.`,
    `- GET ${baseUrl}/output -> {"data":string,"total":number,"id":string}  whole buffer (max 512KB). No query params; ?since&limit are ignored (dump-all). Use jq -r .data.`,
    `- POST ${baseUrl}/input  body {"data":"ls -la\\n"}  (\\n or \\r = Enter, \\x03 = Ctrl-C, \\x04 = Ctrl-D). Must send valid JSON — escape via jq -n --arg. Server auto \\n→\\r and auto-appends \\r if you forget terminator (ghost fix for "pwd; ls" without \\n), but always send \\n.`,
    `- POST ${baseUrl}/clear -> {"ok":true,"cleared":true,"id"}  wipes OUTPUTS ring + screenshot waiters. Use before noisy command.`,
    `- GET ${baseUrl}/cwd -> {"cwd":"/path","id"}  Linux only via /proc/<pid>/cwd, live. Use after cd, not parsing pwd from data.`,
    `- POST ${baseUrl}/resize {"cols":120,"rows":30}  sends SIGWINCH. Use if screenshot shows wrapped tables or tput cols mismatch.`,
    `- GET ${baseUrl}/screenshot -> image/png (binary)  or  ?format=base64 -> {"image":"data:image/png;base64,...","id","width":0,"height":0}`,
    `  Holds 10s for html2canvas (onclone fixes opacity:0 hidden tabs). On 503: {"error", "logs":{session_exists,share_exists,frontend_last_error,hint,elapsed_ms}, "issues":[]}.`,
    ``,
    `ERRORS`,
    `- 404 {"error":"session not found"} on any endpoint — tab closed, stop polling.`,
    `- 503 on /screenshot — no fresh capture in 10s. Check logs.frontend_last_error (wrapper size 0, html2canvas failed, IPC blocked) and retry once.`,
    `- All errors are JSON, not PNG. Check HTTP code with curl -w %{http_code} or curl -f.`,
    ``,
    `WORKFLOW — ROBUST (stable 3)`,
    `0. GET ${baseUrl}/health | jq .  abort if alive==false.`,
    `1. POST ${baseUrl}/clear if total> few KB or you need clean diff (GET /output | jq .total).`,
    `2. GET ${baseUrl}/output | jq -r .data | sed -E strip ANSI | tail -n 200  to see current screen.`,
    `3. POST ${baseUrl}/input  with jq-escaped JSON, check {"ok":true}. Sleep 0.7s. Handles ghost: server auto \\n→\\r and missing terminator →\\r (pwd; ls without \\n still executes).`,
    `4. Poll GET ${baseUrl}/output until idle: stable 3x identical stripped data (sleep 0.6, max 8 loops). Large output: compare total growth instead.`,
    `   prev=""; stable=0; for i in 1 2 3 4 5 6 7 8; do cur=$(curl -sSf ${baseUrl}/output | jq -r .data | sed -E 's/\\x1B\\[[0-9;]*[a-zA-Z]//g'); [ "$cur" = "$prev" ] && stable=$((stable+1)) || stable=0; prev="$cur"; [ $stable -ge 3 ] && break; sleep 0.6; done`,
    `5. GET ${baseUrl}/screenshot --max-time 15 for visual. On 503 read logs.frontend_last_error. For vision use ?format=base64 then cut -d, -f2 | base64 -d. Pinned to bottom — top kali not hidden.`,
    `6. After cd: POST input "cd /tmp\\n" + sleep 0.3 + GET ${baseUrl}/cwd | jq -r .cwd to confirm.`,
    `7. On Ctrl-C/timeout: POST input with $'\\x03' via jq. Repeat. Don't DELETE.`,
    ``,
    `EXAMPLE — COPY-PASTE STABLE (stable 3, max 8)`,
    `BASE="${baseUrl}"`,
    `curl -sSf "$BASE/health" | jq .`,
    `curl -sSf -X POST "$BASE/clear" | jq .`,
    `curl -sSf -X POST -H "Content-Type: application/json" -d "$(jq -n --arg data "ls -la\\n" '{data:$data}')" "$BASE/input" | jq .`,
    `sleep 0.7`,
    `# poll dump-all until idle (stable 3), strip ANSI, show tail`,
    `prev=""; stable=0; for i in 1 2 3 4 5 6 7 8; do cur=$(curl -sSf "$BASE/output" | jq -r .data | sed -E 's/\\x1B\\[[0-9;]*[a-zA-Z]//g; s/\\x1B\\]0;.*\\x07//g'); [ "$cur" = "$prev" ] && stable=$((stable+1)) || stable=0; prev="$cur"; echo "---poll $i stable $stable total $(curl -s "$BASE/output" | jq .total)---"; echo "$cur" | tail -n 100; [ $stable -ge 3 ] && break; sleep 0.6; done`,
    `# screenshot fresh (holds 10s) — handle 503`,
    `curl -sSf --max-time 15 "$BASE/screenshot" -o /tmp/term.png && file /tmp/term.png || curl -s "$BASE/screenshot" | jq .logs`,
    `# base64 for LLM vision:`,
    `curl -sSf --max-time 15 "$BASE/screenshot?format=base64" | jq -r '.image // .error' | cut -d, -f2 | base64 -d > /tmp/term.png && file /tmp/term.png || jq . /tmp/term.png`,
    `# Ctrl-C if hung:`,
    `curl -sSf -X POST -H "Content-Type: application/json" -d "$(jq -n --arg data $'\\x03' '{data:$data}')" "$BASE/input" | jq .`,
    ``,
    `SAFETY: localhost only, no auth, CORS Any. Port IS secret. Don't log URL. POST /clear after secrets (passwords stay in 512KB ring). Unshare/close tab when done. Discovery files /tmp/aterm-*.port and ~/.config/aterm/shares/*.json leak port — chmod 700.`,
  ].join("\n");
}

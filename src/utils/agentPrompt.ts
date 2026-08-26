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
    `- POST /input — INVISIBLE OSC SENTINEL HOLD (DETAILED): transparent per-request __ATERM_DONE_<uuid>__. Server rewrites {"data":"sleep 10 && echo hi\\n"} → "sleep 10 && echo hi; printf '\\033]633;E;__ATERM_DONE_<uuid>__:%s\\007' \\"$?\\"\\r" (Ctrl-C/D/Z and empty skip sentinel). Shell prints ESC]633;E;__MARKER__:code BEL (0x1b…0x07) — xterm swallows OSC so screenshot stays clean (no visible marker line). Typed "; printf '\\033…'" is literal \\033 chars, output is real 0x1b byte, so detection \\x1b]633;E; matches only output, not echo.`,
    `  • Why sentinel: avoids first-output wake bug. sleep 10's typed echo wakes immediately, but loop holds until invisible OSC after hello.`,
    `  • Wake loop: wait_for_output(since, remaining) → pty append_output bumps version, drains oneshot; loop checks for "\\x1b]633;E;"+sentinel+":"; if not found set since=ver and re-wait until 300s deadline.`,
    `  • Fast path: if echo+output+OSC coalesce in one read (echo hi), first wake already has OSC → returns immediately.`,
    `  • Timeout: if sentinel not seen in 300s → returns {"data":dump,"total":N,"version":V,"id":...,"timedOut":true,"elapsedMs":N}. Check timedOut; for sleep 400 re-GET /output or re-POST.`,
    `  • Client timeout: set curl --max-time 310 (>300) or HTTP client read timeout >300s; server has no shorter cutoff.`,
    `  • Concurrency: concurrent POST /input each get unique sentinel; waiters queued per id, each woken with cloned dump.`,
    `- GET /output — dump-all, NO HOLD: returns current 512KB ring immediately (no wait, no since). Use to browse without executing or to re-check after timedOut.`,
    `- Dump-all output: GET /output returns whole 512KB ring as raw PTY bytes. Use jq -r .data | sed strip.`,
    `- Raw ANSI: strip before parsing. total is byte length of ring, version is monotonic u64 per append/clear (clear also bumps).`,
    `- Ring caps at 512KB — drain oldest. For big cat/cargo, GET /clear first then cmd | tail -n 300.`,
    `- Clear sync: shell clear/Ctrl-L auto-wipes both xterm and GET /output ring. GET /clear types 'clear\\r' into shell, shell emits ESC[2J, ring auto-wiped, screenshot invalidated.`,
    `- No auth, localhost only (127.0.0.1). CORS Any, port IS secret. All GET are no-store.`,
    `- Screenshot: GET /screenshot holds 10s for fresh html2canvas capture (no cache). 503 = timeout with logs.`,
    ``,
    `API (port is tab id — no :id in path)`,
    `- GET ${baseUrl}/health -> {"version","id","alive":bool,"pid":number|null}`,
    `- POST ${baseUrl}/input body {"data":"ls -la\\n"} -> {"data","total","version","id","exitCode":0,"elapsedMs":N} or {"data","total","version","id","timedOut":true} — ALWAYS holds up to 300s till invisible OSC \\x1b]633;E;__ATERM_DONE_<uuid>__:code\\x07; no wait param. Auto \\n→\\r + ghost fix; \\x03/\\x04 passthrough (no sentinel, holds for any output). Returns stripped data (OSC removed) + elapsedMs.`,
    `- GET ${baseUrl}/output -> {"data","total","version","id"}  current dump-all (max 512KB), no hold.`,
    `- GET ${baseUrl}/clear -> {"ok":true,"cleared":true,"id"}  wipes ring + screenshot (types clear into shell).`,
    `- GET ${baseUrl}/screenshot -> image/png (binary) or ?format=base64 -> {"image":"data:image/png;base64,...","id"}  holds 10s fresh.`,
    ``,
    `WORKFLOW — ONE-WAY SIMPLE (HOLD SEMANTICS)`,
    `0. GET ${baseUrl}/health | jq . abort if alive==false`,
    `1. GET ${baseUrl}/clear if history large (GET /output | jq .total) — resets version, next input holds from fresh`,
    `2. POST ${baseUrl}/input -d '{"data":"ls -la\\n"}' | jq -r .data | sed strip | tail -n 200  — ONE CALL holds up to 300s till output; inspect .timedOut (jq .timedOut) — if true, output incomplete, re-GET /output after sleep`,
    `3. GET ${baseUrl}/output | jq -r .data | tail -n 100  to browse without executing (no hold)`,
    `4. GET ${baseUrl}/screenshot --max-time 15 for visual (separate 10s hold)`,
    `5. After cd: run pwd via POST /input '{"data":"pwd\\n"}' (no /cwd endpoint) — holds as usual`,
    `6. On hang/long job >300s: POST /input '{"data":"\\u0003"}' (Ctrl-C) holds for its echo; if timedOut after 300s, loop GET /output polling`,
    ``,
    `EXAMPLE — ONE-WAY COPY-PASTE`,
    `BASE="${baseUrl}"`,
    `curl -sSf "$BASE/health" | jq .`,
    `curl -sSf "$BASE/clear" | jq .`,
    `# one-way exec — holds up to 300s till output (set --max-time 310)`,
    `curl -sSf --max-time 310 -X POST -H "Content-Type: application/json" -d "$(jq -n --arg data "ls -la\\n" '{data:$data}')" "$BASE/input" | jq -r .data | sed -E 's/\\x1B\\[[0-9;]*[a-zA-Z]//g; s/\\x1B\\]0;.*\\x07//g' | tail -n 100`,
    `# if timedOut, check flag and re-poll`,
    `curl -sSf --max-time 310 -X POST -H "Content-Type: application/json" -d "$(jq -n --arg data "sleep 1; echo done\\n" '{data:$data}')" "$BASE/input" | jq '{timedOut, version, total}'`,
    `# browse current output without exec`,
    `curl -sSf "$BASE/output" | jq -r .data | tail -n 100`,
    `# screenshot fresh capture`,
    `curl -sSf --max-time 15 "$BASE/screenshot" -o /tmp/term.png && file /tmp/term.png || curl -s "$BASE/screenshot" | jq .logs`,
    `# Ctrl-C if hung (also holds)`,
    `curl -sSf --max-time 310 -X POST -H "Content-Type: application/json" -d "$(jq -n --arg data $'\\x03' '{data:$data}')" "$BASE/input" | jq .`,
    ``,
    `SAFETY: localhost only, no auth, CORS Any. Port IS secret. Don't log URL. GET /clear after secrets. Discovery /tmp/aterm-*.port leak port — chmod 700.`,
  ].join("\n");
}

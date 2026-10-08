# 05 - Web GUI and integration service

Behaviour spec for the two HTTP servers Sarina ships. Written for implementers
who will not see the existing code. Ratings: ESSENTIAL / USEFUL / DROP-CANDIDATE.

Provenance: the GUI (server, routes, front end) is derived from the unlicensed
upstream and must be rebuilt from this spec. The integration service is mostly
original, but its contract is documented in full because external callers
(Chymaera_Sentry's advisory path, the distro's systemd unit) depend on it.
Wire formats below are the compatibility surface; names of fields are part of
that surface, wording of messages is not.

Both servers are plain request/response JSON over HTTP. **Neither uses
streaming, SSE or WebSockets.** A chat request blocks until the whole agent run
(all model turns and tool calls) finishes, then returns the full result. The
rewrite may add streaming to the GUI (recommended, see section 5), but the
service contract below must stay as is.

---

## 1. Shared concepts

### 1.1 Run result (returned by both chat endpoints)

JSON object:

| Field | Type | Meaning |
|---|---|---|
| `final_output` | string | The assistant's final text for this run |
| `turns` | int | Model turns used in this run |
| `tool_calls` | int | Tool invocations in this run |
| `transcript` | array of entries | Full conversation so far (see 1.2) |
| `session_id` | string | The agent-side (persisted) session id, usable as `resume_session_id` |
| `usage` | object | `input_tokens`, `output_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens`, `reasoning_tokens`, `total_tokens` (ints) |
| `total_cost_usd` | number | Estimated cost, 0 when no pricing configured |
| `stop_reason` | string or null | Why the run ended. Values seen: the model's own finish reason (`stop`, `length`, ...), `max_turns`, `budget_exceeded`, `resource_exhausted`, `backend_error`, `prompt_too_long`, `snipped_for_context`, `tool_completed`, `resume_load_error`. Treat unknown values as opaque strings. |

### 1.2 Transcript entry

`{ "role": "system"|"user"|"assistant"|"tool", "content": string }` plus these
optional keys, **present only when non-empty**: `name` (tool name, on `tool`
entries), `tool_call_id`, `tool_calls` (on `assistant`; OpenAI chat-completions
shape: list of `{id, type:"function", function:{name, arguments}}` with
`arguments` a JSON-encoded string), `metadata` (object; tool entries carry at
least an `ok` boolean and sometimes an `action` description), `message_id`.

### 1.3 Error shape

- Framework-style errors: `{"detail": "<message>"}` with 400/401/403/404/409/422/502.
- Unhandled exception inside a chat run: HTTP 500 with
  `{"error": "<message>", "error_type": "<exception class name>"}`. (Note the
  different keys from the `detail` form; keep both, clients branch on them.)
- Request-body validation failure (wrong types, missing required field,
  empty-string `prompt`): HTTP 422 with a `detail` array describing fields.

### 1.4 Persistence

Agent sessions are stored as one JSON file per session id in a session
directory (default: `.port_sessions/agent` relative to the process cwd). The
GUI reads those files directly for its session list. The file contents the
GUI relies on: `session_id`, `turns`, `tool_calls`, `messages` (transcript
entries), `usage`, `total_cost_usd`, `model_config.model`, `file_history`
(list of objects each having at least `timestamp`; other fields are
tool-defined). The session store itself is specified in another spec; this
document only needs it to provide: list, load-by-id (missing -> not found),
and resume-with-prompt.

---

## 2. Integration service (`sarina-service`)  - ESSENTIAL (external contract)

Module entry: `python -m sarina.service`; console script `sarina-service`.
Default **port 8899**, default host **127.0.0.1**.

### 2.1 CLI flags and environment

| Flag | Default | Notes |
|---|---|---|
| `--host` | `127.0.0.1` | Must stay loopback by default. |
| `--port` | `8899` | |
| `--token` | env `SARINA_SERVICE_TOKEN`, else unset | Bearer token required on every endpoint except `/healthz`. Unset or empty = auth disabled. |
| `--model` | env `OPENAI_MODEL`, else `Qwen/Qwen3-Coder-30B-A3B-Instruct` | Default model for sessions that omit one |
| `--base-url` | env `OPENAI_BASE_URL`, else `http://127.0.0.1:8000/v1` | OpenAI-compatible endpoint |
| `--api-key` | env `OPENAI_API_KEY`, else `local-token` | |
| `--session-dir` | `.port_sessions/agent` | Default dir where agent sessions persist |
| `--scratchpad-root` | `.port_sessions/scratchpad` | Default scratchpad root |
| `--max-sessions` | `20` | Pool bound (LRU) |

Startup behaviour: if no token is configured, print a prominent warning that
unauthenticated requests are accepted. Print a short banner (URL, auth on/off,
model, base URL, session dir and pool size). Run until killed. Environment
variables are read at process start only.

### 2.2 Auth

If a token is configured, every endpoint except `GET /healthz` requires header
`Authorization: Bearer <token>`; exact string match; otherwise HTTP 401
`{"detail": ...}`. No token configured: no check at all. No rate limiting, no
per-token scoping, no TLS (loopback trust model). The rewrite should compare
tokens in constant time (the original does not).

### 2.3 Endpoints

**GET /healthz** - no auth. 200 `{"status":"ok","active_sessions":<int>}`.

**POST /v1/sessions** - create a pooled session.
Request JSON:

| Field | Type | Default | Notes |
|---|---|---|---|
| `cwd` | string | required | Workspace dir; `~` expanded; must exist and be a directory else 400 |
| `model`, `base_url`, `api_key` | string or null | service defaults | Falsy value falls back to default |
| `allow_shell` | bool | false | Permits shell-command tools |
| `allow_write` | bool | false | Permits file-write tools |
| `temperature` | number | 0.0 | |
| `timeout_seconds` | number | 120 | Per model call |
| `max_turns` | int | 12 | Per run |
| `max_total_tokens`, `max_input_tokens`, `max_output_tokens`, `max_reasoning_tokens`, `max_tool_calls`, `max_delegated_tasks`, `max_model_calls`, `max_session_turns` | int or null | null (unlimited) | Budget limits |
| `max_total_cost_usd` | number or null | null | Budget limit |
| `session_directory` | string or null | service default | Where this session's transcripts persist |
| `scratchpad_root` | string or null | service default | |
| `tool_allowlist` | array of strings or null | null | See below |

Unknown request fields are ignored (this matters: callers rely on the response
`tools` field to detect an older service that ignored `tool_allowlist`).

Response 200: `{"session_id": "<32 hex chars>", "tools": <int>}`.

`tool_allowlist` semantics (load-bearing; Sentry's advisory path depends on
it): omitted/null = the full tool set; `[]` = **no tools at all** (pure chat,
the model is not even offered any tool definitions and no tool-choice setting
is sent to the backend); a list of names = only those tools exist in the
session; names that match nothing grant nothing (no error). `tools` in the
response is the exact count of tools the session really has (0 for `[]`; for a
list, the number of names that matched). The restriction must survive any
internal rebuild of the tool registry. The restriction removes tools; it is not
merely a call-time permission denial.

Creation does not contact the model backend; an unreachable backend only
surfaces on the first chat.

**POST /v1/sessions/{session_id}/chat** - run one user turn.
Request: `{"prompt": string (min length 1), "resume_session_id": string|null}`.
- Unknown/evicted `session_id`: 404 `{"detail": ...}`.
- Whitespace-only prompt: 400; empty-string prompt: 422.
- Turns on one pooled session are serialized (second concurrent request waits
  for the first). Different sessions run in parallel. Blocking: the HTTP
  response is held open for the whole run, which can be minutes; clients need
  long timeouts.
- Conversation continuity: if `resume_session_id` is given, resume that
  persisted agent session; otherwise resume the **previous run of this pool
  session** if any; otherwise start a fresh agent session. After each run the
  pool entry remembers the returned `session_id` for the next call. Resuming a
  persisted id that does not exist (in the pool session's session directory):
  404 with a "session to resume not found" style detail.
- Success: 200 with the run result (1.1). Exception in the run: 500 (1.3).
  Model-backend failures normally come back as 200 with
  `stop_reason:"backend_error"` and an explanatory `final_output`, not as 500.
- Input beginning with `/` may be handled locally as a slash command without a
  model call (the existing test uses this to run offline); the rewrite should
  preserve "a recognised slash command in the prompt is answered locally with
  200 and non-empty `final_output`" at least for one trivial command, or the
  offline test strategy must be replaced.

**DELETE /v1/sessions/{session_id}** - 200 `{"evicted": true}`; unknown id 404.
Drops the in-memory agent only; persisted transcripts stay on disk.

No list-sessions, get-session or stream endpoint exists. Do not add any without
reason; the surface is deliberately small.

### 2.4 Session pool

- In-memory map `session_id (uuid4 hex) -> entry{agent, session_directory,
  per-session lock, last agent session id, tool count, last-used}`.
- Bounded at `--max-sessions`; creating beyond the bound evicts the
  least-recently-used entry (use = create or any request that resolves the id).
  Evicted ids then 404. (The original docstring claims a later request would
  "reload" an evicted session; the actual behaviour is 404. Keep 404; callers
  must create a new session and may pass the old agent `session_id` as
  `resume_session_id` to continue the conversation from disk.)
- Pool state is lost on process restart; transcripts are not.

### 2.5 Deployment (packaging/arch, owned by Chymaera_Linux's build)

systemd unit `sarina-service.service`: dedicated user/group `sarina`
(sysusers file), `StateDirectory=sarina` (`/var/lib/sarina`), working dir
`/var/lib/sarina`, optional `EnvironmentFile=-/etc/sarina/service.env`,
`ExecStart=/usr/bin/sarina-service --host 127.0.0.1 --port 8899 --session-dir
/var/lib/sarina/sessions --scratchpad-root /var/lib/sarina/scratchpad`,
`Restart=on-failure`, `RestartSec=5`, hardening `NoNewPrivileges`,
`ProtectSystem=strict`, `ProtectHome`, `PrivateTmp`. The env file template
sets `SARINA_SERVICE_TOKEN` (empty by default), `OPENAI_BASE_URL`,
`OPENAI_API_KEY`, `OPENAI_MODEL`. These files and the three console-script
names (`sarina`, `sarina-gui`, `sarina-service`) are a cross-repo contract; the
Go rewrite must keep the same binary/flag/env interface (a `sarina-service`
entry point accepting exactly the flags above) or coordinate with
Chymaera_Linux. Note `ProtectSystem=strict` means sessions can only write under
the state directory; `cwd` for a session must be readable by user `sarina`.

### 2.6 Security rules for the rewrite

1. Loopback bind by default; warn loudly when binding non-loopback without a
   token (the original only warns when there is no token).
2. The caller controls `allow_shell`, `allow_write`, `cwd`, `session_directory`,
   `scratchpad_root`, `base_url` and `api_key` per session. With auth off,
   **any local process gets shell+write on any directory the service user can
   reach**. Recommended hardening (compatible, opt-in or default-off):
   a server-side ceiling (e.g. flags to forbid `allow_shell`/`allow_write`
   entirely, or restrict `cwd`/`session_directory` to configured roots), and
   refusing to start without a token when bound to a non-loopback address.
3. Never log request prompts or API keys. Never echo `api_key` in any response.
4. Callers treat model output as untrusted; the service must not execute
   anything on its own initiative.
5. Callers that need a tool-less session must be able to verify it from the
   response (`tools: 0`); keep that field truthful.

### 2.7 Test expectations (carry over as acceptance tests, offline)

healthz ok even with a token set; create returns an id; missing cwd 400;
slash-command chat 200 with `final_output`; unknown session chat 404; blank
prompt 400; delete then re-use 404; delete unknown 404; 401 without/with wrong
token, 200 with right token; no token = open; allowlist: default has >20
tools, `[]` gives 0 tools and chat still works, named list gives exact count,
unknown names grant none; backend request omits tool definitions and tool
choice when the tool list is empty, and sends them unchanged otherwise.

---

## 3. Web GUI server (`sarina-gui`)

Entry: `python -m sarina.gui`; console script `sarina-gui`. Default
**port 8765**, host **127.0.0.1**. A single-operator, single-workspace
console: one global agent instance, one global lock, **no authentication,
no per-request workspace**. Rating: server ESSENTIAL (as a minimal chat/approval
console), most auxiliary panels DROP-CANDIDATE (see 3.3).

### 3.1 CLI flags

Every flag has a runtime equivalent in `POST /api/state` except where noted.

| Flag | Default | Meaning |
|---|---|---|
| `--host` | 127.0.0.1 | |
| `--port` | 8765 | |
| `--cwd` | `.` | Workspace dir the agent works in |
| `--model` | env `OPENAI_MODEL`, else `Qwen/Qwen3-Coder-30B-A3B-Instruct` | |
| `--base-url` | env `OPENAI_BASE_URL`, else `http://127.0.0.1:8000/v1` | |
| `--api-key` | env `OPENAI_API_KEY`, else `local-token` | never returned by the API |
| `--session-dir` | `.port_sessions/agent` | |
| `--allow-shell`, `--allow-write` | off | Permission toggles (off by default) |
| `--temperature` | 0.0 | |
| `--timeout-seconds` | 120 | per model call |
| `--stream` | off | Ask the backend for streamed model responses (internal only; the HTTP API still returns one blocking response) |
| `--max-turns` | 12 | |
| `--max-total-tokens`, `--max-input-tokens`, `--max-output-tokens`, `--max-reasoning-tokens`, `--max-tool-calls`, `--max-delegated-tasks`, `--max-model-calls`, `--max-session-turns` | unset | Budgets |
| `--max-budget-usd` | unset | cost budget |
| `--system-prompt`, `--append-system-prompt`, `--override-system-prompt` | unset | Custom / appended / fully replacing system prompt |
| `--response-schema-file` (+ `--response-schema-name` default `response`, `--response-schema-strict`) | unset | JSON file with a structured-output schema |
| `--auto-snip-threshold`, `--auto-compact-threshold` | unset | Token thresholds for automatic context trimming/compaction |
| `--compact-preserve-messages` | 4 | |
| `--disable-claude-md` | off | Skip project-instruction file discovery |
| `--add-dir` (repeatable) | none | Extra directories the agent may use |
| `--no-browser` | off | Otherwise open the default browser on startup |

Startup prints the URL and a summary (cwd, model, base URL, session dir,
shell/write on/off). Rating: core flags (host, port, cwd, model, base-url,
api-key, session-dir, allow-shell, allow-write, max-turns, no-browser)
ESSENTIAL; budgets, system-prompt overrides, response schema, compaction
thresholds, add-dir USEFUL (they mirror the main CLI); `--stream` USEFUL only if
the rewrite streams.

### 3.2 State semantics

`AgentState` holds the settings above plus the live agent. Any settings change
**rebuilds the agent** (conversation continuity comes from persisted session
files, not live memory, so this is safe; but any non-persisted runtime state is
lost - the rewrite should avoid that coupling). Settings updates and chat runs
take the same lock, so a settings save waits for a running chat.

### 3.3 Endpoints

All JSON. Unless noted, errors use the `detail` form (1.3). Static files:
`GET /` returns the single-page app; `GET /static/*` serves its assets.
No CORS configuration. No auth. No Host/Origin checking (see 3.5).

#### Core (ESSENTIAL unless noted)

| Method + path | Request | Response / behaviour | Rating |
|---|---|---|---|
| GET `/` | - | the SPA HTML | ESSENTIAL |
| GET `/api/state` | - | Snapshot object: `model, base_url, cwd, session_directory, allow_shell, allow_write, temperature, timeout_seconds, stream_model_responses, max_turns`, the nine budget fields (null = unlimited), the three prompt fields, `response_schema, response_schema_name, response_schema_strict, auto_snip_threshold_tokens, auto_compact_threshold_tokens, compact_preserve_messages, disable_claude_md_discovery, additional_working_directories` (list of strings), `active_session_id`. Never includes `api_key`. | ESSENTIAL |
| POST `/api/state` | Partial object of the same keys plus `api_key`. **Omitted = unchanged; explicit null = clear** (for budgets, prompts, schema, thresholds meaning "unlimited/default"); empty-string prompts also clear. | 200 new snapshot. 400 on validation: `cwd`/extra dirs must exist; temperature >= 0; timeout > 0; max_turns >= 1; budgets and thresholds must be positive numbers or null; schema must be an object; schema name non-empty; compact_preserve >= 0. Wrong JSON types give 422. | ESSENTIAL (subset: model, base_url, api_key, cwd, allow_shell, allow_write, temperature, max_turns); rest USEFUL |
| POST `/api/chat` | `{prompt (string, min length 1), resume_session_id: string|null, pasted_contents: {<int id>: {type:"text", content, media_type?, filename?}}}` | Expands `[Pasted text #N ...]` placeholders in the prompt from `pasted_contents` before running; then runs one turn (new session, or resume if `resume_session_id` given). 200 = run result (1.1). Whitespace-only prompt 400; resume id missing 404; exception 500 `{error, error_type}`. Blocks for the whole run. | ESSENTIAL (pasted_contents USEFUL) |
| POST `/api/clear` | - | Clears the agent's runtime state (task/plan/etc. in-memory state, not saved sessions); returns snapshot | USEFUL |
| GET `/api/sessions` | - | Array, newest-modified first, of `{session_id, turns, tool_calls, preview (first 120 chars of first string user message), modified_at (unix seconds)}`; unreadable files skipped silently; `[]` if directory missing | ESSENTIAL |
| GET `/api/sessions/{id}` | - | `{session_id, turns, tool_calls, messages (normalized entries), usage, total_cost_usd, model, file_history}`; 404 if absent | ESSENTIAL |
| GET `/api/slash-commands` | - | Array of `{names:[...], primary, description}` | USEFUL (palette) |
| GET `/api/skills?include_internal=false` | - | Array of `{name, description, when_to_use, aliases, allowed_tools, user_invocable}`; non-user-invocable skills hidden unless `include_internal=true` | DROP-CANDIDATE (see skills spec; keep only if skills survive) |
| GET `/api/file-history?limit=200` | - | `{total, returned, entries}`; entries are every session's `file_history` records with `session_id` added, newest first by `timestamp`, capped at `limit` | USEFUL (audit of what files the agent touched) |

Missing from the original and **required for a good rewrite** (new, ESSENTIAL):
an approval/permission-request flow (3.6), a way to cancel a running turn, and
streamed progress.

#### Auxiliary panels (all workspace-scoped; each re-reads on-disk state per request)

All of these mirror agent-side "runtime" features that are specified (or
dropped) in other docs. Rating applies to the **HTTP endpoints and panel**
together. The general verdict: a local-model security agent used by one operator
does not need an admin UI for every internal runtime; they are parity features.

| Group | Endpoints | Behaviour summary | Rating |
|---|---|---|---|
| Tasks | GET `/api/tasks`; POST `/api/tasks` (create: `title` required, `description, status (default pending), priority, active_form, owner, blocks[], blocked_by[], metadata{}`); PATCH `/api/tasks/{id}` (partial, plus `merge_metadata` bool); POST `/api/tasks/{id}/start`, `/complete`, `/cancel` (`{reason?}`), `/block` (`{blocked_by?, reason?}`) | GET returns `{storage_path, tasks[] (each task dict plus is_next_actionable), counts{status:n}}`; mutations return `{task, state}`. Invalid status 400, unknown id 404. Completing a task unblocks dependents. | USEFUL (shows the agent's working list; read-only view would suffice) |
| Plan | GET `/api/plan`; PUT `/api/plan` (`{steps:[{step (required), status, task_id, description, priority, active_form, owner, depends_on[]}], explanation?, sync_tasks=true}`); POST `/api/plan/clear` (`{sync_tasks=true}`) | Whole-plan replace; response `{storage_path, explanation, updated_at, steps[], counts}`; invalid status 400. | DROP-CANDIDATE (duplicate of tasks) |
| Memory files | GET `/api/memory` (discovered instruction/rule files: `{cwd, additional_working_directories, global_dir, files[{path,name,parent,size,modified_at,writable}]}`); GET `/api/memory/file?path=`; PUT `/api/memory/file` (`{path, content}`); DELETE `/api/memory/file?path=` | Edits the human-curated project instruction files. Reads allowed for any discovered file (404 if missing); **writes/deletes are sandboxed to the workspace, extra dirs and the user's global config dir, else 403**. Delete returns `{deleted}`. | USEFUL (editing project instructions) - preserve the 403 sandbox rule exactly. NOTE: does not cover the newer agent-curated memory store; a rewrite should expose that read-only. |
| Background | GET `/api/background` (`{root, sessions[], counts{running,completed,failed,exited}}`); GET `/api/background/{id}`; GET `/api/background/{id}/logs?tail=N` (`{background_id, tail, content}`); POST `/api/background/{id}/kill` | Observe/kill detached agent runs; no launch endpoint. 404 unknown. | USEFUL if background runs survive; else DROP |
| Worktree | GET `/api/worktree`; POST `/api/worktree/enter` (`{name?}`); POST `/api/worktree/exit` (`{action: "keep"|"remove"-style, discard_changes}`) | Creates/leaves a managed git worktree and **changes the agent's cwd** (server rebuilds the agent). Response `{status, state_path, history[], has_state}`; errors 400. | DROP-CANDIDATE |
| Account | GET `/api/account`; POST `/api/account/login` (`{target, provider?, auth_mode?}`); POST `/api/account/logout` (`{reason}`) | Manifest-driven fake "profiles"; no real credentials. | DROP-CANDIDATE |
| Remote | GET `/api/remote`; POST `/api/remote/connect` (`{target, mode?}`); POST `/api/remote/disconnect` | Same shape as account; state only. | DROP-CANDIDATE |
| Remote triggers | GET/POST `/api/remote-triggers`; PATCH `/api/remote-triggers/{id}`; POST `/api/remote-triggers/{id}/run` | Stored trigger definitions plus run history; 409 on duplicate create, 404 unknown. | DROP-CANDIDATE |
| MCP | GET `/api/mcp?include_remote=false`; POST `/api/mcp/resources/read` (`{uri, max_chars=12000}`); POST `/api/mcp/tools/call` (`{tool_name, server_name?, arguments{}, max_chars}`) | Lists configured MCP servers/resources/tools; reading/calling spawns stdio subprocesses (404 not found, 500 `tool call failed`). **A raw tool-call endpoint with no approval is dangerous on an unauthenticated server.** | USEFUL as read-only server status; DROP the call/read endpoints |
| Plugins | GET `/api/plugins` | Read-only manifest listing + instruction blocks. | DROP-CANDIDATE |
| Ask-user queue | GET `/api/ask-user`; POST `/api/ask-user/queue` (`{answer, question?, question_id?, header?, match "exact"|"contains" (else 400), consume=true}`); DELETE `/api/ask-user/queue/{index}` (404 out of range); POST `/api/ask-user/clear-history` | Pre-stages canned answers for the agent's ask-the-user tool so non-interactive runs get answers. Poked into the runtime's private persistence. | DROP-CANDIDATE (replace by real interactive approvals/questions, 3.6) |
| Workflows | GET `/api/workflows`; POST `/api/workflows/{name}/run` (`{arguments{}}`) | Catalogue from workspace JSON + run history; run only records history. | DROP-CANDIDATE |
| Search | GET `/api/search`; POST `/api/search/activate/{name}`; POST `/api/search/query` (`{query, provider_name?, max_results=5, domains[], timeout_seconds=20}`) | Web-search provider config; query errors 400 (lookup), 404, 502 (provider failure). | DROP-CANDIDATE (the agent's own web tool is enough) |
| Teams | GET/POST `/api/teams`; DELETE `/api/teams/{name}`; POST `/api/teams/{name}/messages` (`{text, sender, recipient?, metadata}`) | Local team/message log; 409 duplicate, 404 unknown. | DROP-CANDIDATE |

(One stale compiled artefact suggests a `diagnostics` route once existed; no
spec is needed.)

### 3.4 Concurrency and lifecycle

- One agent, one lock: a second chat or a settings change while a run is in
  flight waits (no 409/423 is returned). The UI disables input while busy.
- Auxiliary endpoints do not take the lock and read disk fresh each time.
- No request timeouts server-side; the model-call timeout bounds each backend call.

### 3.5 Security rules (gaps the rewrite must close)

1. Binds loopback by default, but the server has **no auth, no Origin/Host
   check, and permissive-by-omission CORS**. Any web page the operator visits
   can send cross-site `POST` requests (simple JSON posts work against
   loopback) or use DNS rebinding to drive an agent that may have shell and
   write enabled, flip `allow_shell`, or write memory/instruction files.
   Required: validate `Host` (loopback names only) and `Origin` (same origin or
   absent) on every request, require `Content-Type: application/json` on
   mutations, and generate a random per-launch token embedded in the page it
   serves and checked on `/api/*` (printed in the startup URL / opened with the
   browser).
2. `allow_shell` / `allow_write` start **off**; enabling them from the browser
   should require an explicit confirm and be shown persistently in the UI.
3. `/api/memory/file` writes stay sandboxed (workspace, extra dirs, global
   config dir). Reads should be limited to discovered files; the original lets
   any readable path be read via `path=` - tighten.
4. `api_key` is write-only over HTTP.
5. Refuse non-loopback `--host` unless a token mechanism is active.
6. Opening a browser on start is convenience only; honour `--no-browser`.

### 3.6 Approvals (new requirement)

The original has **no interactive approval**: tool use is gated only by the two
boolean toggles evaluated at call time; "ask the user" is a pre-staged answer
queue. A single-operator security console needs real, in-band approvals:

- When the agent wants to run a tool classified as needing consent (shell,
  file write/edit, network fetch, MCP call, anything the permission policy
  marks "ask"), the run pauses and the server emits a pending-approval record:
  `{approval_id, session_id, tool_name, arguments (full), human summary,
  risk hint}`. The operator answers allow-once / allow-for-session / deny, with
  optional edited arguments or reason; the answer is returned to the agent as the
  tool result when denied.
- Endpoints to add (proposed shapes, no legacy to match): `GET /api/approvals`
  (pending), `POST /api/approvals/{id}` (`{decision, reason?}`), and a
  cancel endpoint `POST /api/chat/cancel`.
- Same for the agent's "ask the user a question" tool: surface the question
  and accept a free-text/choice answer instead of the canned queue.
- Unanswered approvals time out as deny.

---

## 4. Front end (GUI) - what exists today

Single static page: sidebar + main area, plain JavaScript, no framework, no
build step, no persistence in the browser except in-memory state. The whole
front end is to be redesigned; this section records behaviour so nothing
useful is lost.

### 4.1 Layout

- **Sidebar**: "New chat" button; saved-session list (preview text, turns,
  relative time; click to open); a collapsible **settings form** (below).
- **Top bar**: status indicator (Ready / Busy / Error) with text; row of 17
  view tabs; three icon buttons: slash-command palette, skills palette,
  clear runtime state.
- **Main area**: one active view at a time. Chat view has the message
  stream and a composer pinned at the bottom.
- **Composer**: multi-line text box that auto-grows; Enter sends, Shift+Enter
  newline; Send button; footer line shows the working directory and, after a
  run, usage (`turns`, `tool calls`, `total tokens`). Input is disabled while a
  run is in flight.

### 4.2 Chat behaviour

- Welcome message when the transcript is empty.
- User messages and assistant messages rendered with a small hand-rolled
  markdown renderer (headings, bold/italic, inline code, fenced code blocks
  with language label, links opening in a new tab, simple bullet lists).
  Output is HTML-escaped first. **Rewrite must use a vetted sanitising renderer
  or build DOM nodes; never insert model text as markup.**
- Each tool call is a collapsed disclosure showing tool name and a one-line
  argument preview; expanding shows full arguments (pretty JSON) and the
  result text, with an "error" marker when the tool entry's `metadata.ok` is
  false. System messages are hidden.
- Sending: the user bubble is appended immediately; the request is a single
  blocking `POST /api/chat` carrying `resume_session_id` = the currently open
  session id (null for a new chat). When the response arrives, tool messages
  and the final assistant text since the last user message are rendered, usage
  line updated, active session id set to the returned `session_id`, session
  list refreshed. If `error` present, an error bubble shows `error_type: error`.
  There is **no progress during a run** beyond a "busy" status.
- Opening a saved session: `GET /api/sessions/{id}`, clears the chat, replays
  the transcript (assistant tool calls shown as pending until their tool
  results follow), sets it as the resume target. "New chat" clears the chat and
  drops the resume target.
- Large paste handling: pastes over 500 characters are collapsed into a
  placeholder token `[Pasted text #N +L lines]` in the input plus a removable
  chip; the real text is sent in `pasted_contents` and expanded server-side.
- Slash palette (button or typing `/`): searchable list from
  `/api/slash-commands`; choosing inserts the command. Skills palette
  similar from `/api/skills`.
- Clear button: `POST /api/clear` after confirmation, resets the view.

### 4.3 Settings form (sidebar)

Editable fields mapped 1:1 to `POST /api/state`: model, base URL, API key
(password field, never pre-filled), working directory, extra working
directories (one per line), allow-shell and allow-write toggles, stream
toggle, temperature, timeout, max turns, auto-snip and auto-compact
thresholds, compact-preserve count, disable-instruction-files toggle, three
system-prompt text areas, response schema (JSON text area) with name and
strict toggle, nine budget numeric fields (blank = unlimited -> send null).
Save button applies and refreshes the status/cwd line. Server validation
errors (400) are shown inline.

### 4.4 Other views (each is a refresh button plus lists/forms over the
section 3.3 endpoints)

Tasks (list with status counts, per-task start/complete/cancel/block buttons,
add form); Plan (editable ordered rows with add/remove/status, explanation,
save/clear, "sync tasks" toggle); Memory (file list on the left, editor on the
right with save/delete/new-file); History (aggregate file-change log across
sessions); Background (list, log viewer, kill, reload logs); Worktree (enter
form, exit with keep/remove and discard toggle, history); Skills (list with
descriptions, "show internal" toggle); Accounts, Remote, Remote triggers,
Workflows, Search, Teams, Ask queue, MCP (servers/resources/tools with read and
call actions), Plugins - simple tables and forms.

### 4.5 Minimal good single-operator UI (requirements for the redesign)

ESSENTIAL
1. **Chat stream** with streamed assistant text (add streaming; see 5),
   markdown + code blocks, copy buttons, stop/cancel button, busy state, clear
   error display including `stop_reason` and backend-unreachable guidance.
2. **Tool-call display**: inline, collapsible, name + args + result + ok/error;
   long outputs truncated with expand; shows which tools ran with write/shell
   effect distinctly.
3. **Approvals**: modal or inline card per 3.6; blocks the run, shows full
   arguments (commands/paths/diffs), allow once / allow for session / deny.
   Persistent indicator of current permission mode (read-only vs write vs
   shell).
4. **Session list**: saved sessions with preview, time, tool/turn counts;
   open (resume), new chat, search box (USEFUL), delete (not in original).
5. **Settings**: model, base URL, API key, workspace dir, permission toggles,
   max turns, a budget or two, system-prompt append. Show backend connectivity
   state and the active model.
6. **Status bar**: workspace, model, permission mode, token/cost usage for the
   session, connection status.

USEFUL
7. File-change history/diff view for the session (from `file_history`).
8. Task list read-only panel (what the agent plans to do).
9. Instruction-file (CLAUDE.md-style) editor.
10. Slash-command palette.
11. Read-only view of agent-curated memory and pending memory-review
    proposals with approve/reject (these exist as CLI features; a GUI is
    natural because approval must be a human act).
12. Large-paste collapsing; keyboard shortcuts; dark/light theme following OS.

DROP-CANDIDATE: every panel for accounts, remote, remote triggers, workflows,
search providers, teams, ask-queue, plugins, worktree, plans; the skills
palette; 17-tab navigation. Reintroduce one only if the underlying agent
feature survives the clean-room cut.

Accessibility and robustness: keyboard-operable, labelled controls, no
markup injection from model/tool text, no external network fetches from the page
(fonts, CDNs): the page must work fully offline and ship as embedded static
assets in the Go binary.

---

## 5. Recommended contract changes for the rewrite (not in the original)

1. Streaming for the GUI: a `POST /api/chat` variant (or `GET` event stream)
   emitting server-sent events: `turn_start`, `text_delta`, `tool_call`
   (name, args), `approval_request`, `tool_result` (ok, content), `usage`,
   `done` (the run result of 1.1), `error`. Keep the blocking endpoint working
   for tests and scripts.
2. Cancel endpoint and server-side run state (idle/running/waiting-approval),
   so the UI can reattach after a page reload.
3. Do **not** change the service contract in section 2; if streaming is added
   there it must be a new endpoint (e.g. `POST /v1/sessions/{id}/chat/stream`),
   not a change to existing response shapes.
4. Share one run-result serializer between GUI and service (as the original
   does) so field names stay identical.
5. One agent per GUI process is fine, but make settings changes not discard
   in-flight/unsaved agent state, and report clearly when a change needs a new
   session.

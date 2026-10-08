# 03 - Extensibility and multi-agent modules

Clean-room behaviour spec (see `README.md`, `docs/PROVENANCE.md`). Describes what
the existing Python modules do, observably, so a Go implementation can be written
without seeing them. All prompt, description and help text below is described by
intent only; the rebuild must write its own wording.

Honest framing up front: most of this area exists because the upstream project set
out to mirror a cloud coding assistant feature-for-feature. A local-model security
agent on Chymaera Linux needs only a small subset. Several modules are *simulations*
that record state but never act (remote, remote triggers, workflows, account, teams).
They are rated accordingly. The ratings table is in section 0; details follow.

Conventions used by all modules in this area:
- "Workspace" = the `--cwd` directory (plus any "additional working directories").
- Most manifests are JSON, read leniently: an unreadable or malformed file is
  silently ignored (treated as absent); entries with missing required fields are
  silently skipped. No module raises on bad config at load time.
- Name lookups (servers, teams, workflows, triggers, profiles) are case-insensitive
  and whitespace-trimmed; the stored name keeps its original case.
- Runtime state that mutates (teams, workflow history, triggers, remote/account
  sessions) lives as JSON under `<workspace>/.port_sessions/`, rewritten whole on
  every change, UTF-8, pretty-printed (2-space indent), ASCII-escaped.
- Timestamps are UTC ISO-8601 strings with offset.
- Tool results are rendered as plain text with a leading markdown-style heading
  line (e.g. `# Teams`) followed by `key=value` lines or `- item ; item ; item`
  bullets. Exact text is not a contract; consumers are the model and humans.

---

## 0. Ratings

| Module | Rating | Reason |
|---|---|---|
| MCP client (`mcp_runtime.py`) | USEFUL | Only generic way to bolt external tools (Limbo, Sentry, Flipper bridge) onto the agent; but must be redesigned (persistent connection, trust gate). |
| LSP (`lsp_runtime.py`) | DROP-CANDIDATE | Not LSP at all: a regex/AST symbol indexer. grep + read already cover this; a real LSP client is a separate, large project. |
| Plugin manifests (`plugin_runtime.py`) | DROP-CANDIDATE | Plugins are static JSON that inject text and alias/forbid tools; nothing executes. Overlaps hook policy and MCP. |
| Plugin cache summary (`agent_plugin_cache.py`) | DROP-CANDIDATE | Reads a cache file written by a different product and prints a count into the prompt; no behaviour. |
| Bundled skills (`bundled_skills.py`) | DROP-CANDIDATE | Four canned coding-assistant prompts (diff review, verify, debug, settings help); irrelevant to Sarina's job. |
| `Skill` tool | DROP-CANDIDATE | Thin dispatcher over bundled skills and slash commands; drop with them. |
| Built-in agent definitions (`builtin_agents.py`) | USEFUL (partly DROP) | A read-only explorer/planner/verifier subagent profile set is worth keeping; `statusline-setup` and `claude-code-guide` are DROP-CANDIDATE (they serve another product). |
| Agent registry (`agent_registry.py`) | USEFUL (read path) / DROP-CANDIDATE (write path) | Loading user/project agent profiles from markdown is a cheap, good extension point; create/update/delete CLI is not needed (users can edit files). |
| `Agent` / `delegate_agent` tool (delegation engine, in `agent_runtime.py`) | USEFUL | Subagents (read-only recon, verification) matter for a security agent; the dependency-batch machinery is over-built (see 6.5). |
| Agent manager (`agent_manager.py`) | USEFUL (small) | In-memory bookkeeping of parent/child agents and groups; only needed if delegation is kept; could be 30 lines. |
| Teams (`team_runtime.py`) | DROP-CANDIDATE | A JSON message board with no delivery; no agent ever reads a message. |
| Workflows (`workflow_runtime.py`) | DROP-CANDIDATE | "Running" a workflow only records a history entry and prints substituted step text; it executes nothing. |
| Remote (`remote_runtime.py`) | DROP-CANDIDATE | Simulation: `connect` stores a record and prints it; nothing connects. Sarina's real remote needs (Chymaera console, fleet) are separate modules. |
| Remote triggers (`remote_trigger_runtime.py`) | DROP-CANDIDATE | Same: "run" appends a `queued` record; nothing ever dequeues it. |
| Account (`account_runtime.py`) | DROP-CANDIDATE | "Login" stores an identity string; no authentication, no tokens. Also exposed as model-callable tools, which is a smell. |
| Hook policy (`hook_policy.py`) - plugin-adjacent parts only | USEFUL | The tool deny-list (exact + prefix), trust flag and budget overrides are real security controls; the "inject a message before/after tool" parts are DROP-CANDIDATE. Covered fully by another spec file if one exists; summarised in 5. |

If the rebuild keeps only one thing from this file: MCP stdio client, subagents with
read-only profiles, and the hook-policy deny-list. Everything else can come back
later from this document if a real need appears.

---

## 1. MCP (Model Context Protocol) client

**Governing public spec:** the Model Context Protocol specification (JSON-RPC 2.0
messages; this implementation advertises protocol version string `2025-11-25`).
Only Sarina-specific behaviour follows. Rating: USEFUL.

### 1.1 What it is (and is not)
- A **client only**, for **stdio transport only**. It launches a configured command
  as a child process and speaks newline-delimited JSON-RPC over its stdin/stdout.
- Servers declaring any other transport (anything but `stdio`, case-insensitive) are
  silently dropped at load time (not listed, no error). No HTTP/SSE/streamable-HTTP.
- It also supports **static local resources** declared directly in the manifest
  (no server process needed).
- No MCP prompts, sampling, roots, subscriptions, notifications handling, or
  OAuth. Client capabilities sent in `initialize` are an empty object.

### 1.2 Config discovery
Manifest filenames looked for in each root, in this order per root:
`.sarina-mcp.json`, `.mcp.json`, `.codex-mcp.json`, `mcp.json`.
Roots: the workspace directory and **every ancestor directory up to the filesystem
root**, then each additional working directory. (Gotcha: ancestor walk means a
manifest in a parent or home directory applies. Combined with 1.4 this is a
code-execution-from-untrusted-repo risk; the rebuild should gate manifest-spawned
commands on a trust decision. See 1.7.)
Duplicate files (same resolved path) are read once. Order of discovery is
preserved: workspace first, then upward.

### 1.3 Manifest format
Top-level JSON object. Recognised keys (all optional):
- `name` (string): label for top-level `resources`; default `local`.
- `resources`: array of resource objects (see below), attributed to server `name`.
- `servers`: array of server objects `{ "name": ..., "command": ..., ... }`.
- `mcpServers`: object mapping server name -> server object (the common
  "Claude/desktop-style" shape). Both `servers` and `mcpServers` may appear and are
  merged.

Server object fields: `command` (string, **required**, else the server is skipped),
`args` (array of strings; non-strings dropped; non-array -> empty), `env` (object of
string->string; non-string pairs dropped), `cwd` (string; `~` expanded; relative
paths resolve against the manifest's directory), `description`, `transport`
(default `stdio`), `metadata` (object, passed through untouched), and optionally a
nested `resources` array (attributed to that server). A `servers` entry also needs
a non-empty string `name`.

Resource object fields: `uri` (required, non-empty; duplicates within one list
dropped), `name`, `description`, `mimeType` (alias `mime_type`), `path` (alias
`file`; `~` expanded; relative resolved against the manifest directory), `text`
(inline content), `metadata`.

Servers are de-duplicated across manifests by (lowercased name, transport, command,
args); the first occurrence wins.

### 1.4 Process lifecycle and protocol flow
**One short-lived process per request.** Every list/read/call spawns the server,
performs the handshake, sends exactly one request, then tears the process down.
(Consequences: stateful servers lose state between calls; each call pays startup
cost; listing resources/tools across N servers costs N spawns. A rebuild may keep a
connection pool, but must preserve observable semantics below.)
Spawn: command + args, stdin/stdout/stderr piped, text mode, working directory =
server `cwd` if set, environment = parent environment overlaid with the server's
`env`.
Handshake: send `initialize` (id 1) with the protocol version, empty capabilities
and `clientInfo` name `sarina`, version `0.1.0`; wait for the response; an error
response aborts. Then send the `notifications/initialized` notification. The
server's returned capabilities/version are not inspected or negotiated.
Request: send one request with the next integer id; wait for the response with the
matching id. Lines from stdout that are not JSON objects, or whose id does not match,
are ignored. stderr is read and kept (last 5 lines are appended to timeout errors),
never shown otherwise.
Shutdown: close stdin; if still running, terminate, wait up to 1s, then kill.

### 1.5 Timeouts and failure handling
- **10 second** deadline per awaited response (handshake and request each).
  Timeout raises an error containing the server name and the last stderr lines.
- JSON-RPC `error` response -> error of the form "MCP <method> failed for server
  <name>: <message>".
- Missing/empty `command`, failure to spawn -> error.
- A non-object `result` is treated as an empty result.
- **Discovery is forgiving:** when listing resources or tools across servers, a
  server that fails (spawn error, timeout, protocol error) is skipped silently and
  the others still list. When reading/calling against a specific server, failure
  surfaces as an error to the model (tool result with `ok=false`).
- Unknown server/tool/resource -> "not found" style errors, surfaced to the model.

### 1.6 How tools and resources are exposed to the model
There is **no per-MCP-tool registration**. MCP tools do not appear in the model's
tool list under their own names and are **not name-prefixed** (no `mcp__server__tool`
scheme). Instead four generic built-in tools exist (names are snake_case):

- `mcp_list_resources` - params: `query` (substring filter), `max_resources`
  (1-200, default 50). Output: one line per resource: `uri ; server=<s> ; name=<n> ;
  mime=<m> ; path=<resolved>` (fields omitted when absent). Empty -> `(no MCP resources)`.
  Merges manifest-declared resources with those reported live by each stdio server
  (`resources/list`). Filter = case-insensitive substring over uri, server name,
  name, description.
- `mcp_read_resource` - params: `uri` (required), `max_chars` (1-50000, default =
  the session's tool-output limit). Resolution order: manifest resources with that
  URI first (inline `text` wins over `path`; missing file -> not-found error), then
  a live `resources/read` against the server that listed it, then against every other
  stdio server in turn until one returns non-empty content. Content rendering: text
  parts used verbatim; binary `blob` parts rendered as `[blob:<mime>] <base64>`;
  other parts as indented JSON; parts joined with a blank line.
- `mcp_list_tools` - params: `query`, `server`, `max_tools` (1-200, default 50).
  Calls `tools/list` on every stdio server (or the one named). Output: `name ;
  server=<s> ; <description>` per line; empty -> `(no MCP tools)`. Filter =
  substring over tool name, description, server name.
- `mcp_call_tool` - params: `tool_name` (required), `server`, `arguments` (object;
  null -> `{}`), `max_chars`. The tool is looked up by exact name via a fresh
  `tools/list`; if the same name exists on several servers and `server` is not given
  -> error asking the caller to disambiguate. Then `tools/call` with `name` and
  `arguments`. Result rendering: text content items verbatim; items carrying
  structured content or other shapes as indented JSON; a top-level
  `structuredContent` appended as JSON; if nothing renderable, the whole result as
  JSON. Joined with blank lines. The MCP `isError` flag is **not** turned into a
  tool failure: the call succeeds and `is_error` is reported in result metadata
  (`mcp_is_error`) and in CLI output.
- Output truncation (all of the above): if longer than the limit, keep the first and
  last half-limit characters with a `...[truncated]...` marker between.
- These four tools return an error ("no MCP manifests/servers/resources") if the
  workspace has no resources and no servers, so they are useless-but-listed when
  nothing is configured. The system prompt gains a short MCP guidance section only
  when something is configured (it tells the model these tools exist, to treat
  summaries as hints, and to prefer reading a specific URI / calling a specific tool).

### 1.7 CLI and security notes
CLI: `mcp-status`, `mcp-resources [--query]`, `mcp-resource <uri>`,
`mcp-tools [--query] [--server]`, `mcp-call-tool <tool> [--arguments-json] [--server]`,
all with `--cwd`. `mcp-status` prints: manifest count, local resource count,
configured server count, per-transport counts, per-server local-resource counts, and
up to 10 servers as `name ; transport ; command`. `mcp_call_tool` is available to the
model with **no extra approval step**; the manifest decides which commands run.
Rebuild requirements (not present today): trust-gate manifest-defined commands (never
auto-run a server from a repository the operator has not approved), do not search
ancestor directories beyond the workspace root, and consider the model-callable
generic-call design (arbitrary tool with arbitrary arguments) as a prompt-injection
amplifier; prefer an explicit per-server allowlist.

---

## 2. "LSP" runtime (code intelligence)

**Governing public spec:** Language Server Protocol / JSON-RPC - but **this module
does not implement or speak LSP**. It is an in-process, heuristic indexer that mimics
LSP *operation names*. There is no language-server process, no JSON-RPC, no
initialization. Rating: DROP-CANDIDATE. A rebuild that wants real language servers
should specify a proper LSP client separately; do not port this.

What it does, for reference:
- Config files: `.sarina-lsp.json` and `.claude/lsp.json` in the workspace and each
  additional directory (no ancestor walk). Keys: `includeExtensions` (array, added to
  defaults), `excludeDirs` (array, added to defaults), `maxIndexedFiles` (positive
  int), `maxFileBytes` (positive int).
- Defaults: extensions `.py .pyi .js .jsx .ts .tsx .json`; ignored dirs `.git .hg .svn
  .port_sessions __pycache__ node_modules dist build .venv venv` (+ anything
  starting `.pytest_cache`); at most 500 files, files over 1,500,000 bytes skipped.
- Paths must resolve inside the workspace roots, else error.
- Indexing: Python files parsed with a real Python AST (classes, functions, methods,
  async functions, module/class-level variables, call edges); a syntax error becomes
  an `error` diagnostic with code `syntax-error` and falls back to regex. JSON files:
  top-level keys become property symbols; parse failure -> `json-error` diagnostic.
  JS/TS and everything else: line-anchored regexes for class/function/arrow-function/
  variable/interface/type declarations. Results are cached per file by
  (mtime, size).
- Tool `LSP` (capitalised name; the one non-snake-case tool besides `Agent`/`Skill`):
  params `operation` (enum: `goToDefinition findReferences hover documentSymbol
  workspaceSymbol goToImplementation prepareCallHierarchy incomingCalls
  outgoingCalls`), `file_path`, `line` (1-based), `character` (1-based) - all four
  required, `query`, `max_results` (1-200, default 50). Semantics are name-based:
  the "symbol at position" is the identifier under the cursor; definitions = every
  indexed symbol with that exact name (same file first, then nearest line);
  references = whole-word text matches across indexed files; `goToImplementation` is
  identical to definition; call hierarchy matches callee names textually (so
  same-named functions collide). A diagnostics view exists only via CLI (syntax
  errors only).
- CLI: `lsp-status`, `lsp-symbols`, `lsp-workspace-symbols`, `lsp-definition`,
  `lsp-references`, `lsp-hover`, `lsp-diagnostics`, `lsp-call-hierarchy`,
  `lsp-incoming-calls`, `lsp-outgoing-calls`.
- The tool errors with "no LSP runtime" if no indexable file exists.
Value for a security agent: low. `grep`/`read` cover the same ground more honestly.

---

## 3. Plugins

Rating: DROP-CANDIDATE (both parts). "Plugin" here means a **declarative JSON
manifest**, not code. A plugin cannot run anything.

### 3.1 Plugin manifests (`plugin_runtime.py`)
Discovery (each root = workspace, every ancestor directory, then additional
directories): `<root>/.codex-plugin/plugin.json`, `<root>/.sarina-plugin/plugin.json`,
and every `<root>/plugins/*/plugin.json` (sorted by directory name). Duplicates by
resolved path read once.

Manifest: JSON object with required non-empty string `name` (otherwise ignored) and
optional: `version`, `description`; `tools` (array of names, or object whose keys are
names - informational list only); `hooks`; `tool_aliases`/`toolAliases`;
`virtual_tools`/`virtualTools`/`runtimeTools`; `tool_hooks`/`toolHooks`;
`blocked_tools`/`blockedTools`. Both snake_case and camelCase spellings accepted
everywhere.

Behaviour contributed to a session:
- **Instruction summary.** One text block per plugin (name, description, tool names,
  hook names, alias names, virtual-tool names, blocked tool names) is surfaced in the
  prompt context, plus a short generic "plugins are advisory, verify before claiming"
  guidance section. A one-line-per-plugin summary (first 10 plugins) is used for status.
- **Lifecycle hooks.** `hooks` may be a list of names (informational) or an object
  with string values under `beforePrompt`, `afterTurn`, `onResume`, `beforePersist`,
  `beforeDelegate`, `afterDelegate` (snake_case also accepted). These are *text
  injections*, not code: `beforePrompt` text is prepended to the user prompt inside a
  system-reminder-style wrapper (together with a small plugin-runtime-state
  counters block); `onResume` likewise when a saved session is resumed;
  `beforePersist` appends a note to the transcript before saving; `afterTurn` adds
  an event to the run result; `beforeDelegate`/`afterDelegate` text is injected into
  a delegated child's prompt and the delegation result respectively.
- **Tool aliases** `[{name, base_tool|baseTool, description?}]`: registers a new tool
  name sharing the base tool's parameter schema and handler. Skipped if the base tool
  does not exist or the alias name is already taken (by a built-in or an earlier alias).
- **Virtual tools** `[{name, description, parameters?, metadata?,
  responseTemplate|response_template|response}]`: all of name, description and
  template required. The tool returns the template with `{arg}` placeholders replaced
  by the call's arguments (objects/arrays JSON-encoded, others stringified; unknown
  placeholders left literally as `{name}`; any formatting failure returns the
  template unchanged). Result metadata marks it a plugin virtual tool (plugin name,
  tool name, plus the declared metadata). Skipped if the name collides with an
  existing tool. Parameter schema defaults to an empty object schema.
- **Tool hooks** `{tool_name: "text"}` (string = after-result text) or `{tool_name:
  {beforeTool, afterResult, blockMessage}}`: `beforeTool` text is emitted as a
  preflight note (`<plugin>: <text>`) before that tool runs; `afterResult` text
  after it; `blockMessage` **blocks** the tool outright (tool result `ok=false`,
  content = that message).
- **Blocked tools** (list of names): any call to such a tool is blocked with a
  message naming the plugin. Applies by exact tool name, any plugin.
- **Session counters**, exported/restored with the saved session (only ints/floats/
  strings/bools and flat str->int maps survive): before-prompt calls, after-turn calls,
  resume calls, persist calls, delegate calls, tool attempts (total, per tool,
  blocked), tool results (total ok/failed, per tool), virtual-tool results.
Security note: a plugin found by the ancestor walk can block or reshape tools and
inject text into prompts. Treat plugin text as untrusted-ish configuration; a
rebuild that keeps plugins must apply the same trust gating as MCP.

### 3.2 Plugin cache summary (`agent_plugin_cache.py`)
Looks (workspace + ancestors, then additional dirs; first parseable, non-empty file
wins) for: `.port_sessions/plugin_cache.json`, `.port_sessions/plugins.json`,
`.claude/plugins/cache.json`, `.sarina/plugins/cache.json`, `plugins/cache.json`,
`.plugins/cache.json`. Accepts a list, or an object with `plugins` or `entries`
list, or an object whose values are objects (key = name). Entry = string (name) or
object with name (`name`|`plugin`|`id`, in that precedence), `source`
(`source`|`path`|`module`), `version`, `enabled` (default true). Output: a text block
(loaded-from path, entries, enabled/disabled counts, up to 12 entry lines then a
"plus N more" line, hard cap 4000 chars with `...`) injected into user context as
`pluginCache`. Purely informational; the file is produced by some other tool.

---

## 4. Skills (`bundled_skills.py` and the `Skill` tool)

Rating: DROP-CANDIDATE.
- Skill record: name, description, `when_to_use` hint, aliases, optional
  `allowed_tools` list, `user_invocable` flag, and a prompt generator taking the
  agent and the argument string.
- Four are bundled: `simplify` (collects `git diff HEAD`, falling back to `git diff`,
  30s timeout each, and asks the model to review for reuse/quality/efficiency and fix
  issues), `verify` (asks the model to work out how to verify recent changes and
  report PASS/FAIL/PARTIAL), `debug` (no model call: reports working dir, model,
  base URL, message and tool-result counts, token and cost totals, debug-log env
  status), `update-config` (alias `config-help`; explains settings-file locations,
  hook format, permission rules, env vars). `allowed_tools` is data only - it is not
  enforced anywhere.
- **No discovery of user/project skills** (no SKILL.md scanning). Only these four.
- A listing function renders `- name: description When to use: ...` lines (each capped
  at 250 chars, whole list at 8000 chars) headed by an "available skills" line; used
  by `/skills`.
- Tool `Skill`: params `skill` (required), `args`. Handled by the runtime, not a
  normal tool handler. Leading `/` stripped. Lookup: bundled skill by name/alias
  (case-insensitive) -> its generated prompt is returned as the tool result with
  metadata `should_query=true`; otherwise falls back to a slash command of that name
  (runs it, returns its output or prompt); otherwise `ok=false` "Unknown skill" with
  up to 30 available names.

---

## 5. Hook policy (only how plugins/hooks interact)

Rating: USEFUL (deny-list, trust, budgets), the message-injection parts DROP-CANDIDATE.
Files (workspace + ancestors + additional dirs): `.sarina-policy.json`,
`.codex-policy.json`, `.sarina-hooks.json`. Keys: `trusted` (bool), `managedSettings`
(object), `safeEnv` (list of env var names whose current values are passed to tool
subprocesses), `denyTools` (exact names, case-insensitive), `denyToolPrefixes`,
`hooks.{beforePrompt, afterTurn}` (string or list of strings), `hooks.{beforeTool,
afterTool}` (map tool-name or `*` -> string/list), `budget` (limits such as total/
input/output/reasoning tokens, total cost USD, tool calls, delegated tasks, model
calls, session turns; snake or camel case; booleans rejected).
Semantics: the last manifest that sets `trusted` wins; unset = trusted. A denied tool
call returns `ok=false` with error kind `permission_denied` and a message naming the
policy file. Deny checks run **before** execution for every tool including `Agent`
and `Skill`. Plugin blocks and policy blocks are independent; either blocks. Hook
messages are advisory text only. Tools denied by policy are labelled as blocked in
tool listings. A deny-list is the one piece here that is genuinely a security
control; if kept, it should not be bypassable by a plugin (it is not today).

---

## 6. Subagents

### 6.1 Agent definitions (`builtin_agents.py`)
An agent definition has: type name; "when to use" description (shown to the parent
model); system prompt; optional model alias (`sonnet`/`opus`/`haiku`/`inherit`/none);
tool allow-list (none = all) and deny-list; display colour; flags `background`,
`one_shot`, `omit_claude_md` (skip project-instruction discovery for the child);
`permission_mode`; `max_turns`; an optional critical reminder appended to the
system prompt in a reminder wrapper; plus metadata (source, filename, base_dir,
skills, memory, effort, initial prompt, isolation, hook names).
Only these fields actually change child behaviour today: model, tools,
disallowed_tools, system prompt, critical reminder, omit_claude_md, max_turns,
initial prompt. `background`, `one_shot`, `color`, `permission_mode`, `memory`,
`effort`, `skills`, `isolation`, `hook_names` are parsed, stored and displayed but
**not acted on**.

Two built-in deny sets:
- Always denied to every child: task output/stop tools, plan get/update/clear, and
  `ask_user_question` (children may not interact with the user or touch the plan).
- Read-only profiles additionally deny: `Agent`, `delegate_agent`, plan tools,
  `edit_file`, `write_file`, `notebook_edit`.
Children never receive `Agent`/`delegate_agent` at all (no recursive spawning).

Six built-ins (names are case-sensitive):
1. `general-purpose` - all tools; full-capability worker; replies with a concise
   report of what was done and key findings; told not to create files or docs unless
   needed.
2. `Explore` - read-only; model alias `haiku`; one-shot; skips project instructions.
   File/code search specialist; accepts a thoroughness hint (quick/medium/very
   thorough) from the caller; may use shell only for read-only listing/inspection;
   encouraged to issue parallel tool calls; reports findings as a plain message.
3. `Plan` - read-only; model `inherit`; one-shot; skips project instructions.
   Architecture/planning specialist; must end its answer with a short list of the
   most important files to create/modify and why.
4. `verification` - read-only on the project (scratch files in a temp dir allowed by
   its reminder); model `inherit`; flagged background, colour red. Must actually
   run builds/tests/linters rather than reason from reading, probe edge cases, report
   per step the command, observed output and PASS/FAIL/SKIP, and **end with exactly
   one verdict line**: `VERDICT: PASS`, `VERDICT: FAIL` or `VERDICT: PARTIAL`.
5. `statusline-setup` - tools: read_file, edit_file; model `sonnet`; edits the
   other product's status-line setting. DROP-CANDIDATE.
6. `claude-code-guide` - tools: glob_search, grep_search, read_file, web_fetch,
   web_search; model `haiku`; permission mode `dontAsk`; documentation Q&A for the
   other product. DROP-CANDIDATE.
The rebuild's analogous set should be tuned to Sarina (e.g. a read-only recon
profile and a read-only verification profile; the verdict-line convention is worth
keeping because it is machine-checkable). Model aliases map to nothing sensible for
local models: today an alias is substituted verbatim as the model name sent to the
backend, which is wrong unless the operator's backend serves models with those names.

Listing format given to the parent model in the system prompt: one line per agent:
`- <type>: <when to use> (Tools: <description>)` where the tools description is the
allow-list joined by commas, or "All tools except <sorted deny list>", or `*`. Only
the first 20 are listed, then a "plus N more" line. The Agent guidance section is
included only if `Agent` or `delegate_agent` is among enabled tools.

### 6.2 Agent registry (`agent_registry.py`)
Discovery: markdown files `*.md` (non-recursive, sorted by name) in
`~/.claude/agents/` (source label `userSettings`) and `<cwd>/.claude/agents/`
(source `projectSettings`). The `.claude` path is inherited from the upstream
product; a Sarina-native location (e.g. `.sarina/agents/`) would be reasonable, but
if both are supported keep the same precedence.
Precedence: built-in < user < project. Same type name in a higher source **shadows**
the lower (the winner is "active"; the others stay visible as "shadowed"). Active
list is ordered by source rank then name (case-insensitive). An invalid file never
stops loading; it is reported as a failed file (path + error).
File format: optional YAML-like front matter delimited by `---` lines at the start,
then the body = the system prompt. Front-matter parsing is deliberately naive
(one `key: value` per line; `#` comments and lines without `:` ignored; no nesting,
no multiline values): values are quoted strings (decoded), `true`/`false`, integers,
`[a, b]` lists (comma separated, quotes stripped), or raw text. CRLF normalised.
Keys: `name` (default = file name stem), `description` (aliases `whenToUse`,
`when_to_use`) **required**, `tools` (comma string or list; empty or `*` means all),
`disallowedTools`, `model`, `color`, `background`, `oneShot`, `omitClaudeMd`,
`permissionMode`/`permission_mode`, `maxTurns` (int), `criticalSystemReminder`,
`skills`, `memory`, `effort` (string or int), `initialPrompt`/`initial_prompt`,
`isolation`, `hooks` (list or map; names only). Body empty -> falls back to a
`prompt` front-matter key; still empty -> error "missing system prompt". Bool
parsing from strings accepts 1/true/yes/on.
Mutation (CLI `agents-create|update|delete`, slash command): creates/overwrites/
removes a file in the user or project agents dir (sources `project`, `user`,
`projectSettings`, `userSettings`, plus `auto` for update/delete = highest-precedence
editable definition). Create refuses to overwrite unless forced. Written files
include only the non-default fields, description double-quoted with escaping. A
"scaffold" create fills a generic description and prompt. Built-ins cannot be edited.
Reports: `agents [--all]` lists counts (active/all/shadowed/failed), source
precedence line, per-source counts and directories, then `- <type> [<source>] ;
tools=... ; model=<m|inherit>` lines; `agents <type>` shows the full definition
(source, model, tools, permission mode, max turns, flags, base dir, file, colour,
memory, effort, isolation, skills, hooks, initial prompt, when-to-use, system prompt).
Rating of mutation CLI: DROP-CANDIDATE; of load path: USEFUL.

### 6.3 Agent manager (`agent_manager.py`)
In-memory only (not persisted), per top-level agent. Tracks:
- **Agent record**: id `agent_<n>` (counter from 1), prompt, parent id, group id,
  child index, label, resumed-from session id, session id/path, status (`running`
  until finished, then `completed`), turns, tool calls, stop reason.
- **Group record**: id `group_<n>`, label, parent id, ordered child ids, strategy
  (`serial` default), status, completed/failed child counts, batch count, max batch
  size, dependency skips.
Operations: start agent (optionally registering in a group), start group, register
a child in a group (idempotent; unknown ids are no-ops), finish agent (sets
`completed` regardless of success; session info and stats recorded), finish group
(status + stats), children-of, group children (sorted by child index, unindexed
last, then id), group summary (counts, resumed count, histogram of stop reasons,
`n/a` for none), completed records, and summary lines for `/status`-style reports
(totals; first 8 agents; first 6 groups; "plus N more" lines). Records are
immutable values replaced on update. Rating USEFUL only as a delegation helper.

### 6.4 The `Agent` tool (and legacy `delegate_agent`)
Both tool names are handled by the runtime (not by the generic tool executor) and
share one implementation; `delegate_agent` is the older name, kept for
compatibility, and results always report the canonical action `delegate_agent` even
when invoked as `Agent`. Plugin tool-block and policy-deny checks apply first.
Parameters (JSON schema): `description` (3-5 word label) and `prompt` required for
`Agent`; `subagent_type` (agent definition name; unknown -> silently falls back to
`general-purpose`), `model` (enum sonnet/opus/haiku in the schema; any non-empty
string is actually accepted and substituted as the model name), `run_in_background`
and `isolation` (enum `worktree`) - **accepted by the schema but ignored**; no
background execution and no worktree isolation happen via this tool - `subtasks`
(array, each a string prompt or an object `{prompt, label?, max_turns?,
resume_session_id|session_id?, depends_on?[labels]}`), `resume_session_id`/
`session_id`, `max_turns` (1-20), `allow_write`, `allow_shell`,
`include_parent_context` (default true), `continue_on_error` (default true),
`max_failures` (0-20), `strategy`. `delegate_agent` has the same except `description`/
`subagent_type`/`model`/`run_in_background`/`isolation`, with only `prompt` as the
(optional) primary field.

### 6.5 Delegation semantics (the real behaviour)
- **Subtasks:** at most **8** are used (extras dropped). Empty-prompt entries dropped.
  A plain `prompt` is used only if no subtasks were given. Default labels
  `subtask_<index>`. No prompt and no subtasks -> `ok=false` error.
- **Execution:** each subtask runs a fresh child agent, **synchronously and one after
  another** - there is no real parallelism even under the "topological" strategy.
- **Strategy:** `serial` (default) = one batch in given order. Names `graph`,
  `topological`, `dependency_graph`, `parallel`, `parallel_batches` all mean
  `topological` = subtasks grouped into successive batches by dependency readiness
  (a task is ready when all `depends_on` labels were scheduled in earlier batches; a
  task depending on an unknown label, or a cycle, ends up in a final leftover batch).
  Within a batch, run order is by original index. Batches are reporting structure
  only.
- **Dependencies:** before running a subtask, any `depends_on` label not in the set
  of *successfully completed* labels causes the subtask to be **skipped** (stop
  reason `skipped_dependency` if one of them failed, else `pending_dependency`),
  counted as failed and as a dependency skip. Under `serial`, order therefore matters.
- **Failure:** a child "fails" if its stop reason is `backend_error` or
  `budget_exceeded`, if its resume session cannot be loaded (`resume_load_error`),
  or if skipped as above. After each failure: stop if `failed > max_failures` (when
  given) or if `continue_on_error` is false; otherwise continue.
- **Resume:** a subtask may resume a saved child session by id; the child continues
  that transcript with the new prompt.
- **Child configuration:** model = explicit `model` arg > definition model (unless
  `inherit`) > parent's. Max turns = explicit arg > definition's > `min(parent
  max_turns, 6)`, per-subtask `max_turns` overriding. Tool set = parent's registry minus
  `Agent`/`delegate_agent`, intersected with the definition's allow-list, minus its
  deny-list, minus the always-denied set. Permissions: read-only definitions (deny
  `edit_file` or `write_file`) -> no file writes, no destructive shell, shell only if
  the parent allows it; other definitions -> file write and shell are allowed only if
  the **parent allows them AND the call passed `allow_write`/`allow_shell` true**
  (both default false); destructive shell is never allowed to children. The
  definition's system prompt replaces the parent's custom/override prompt;
  the critical reminder, if any, is appended in a reminder wrapper; project
  instruction discovery is skipped if `omit_claude_md`.
- **Prompt assembly per child:** initial prompt (if the definition has one and the
  subtask is not a resume) + `\n\n` + the subtask prompt; then plugin
  `beforeDelegate` text (reminder-wrapped), then (if `include_parent_context`) up to
  the **last 4** previous sibling results as `label: preview` lines in a reminder
  block. Previews are 220 characters.
- **Bookkeeping:** each child is registered with the agent manager (parent id, group
  for multi-subtask calls, index, label). A group is created only when more than one
  subtask is given. Delegated task units count against the session budget
  (`max_delegated_tasks`).
- **Result:** `ok=true` text containing: a one-line completion statement; for groups,
  `group_id`, `group_status` (`completed`/`partial`/`failed`), resumed count,
  strategy, batch count, dependency skips; per-batch status lines; per-child blocks
  (label, batch index, session id, turns, tool calls, stop reason, resume info,
  dependencies, output preview); plugin before/after delegate notes; then the **final
  child's full output** under a "final delegated output" heading. Metadata carries
  structured copies of all of this (child session ids, per-child results,
  counts). `ok` is true even if some children failed; failure is conveyed in the
  text/metadata. Child sessions are saved like any session and can be resumed later.
- **Not implemented here:** no cross-agent messaging, no shared task list for
  children (task/plan tools are denied), no background completion notifications, no
  token/cost roll-up other than the standard session accounting.

---

## 7. Teams (`team_runtime.py`)

Rating: DROP-CANDIDATE. A named list of member strings plus an append-only message
log. Nothing delivers messages or launches member agents; "members" are free text.
- Manifests (workspace and additional dirs only, no ancestor walk):
  `.sarina-teams.json`, `.sarina-team.json`, `.claude/teams.json`; shape
  `{"teams":[{name, description?, members?[], metadata?, created_at?}]}`.
- State file: `.port_sessions/team_runtime.json` = `{teams:[...], messages:[...]}`.
  **Once the state file contains a `teams` list, manifests are ignored entirely**
  (manifest teams only seed the first run); messages come only from state.
- Message: `message_id` (`msg_` + 10 hex chars), team name, sender, text, optional
  recipient, metadata, created_at. Sending to an unknown team errors.
- Teams are kept sorted by name (case-insensitive); names unique case-insensitively;
  delete also deletes that team's messages. Mutations persist immediately.
- Tools (all snake_case): `team_list(query?, max_teams<=200 default 50)`,
  `team_get(team_name)`, `team_create(team_name, description?, members?[], metadata?)`
  (requires write permission; error if exists), `team_delete(team_name)` (write
  permission), `send_message(team_name, message, sender default "agent", recipient?,
  metadata?)` (write permission), `team_messages(team_name?, limit<=200 default 20)`.
  `team_get` shows name, description, member list and the last 5 messages.
- CLI: `team-status`, `team-list [--query]`, `team-get`, `team-create [--description]
  [--member]...`, `team-delete`, `team-messages [--team-name]`.

---

## 8. Workflows (`workflow_runtime.py`)

Rating: DROP-CANDIDATE. Definitions are named lists of text steps; "running" one
**executes nothing**.
- Manifests (workspace + additional dirs, no ancestor walk): `.sarina-workflows.json`,
  `.sarina-workflow.json`. Shape: `{"workflows":[...]}` and/or `{"workflow":{...}}`;
  each `{name (required), description?, prompt?, steps?[], metadata?}`; a step is a
  string (becomes `{title}`) or an object with `title|name`, `detail|command|prompt`.
- Run: `workflow_run(workflow_name, arguments?{})` (CLI `workflow-run <name>
  --arguments-json`): looks up the workflow, substitutes `{key}` placeholders in step
  text and prompt using the arguments (plain string replace; unknown placeholders
  left), appends a history record (`run_id` = `workflow_run_` + 10 hex, status
  always `recorded`, created_at, arguments, summary = first rendered step or the
  description or a "recorded without steps" note), persists to
  `.port_sessions/workflow_runtime.json` (last 128 records kept), and returns a text
  report (run id, status, arguments as JSON, resolved steps, formatted prompt).
  The model is expected to act on the report itself.
- Other tools: `workflow_list(query?, max_workflows<=200 default 50)`,
  `workflow_get(workflow_name)`. Unknown name -> error. CLI: `workflow-list`,
  `workflow-get`, `workflow-run`.

---

## 9. Remote (`remote_runtime.py`) and remote triggers (`remote_trigger_runtime.py`)

Rating: DROP-CANDIDATE for both. Both are bookkeeping with no network or process
behaviour. (Sarina's genuine remote integrations - Chymaera console TCP client,
fleet - are separate, original modules and are not part of this section.)

### 9.1 Remote profiles and connection state
- Manifests: `.sarina-remote.json`, `.remote.json`, `.codex-remote.json`,
  `remote.json` in the workspace, every ancestor, and additional dirs. Shape:
  `{"profiles":[...]}` and/or `{"remotes":[...]}`, or a single top-level profile
  object. Profile: `name` (required), `target` (required), `mode`, `description`,
  `workspaceCwd`/`workspace_cwd`, `sessionUrl`/`session_url`, `env` (scalar values
  stringified), `metadata`. Duplicate names (case-insensitive) within one list dropped.
- Modes: `remote`, `ssh`, `teleport`, `direct-connect`, `deep-link`. Normalisation:
  lowercase, `_`->`-`, `direct`->`direct-connect`, `deeplink`->`deep-link`, anything
  unknown -> `remote`.
- `connect(target, mode?)`: matches a profile by name or target (case-insensitive);
  if found, the connection takes its mode/target/workspace/session URL/metadata;
  otherwise an **ephemeral** connection is recorded with the given target and mode
  (default `remote`, metadata `ephemeral: true`). Persists
  `.port_sessions/remote_runtime.json` = `{active_connection, history}` (history keeps
  last 40 connect/disconnect events). **Nothing is contacted or launched.** Output is
  a `key=value` report (mode, connected, detail, target, profile, workspace_cwd,
  session_url, source_manifest, manifest_count, profile_count, metadata.*).
  `disconnect(reason?)` clears it.
- Tools: `remote_status(profile?)`, `remote_list_profiles(query?, mode?,
  max_profiles<=200 default 20)`, `remote_connect(target, mode?)`,
  `remote_disconnect(reason?)`. Note: **`remote_connect` is model-callable with no
  confirmation**. Profile `env` values are printed in profile detail output.
- CLI: `remote-mode`, `ssh-mode`, `teleport-mode`, `direct-connect-mode`,
  `deep-link-mode` (each = connect with that mode), `remote-status`,
  `remote-profiles [--query]`, `remote-disconnect`; slash commands `/remote`,
  `/ssh`, `/teleport`, `/direct-connect`, `/deep-link`, `/disconnect`, `/remotes`,
  plus read-only `/remote-env`, `/bridge`, `/remote-setup` status reports.

### 9.2 Remote triggers
- Manifests (workspace + additional dirs): `.sarina-remote-triggers.json`,
  `.sarina-triggers.json`; shape `{"triggers":[{trigger_id|id|name (one required),
  name?, description?, schedule?, workflow?, remote_target?, body?{}, metadata?{}}]}`.
  `schedule` is an uninterpreted string (no scheduler exists).
- State `.port_sessions/remote_trigger_runtime.json` = `{custom_triggers[],
  history[]}` (history last 128). Triggers created at runtime are saved as
  `source: local_state` and override manifest triggers with the same id.
- `run`: merges the trigger's stored `body` with the call's body (call wins), appends
  a run record (`trigger_run_` + 10 hex id, status always `queued`) and reports it.
  **Nothing consumes the queue.**
- Tool `remote_trigger(action, trigger_id?, body?, query?, max_triggers?)` with action
  one of `list`, `get`, `create`, `update` (shallow-merge of provided fields over the
  stored definition), `run`; invalid action -> error. `create`/`update`/`run` have no
  write-permission check. CLI: `trigger-list [--query]`, `trigger-get`,
  `trigger-create --body-json`, `trigger-update --body-json`, `trigger-run
  [--body-json]`. Slash commands `/triggers`, `/trigger`.
- Bad `body`/`metadata` types (non-object) -> error on create/update.

---

## 10. Account (`account_runtime.py`)

Rating: DROP-CANDIDATE. This is a local label for "which identity/profile is active",
unrelated to the model backend's real credentials (which come from `OPENAI_*`
environment variables elsewhere).
- Manifests (workspace and additional dirs only): `.sarina-account.json`,
  `.claude/account.json`, `.claude/auth.json`. Shape: `{"profiles":[...]}` or a single
  profile object. Profile: `provider` (required), `identity` (required; aliases
  `email`, `user`), `name` (alias `profile`; defaults to identity), `description`,
  `org`, `authMode`/`auth_mode`, `apiBase`/`api_base`, `metadata`.
- Credential detection: reports **which** of `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`,
  `OPENROUTER_API_KEY`, `LITELLM_MASTER_KEY` are set and non-empty (names only, never
  values).
- `login(target, provider?, auth_mode?)`: if target matches a profile (name or
  identity) it becomes the active session; else an ephemeral session is created
  (provider default `custom`, auth mode default `token`). Persists
  `.port_sessions/account_runtime.json` = `{active_session, history(last 100)}` with
  sorted keys and trailing newline. **No credential is requested, validated, or
  stored; no network.** `logout(reason?)` clears it.
- Tools: `account_status(profile?)`, `account_list_profiles(query?,
  max_profiles<=200 default 20)`, `account_login(target, provider?, auth_mode?)`,
  `account_logout(reason?)`. CLI: `account-status`, `account-profiles [--query]`,
  `account-login <target> [--provider] [--auth-mode]`, `account-logout`; slash
  `/account`, `/login`, `/logout`.
- Smell for the rebuild: giving the model the ability to "log in" as an identity is
  meaningless and, if the field is ever consulted for authorisation, dangerous.

---

## 11. Cross-cutting behaviour worth preserving (if any of this is kept)

1. **Fail soft at load, fail loud at call.** Bad manifests never break startup;
   calls against a missing/failed target return a clear error to the model.
2. **Name collision rules:** built-in tool names always win over plugin aliases and
   virtual tools; earlier plugin wins over later; project agent definition wins over
   user wins over built-in.
3. **Children are weaker than parents:** a child can never gain a permission or tool
   the parent lacks, cannot spawn further agents, cannot ask the user, and cannot
   edit the plan.
4. **Everything the model is told about these extension points is advisory text**
   (guidance sections mention they are hints and must be verified); none of it is an
   authorisation mechanism.
5. **Tool-name styles:** most tools are snake_case (`mcp_call_tool`, `team_create`);
   `Agent`, `Skill`, `LSP` are capitalised. A rebuild should pick one scheme; any
   renaming must be applied in the allow-list/deny-list handling of the service API
   and hook policy too.
6. **Security posture for a security agent:** manifests discovered in ancestor
   directories (MCP, plugin, remote, hook policy) are a repository-controlled
   configuration channel. Anything in this area that can spawn a process (only MCP)
   or deny/permit tools (plugins, hook policy) must require an explicit operator
   trust decision per workspace before it takes effect. Today none of them do.

## 12. Suggested minimal rebuild scope

1. MCP stdio client (sections 1.1-1.6), with a per-workspace trust gate and a
   per-server tool allowlist; keep the four-generic-tool surface or replace it with
   per-tool registration using a documented prefix (decide explicitly - today there is
   no prefixing).
2. Subagent definitions + registry load path (6.1, 6.2), `Agent` tool with
   serial execution, read-only profiles, permission narrowing, result format (6.4,
   6.5). Skip dependency graphs and groups unless a concrete use appears.
3. Hook-policy deny-list/trust/budget (section 5, minimal).
Defer everything else; sections 2-4 and 7-10 should not be rebuilt without a new,
real requirement.

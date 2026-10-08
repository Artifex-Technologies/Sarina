# Spec 01 - Core agent loop and sessions

Clean-room input for the Go rewrite. Read `docs/spec/README.md` and
`docs/PROVENANCE.md` first. This document describes observable behaviour only.
Where an existing behaviour is a bug or an accident it is marked
**DO NOT REPLICATE** and the intended behaviour is stated. Wording of prompts,
reminders and help text is deliberately NOT given; the implementer writes fresh
text that conveys the stated meaning. Exact strings are given only where they
are a wire format, a file key, an event/stop-reason identifier, or something
another repo reads.

Terminology: "the model" is an OpenAI-compatible chat-completions server
(Ollama, vLLM, llama.cpp server, LM Studio, etc.). "Tool" is a model-callable
function (tool inventory and schemas are in another spec; this one only covers
how the loop dispatches them). "Workspace" is the working directory the agent
was started in.

---

## 0. Module ratings

| Existing module | Rating | Why |
|---|---|---|
| `agent_runtime.py` (turn loop, budgets, persistence, resume) | ESSENTIAL | It is the agent. About 40% of its 4.7k lines is `render_*_report` helpers and delegation/plugin plumbing covered by other specs; only the loop, budgets, compaction orchestration, persistence, resume and escalation belong here. |
| `agent_types.py` (config + result dataclasses) | ESSENTIAL | The data contracts. |
| `agent_session.py` (message model, transcript, secret redaction) | ESSENTIAL | Message/transcript model. The mutation-history/lineage metadata is USEFUL-but-heavy, see 3.4. |
| `session_store.py` (on-disk JSON) | ESSENTIAL | Persistence and the format Sentry reads. The legacy flat `StoredSession` half is DROP-CANDIDATE (4.5). |
| `openai_compat.py` | ESSENTIAL | The only model transport. |
| `token_budget.py` | ESSENTIAL | Pre-flight context-limit arithmetic. |
| `agent_context_usage.py` | ESSENTIAL (counting and window inference) / DROP-CANDIDATE (the markdown report formatter, which only serves a `/context` slash command) | Token accounting is needed by the budget and compaction. |
| `tokenizer_runtime.py` | USEFUL | Accurate counting; a plain length/4 estimate works. In Go, prefer the heuristic plus an optional backend `/tokenize` style endpoint rather than porting the tiktoken/transformers probing. |
| `compact.py` (LLM summarisation compaction, prompt, PTL retry) | USEFUL | Long sessions on small local contexts need it; the heuristic boundary compaction (7.4) is the ESSENTIAL minimum. |
| `microcompact.py` | DROP-CANDIDATE | Never fires: it keys on a message timestamp that nothing ever records, and the "prompt cache expired" rationale does not apply to local servers. |
| `session_memory_compact.py` | DROP-CANDIDATE | Never fires: depends on a `~/.claude/session-memory/session.md` file nothing writes and on a "last summarised message id" nothing sets. Pure parity feature. |
| `context.py` | DROP-CANDIDATE | Counts files in a `src/`+`archive/` tree of the upstream port; not part of the agent. |
| `agent_context.py` (context snapshot, CLAUDE.md discovery, git status) | ESSENTIAL | Context assembly. The many `*Runtime` summary blocks it pulls in belong to other specs. |
| `agent_prompting.py` (system prompt assembly) | ESSENTIAL (the mechanism) / the existing prose is DROP, rewrite from scratch | Prompt text is derived from the proprietary original; write new text. |
| `config_runtime.py` (`.claude/settings.json` store) | USEFUL | Generic layered settings store plus config tools. Nothing in the core loop reads values from it. |
| `system_resources.py` | USEFUL | Resource-pressure trigger for escalation; original code. |
| `escalation_runtime.py` | USEFUL | Hand-off to the real `claude` CLI; original code, opt-in, off by default. |
| `memory_runtime.py` / `memory_review.py` / `session_search_fts.py` / `sentry_governance.py` / `memory_security.py` | ESSENTIAL for Sarina's differentiators (original code) | Hook points are documented in section 13; their own behaviour is out of scope here. |

---

## 1. Configuration

### 1.1 Model configuration (`ModelConfig`)

| Field | Default | Notes |
|---|---|---|
| model | CLI default `Qwen/Qwen3-Coder-30B-A3B-Instruct` (env `OPENAI_MODEL`) | Sent verbatim as `model`. |
| base_url | `http://127.0.0.1:8000/v1` (env `OPENAI_BASE_URL`) | Trailing slash tolerated; the client appends `/chat/completions`. |
| api_key | `local-token` (env `OPENAI_API_KEY`) | Sent as `Authorization: Bearer <key>` always, even for servers that ignore it. |
| temperature | 0.0 | Always sent. |
| timeout_seconds | 120.0 | Per HTTP request (connect + each read), also the whole non-stream response. |
| pricing | all four rates 0.0 | USD per million tokens: input, output, cache-creation input, cache-read input. Cost = sum(tokens/1e6 * rate). With zero rates cost is always 0 and the cost budget never trips. |

CLI flags: `--model`, `--base-url`, `--api-key`, `--temperature`,
`--timeout-seconds`, `--input-cost-per-million`, `--output-cost-per-million`.
(Cache-token rates have no CLI flag; they exist only in the persisted file.)

### 1.2 Runtime configuration (`AgentRuntimeConfig`)

| Field | Default | CLI flag |
|---|---|---|
| cwd | required, resolved to absolute | `--cwd` (default `.`) |
| max_turns | 12 | `--max-turns` |
| command_timeout_seconds | 30.0 | (tool concern) |
| max_output_chars | 12000 | (tool concern; tool output is truncated head+tail with a marker when over) |
| stream_model_responses | false | `--stream` |
| auto_snip_threshold_tokens | none | `--auto-snip-threshold` |
| auto_compact_threshold_tokens | none | `--auto-compact-threshold` |
| compact_preserve_messages | 4 (CLI clamps negatives to 0) | `--compact-preserve-messages` |
| permissions | all false | `--allow-write`, `--allow-shell`, `--unsafe` (destructive shell) |
| additional_working_directories | empty | `--add-dir` (repeatable) |
| disable_claude_md_discovery | false | `--disable-claude-md` |
| budget_config | all unset | see 5.3 |
| escalation_config | disabled | see 11 |
| output_schema | none | `--response-schema-file`, `--response-schema-name`, `--response-schema-strict` |
| session_directory | `<process cwd>/.port_sessions/agent` resolved | no flag |
| scratchpad_root | `<process cwd>/.port_sessions/scratchpad` resolved | `--scratchpad-root` |

Note `session_directory` is relative to the PROCESS cwd at start-up, not the
agent's `--cwd`. Several other components (memory under `<workspace>/.port_sessions/memory/`, sentry, background runner) assume `.port_sessions` is under the workspace. **Intended behaviour for the rewrite:** anchor `.port_sessions` at the workspace cwd, and make the session directory overridable.

Prompt-related flags: `--system-prompt` (replace the default prompt with this
single part), `--append-system-prompt` (add a final part), `--override-system-prompt`
(wins over everything, becomes the only part). See 8.1.

### 1.3 Environment variables read by the core

| Variable | Used for |
|---|---|
| `OPENAI_BASE_URL`, `OPENAI_API_KEY`, `OPENAI_MODEL` | CLI defaults for the three backend settings. No other part of the core reads them; in-process callers pass a config. |
| `SHELL` | Reported in the environment context (basename shown; "unknown" if unset). |
| `HOME` (via home-directory lookup) | Global CLAUDE.md, user-scope memory, legacy session-memory path. |
| `SARINA_TOKENIZER_PATH`, `SARINA_TOKENIZER_MODEL`, `SARINA_TOKENIZER_TRUST_REMOTE_CODE` | Tokenizer selection (9.2). |
| `CLAUDE_CODE_CLI_PATH` | Default for `--claude-code-cli-path` (escalation CLI binary, default `claude`). |
| `SENTRY_REVIEW_URL` | Opt-in Sentry governance endpoint (13.4). |
| `DISABLE_CLAUDE_CODE_SM_COMPACT` | Disables the dead session-memory compaction; irrelevant if that module is dropped. |

Any new Sarina-specific variable the rewrite adds should use the `SARINA_` prefix. **Recommendation (new, not existing):** add `SARINA_CONTEXT_WINDOW` (integer tokens) because context-window inference by model name (9.3) is wrong for any local server whose context length is set at launch.

### 1.4 `.claude/settings.json` layered store (`ConfigRuntime`)

Sources, in precedence order lowest to highest, all relative to the workspace:
`.sarina-config.json` (legacy), `.codex-config.json` (legacy),
`.claude/settings.json` ("project"), `.claude/settings.local.json` ("local").
Each must be a JSON object; unreadable or non-object files are silently skipped.
Effective settings = recursive merge (objects merged key-wise, any other value
overwritten by the higher-precedence source). Key paths are dot-separated;
empty segments ignored.

Operations used by config tools: list keys (flattened, sorted, with optional prefix
and limit), get value (error if the path is absent), set value (default target
"local"; accepted aliases for project: `project`, `project-settings`, `settings`;
local: `local`, `local-settings`; legacy: `legacy`, `legacy-project`; names are
case-insensitive with `_` treated as `-`). A write rewrites the whole file as
JSON with sorted keys, 2-space indent, ASCII-escaped, trailing newline, creates
parent directories, and reports before/after SHA-256 and a 220-char preview.
Rated USEFUL: no part of the loop depends on it other than injecting a one-block
summary into the context (source names, paths, key counts).

---

## 2. Data model

### 2.1 Usage

Five non-negative integers: `input_tokens`, `output_tokens`,
`cache_creation_input_tokens`, `cache_read_input_tokens`, `reasoning_tokens`.
`total_tokens` = input + output + cache_creation + cache_read (reasoning is NOT
added; it is assumed to be a subset of output). Usage values add component-wise.
Serialised form is those five keys plus a derived `total_tokens` key.

### 2.2 Tool call and assistant turn

ToolCall: `id` (string), `name` (string), `arguments` (JSON object).
AssistantTurn: text `content`, ordered tool calls, `finish_reason` (string or null),
the raw provider message, and `usage`.

### 2.3 Run result (`AgentRunResult`)

| Field | Meaning |
|---|---|
| final_output | Text for the human: the last assistant text, or a stop explanation. |
| turns | Number of loop turns used in THIS call (not cumulative). |
| tool_calls | Tool calls counted. On a resumed run this is cumulative from the stored session (see quirk 5.9). |
| transcript | List of transcript entries (2.5) for the whole session. |
| events | Ordered list of event dicts for this run (section 6). |
| usage | Cumulative usage including everything restored from a resumed session. |
| total_cost_usd | Cumulative cost estimate. |
| stop_reason | See 5.2. |
| file_history | List of dict history entries (5.10). |
| session_id, session_path | Identity and file written (null if no session was created, e.g. a purely local slash command). |
| scratchpad_directory | Per-session temp directory path (5.1). |

### 2.4 Message (`AgentMessage`) fields

`role` (`system`/`user`/`assistant`/`tool`), `content` (string, never null),
`name` (tool name for tool messages), `tool_call_id`, `tool_calls` (list of
OpenAI-style call objects, assistant only), `blocks` (structured mirror, 2.6),
`message_id`, `state` (`final` default, `streaming`, `tombstoned`), `stop_reason`,
`usage`, `metadata` (free dict).

### 2.5 What is sent to the model vs what is stored

Sent to the backend per message: exactly `role`, `content`, and when present
`name`, `tool_call_id`, `tool_calls`. Nothing else (no blocks, ids, metadata).
`tool_calls` entries use the OpenAI shape: `{"id", "type":"function",
"function":{"name","arguments":<JSON string>}}`. Assistant tool-call arguments are
re-serialised from the parsed object as a compact ASCII-escaped JSON string
in the non-streaming path; in the streaming path the accumulated raw string is kept.

The tool-role message body is a JSON document (pretty-printed, 2-space indent,
ASCII-escaped) with keys `tool` (name), `ok` (bool), `content` (string), and
`metadata` (object, only if non-empty). The model sees this JSON as the tool
result text.

Stored (transcript) entry = the sent shape plus, when applicable: `blocks`,
`message_id`, `state` (only if not `final`), `stop_reason`, `usage` (only if its
total is non-zero), `metadata` (only if non-empty).

### 2.6 Blocks

Structured mirror of a message, derived (not authoritative):
- text content -> `{"type":"text","text":...}` (omitted when content empty)
- assistant tool calls -> `{"type":"tool_call","id","name","arguments"}` where
  arguments is the raw JSON string (empty string default)
- tool result -> one `{"type":"tool_result","name","tool_call_id","text"}` (omitted when empty)
- user/system -> one text block.
On load, `blocks` is taken from the file if it is a list of objects; it can be
re-derived. Rated DROP-CANDIDATE for the rewrite's own use (nothing reads blocks
back), kept only if a UI wants them; if kept, keep these exact shapes.

### 2.7 Message ids

- system message: `system_0`
- user-context reminder: `user_context_0`
- initial user prompt (when passed at creation): `user_0`
- assistant: `assistant_<index in message list>` (index at creation time)
- tool: `tool_<index>`
- later user messages: `user_<index>` unless an id is supplied
- injected reminder messages use descriptive ids such as `continuation_<turn>`,
  `file_history_replay_<count>`, `compaction_replay_<boundaries>_<snipped>`,
  `compact_boundary`, `compact_summary`, `escalation_<agent id>`.
Ids are unique labels, not guaranteed unique across compactions (compaction
re-uses `compact_boundary`/`compact_summary` ids in the LLM path).

### 2.8 Metadata keys the system relies on

| Key | Where | Meaning |
|---|---|---|
| `kind` | any | Role of an injected message: `compact_boundary`, `compact_summary`, `snipped_message`, `continuation_request`, `file_history_replay`, `compaction_replay`, `plugin_*`, `ptl_retry_marker`. Compaction/snip logic keys off this. |
| `tool_name`, `tool_call_id` | tool | Mirrors fields. |
| `phase` | tool | `starting` while running, `completed` after. |
| `lineage_id`, `revision`, `revision_count`, `message_role` | any | Lineage tracking (3.4). |
| `mutations`, `mutation_count`, `mutation_totals`, `last_mutation_kind`, `last_mutation_serial`, `max_mutation_serial` | any | Mutation history (3.4). |
| `stream_preview` | tool | Streamed partial text if the final text replaced it. |
| `original_token_estimate`, `replacement_token_estimate`, `snipped_*` | tombstones | Provenance of a snip (7.3). |

Plus whatever the tool result's own metadata contains (merged into the tool
message metadata; contains things like `path`, `command`, `action`,
`error_kind`, hash fields), because file history is derived from it.

---

## 3. Session state behaviour

### 3.1 Creation

A new session's messages are, in order:
1. system message: all system-prompt parts joined with a blank line; if
   system context is non-empty a last part is appended containing one
   `key: value` line per non-empty entry (see 8.3).
2. if user context is non-empty: a `user`-role reminder message (id
   `user_context_0`) wrapping the context (8.2).
3. the user prompt (id `user_0`) if one was supplied at creation.
In the real run path the session is created with NO prompt and the prompt is
appended afterwards as `user_<index>` (see quirk 8.6).

### 3.2 Streaming assembly (when `stream_model_responses` is on)

An assistant message is created empty with state `streaming`, mutated by
appending text deltas and merging tool-call fragments (3.3), then finalised
(state `final`, stop_reason = finish reason, usage = accumulated streaming
usage). Tool messages are likewise created in state `streaming` with
`phase=starting`, text deltas appended as the tool runs, and finalised with the
complete serialised result (3.5).

### 3.3 Tool-call fragment merge (streaming)

Fragments are addressed by integer `index` (missing or non-int treated as 0). The
list is grown with placeholder calls (`id` null, `type` function, empty name and
arguments) up to the index. A fragment with a non-empty `id` sets the id; a
non-empty name sets the name (assignment, not append); an arguments fragment is
APPENDED to the accumulated argument string. After the stream ends the
accumulated calls are parsed (see 7.5 for failure handling).

### 3.4 Mutation history and lineage

Every in-place change to a message (delta append, tool-call delta, finalise, tool
delta, tool finalise-with-replacement, snip tombstone) records a mutation entry
in `metadata`: `kind`, `previous_state`, `previous_stop_reason`,
`previous_content_length`, `previous_content_preview` (whitespace-collapsed,
120 chars max with `...`, or the literal `(empty)`), and a monotonically
increasing `serial` (session-wide counter, restored on load as the max
`last_mutation_serial` seen). Only the last 8 entries are kept in `mutations`;
`mutation_count` is the length of that kept list; `mutation_totals` counts
all-time per kind; `max_mutation_serial` is the high-water mark. `lineage_id`
defaults to the message id (or `<role>_lineage`), `revision` starts at 0 and +1
for each mutation, `revision_count` = max(previous, revision+1).
Observable requirement from tests: after streaming "Streaming works." as two
deltas, the single assistant message has `mutation_totals.assistant_delta_append`
>= 2 and `assistant_finalize` == 1; with tool-call fragments,
`assistant_tool_call_delta` >= 2.
Rating: USEFUL for debugging, DROP-CANDIDATE for correctness. A rewrite may
keep only `lineage_id`/`revision` plus the totals. Compaction boundary metadata
(7.4) aggregates these, so if lineage is dropped drop those aggregates too.

### 3.5 Secret redaction at tool-result ingestion (security feature, KEEP)

Every tool-role message content, including streamed deltas (re-applied to the
whole accumulated text each time), the final content, and any content update to
a tool message, is passed through a redactor BEFORE it is stored or sent to the
model. Matches are replaced by `[REDACTED:<kind>]`. Token families to detect
(write fresh patterns; thresholds are approximate and the point is "high
confidence real credential"):
- Anthropic/OpenAI-style `sk-<ant|proj|or|live|test>-` followed by >= 8 token chars (kind = the middle word)
- Stripe-style `sk_|rk_|pk_` + `live|test` + `_` + >= 16 alphanumerics (kind `stripe`)
- GitHub `ghp_` + >= 20 alphanumerics (`github`)
- AWS access key id `AKIA` + >= 16 uppercase alphanumerics (`aws`)
- Slack bot `xoxb-` + >= 20 chars (`slack`)
- Google API key `AIza` + 35 url-safe chars (`google`)
- JWT: three dot-separated base64url segments, the first two starting `eyJ` (`jwt`)
- PEM/OpenSSH private-key header lines (`pem`)
Redaction applies to tool results only, not to user text, assistant text or the
system prompt. (Memory writes have a separate, stricter scan: section 13.)

### 3.6 Reload from disk

Messages loaded from a session file keep only entries that are objects;
unknown roles are accepted; missing `content` becomes `""`; `state` defaults to
`final`; `usage` missing -> zeros. `blocks`/`metadata` are optional.

---

## 4. On-disk session format (READ BY CHYMAERA_SENTRY - keep exact)

### 4.1 Location and naming

`<session_directory>/<session_id>.json`, UTF-8, `json.dumps` with 2-space
indent (key order as listed in 4.2; non-ASCII is escaped as `\uXXXX`, i.e. ASCII-
only output, because no `ensure_ascii=False` is used). `session_id` is a
32-character lowercase hex UUID4 with no dashes. The directory is created
(parents) on save. A session file is rewritten in full after every run (not
appended), so a reader may observe a partially written file (Sentry already
retries on parse failure). **Intended:** write to a temp file in the same
directory and rename atomically.

### 4.2 Agent session file keys (written by the agent loop)

Top-level object, in this order:

| Key | Type | Content |
|---|---|---|
| `session_id` | string | the id |
| `model_config` | object | `model`, `base_url`, `api_key`, `temperature`, `timeout_seconds`, `pricing` {`input_cost_per_million_tokens_usd`, `output_cost_per_million_tokens_usd`, `cache_creation_input_cost_per_million_tokens_usd`, `cache_read_input_cost_per_million_tokens_usd`} |
| `runtime_config` | object | see 4.3 |
| `system_prompt_parts` | array of string | the parts as assembled at session creation |
| `user_context` | object string->string | frozen at creation |
| `system_context` | object string->string | frozen at creation |
| `messages` | array of OBJECT | transcript entries (2.5) |
| `turns` | int | cumulative session turns |
| `tool_calls` | int | cumulative tool calls |
| `usage` | object | the five usage keys + `total_tokens` |
| `total_cost_usd` | number | |
| `file_history` | array of object | history entries (5.10) |
| `budget_state` | object | `model_calls`, `session_turns`, `tool_calls`, `delegated_tasks`, `escalations` (ints) |
| `plugin_state` | object | opaque, owned by the plugin subsystem |
| `scratchpad_directory` | string or null | |

SECURITY NOTE: `model_config.api_key` is written to the file in clear text.
**Intended:** never persist the API key (re-read from env/flags on resume), or at
minimum write the file mode 0600. The rewrite should drop it from the file; the
loader must still tolerate its presence.

### 4.3 `runtime_config` object

Keys: `cwd`, `max_turns`, `command_timeout_seconds`, `max_output_chars`,
`stream_model_responses`, `auto_snip_threshold_tokens`,
`auto_compact_threshold_tokens`, `compact_preserve_messages`, `permissions`
{`allow_file_write`, `allow_shell_commands`, `allow_destructive_shell_commands`},
`additional_working_directories` (array of strings), `disable_claude_md_discovery`,
`budget_config` {`max_total_tokens`, `max_input_tokens`, `max_output_tokens`,
`max_reasoning_tokens`, `max_total_cost_usd`, `max_tool_calls`,
`max_delegated_tasks`, `max_model_calls`, `max_session_turns`; each null when
unset}, `escalation_config` {`enabled`, `cli_path`, `trigger_stop_reasons`
(array), `max_escalations`, `timeout_seconds`, `resource_thresholds`
{`max_memory_percent`, `max_cpu_percent`, `max_gpu_memory_percent`,
`min_disk_free_mb`}, `extra_cli_args` (array)}, `output_schema` (null or
{`name`, `schema`, `strict`}), `session_directory`, `scratchpad_root`.

Loader tolerance (must be kept): every optional key may be missing or null and
falls back to the defaults of 1.2; numeric fields accept numeric strings
(non-numeric -> null/default, booleans are NOT accepted as numbers); sub-objects
that are not objects are treated as empty; `output_schema` with a missing/empty
`name` or a non-object `schema` loads as none; `cwd` is required.
The top-level required keys when loading are: `session_id`, `model_config`
(`model` required inside), `runtime_config` (`cwd` required inside),
`system_prompt_parts`, `user_context`, `system_context`, `messages`, `turns`,
`tool_calls`. Everything else is optional with defaults (`usage` {},
`total_cost_usd` 0, `file_history` [], `budget_state` {}, `plugin_state` {},
`scratchpad_directory` null). Non-object entries in `messages` and
`file_history` are dropped.

### 4.4 IMPORTANT: Sentry mismatch (found during this audit)

`docs/ECOSYSTEM.md` section 4a says Sentry reads the files "this repo's
`session_store.py` already writes" and describes them as flat: `session_id`,
`messages` (a list of STRINGS), `input_tokens`, `output_tokens`. The Sentry Go
reader (`Chymaera_Sentry/internal/sources/agentsource`) globs `*.json` directly
in the configured directory and decodes `messages` as `[]string`.

The agent loop does NOT write that shape. It writes the file of 4.2 into
`.port_sessions/agent/`, one directory below where Sentry looks, with `messages`
as an array of OBJECTS and no top-level `input_tokens`/`output_tokens`. The flat
shape (4.5) is written only by an unused helper. Consequences today: pointed at
`.port_sessions/`, Sentry sees nothing from agent runs; pointed at
`.port_sessions/agent/`, JSON decoding fails on every file and each is skipped
silently. The Sentry integration is therefore effectively inert for real runs.

**Decision needed from the owner before the rewrite freezes the format.**
Options: (a) make the agent file of 4.2 itself the contract and have Sentry
decode `messages[].role/.content` and read `usage` (a Sentry-side struct change;
recommended); or (b) keep emitting the flat legacy file of 4.5 into
`.port_sessions/` in addition to the agent file (option "(c)" in later
sections refers to this). Whichever is chosen, document it in
`docs/ECOSYSTEM.md` 4a in the same commit and add a cross-repo test. The
information Sentry's detectors need is only: session id, the ordered message
texts (ideally with role so injection-in-tool-output can be distinguished from
user text), token totals, and a modification time.

### 4.5 Legacy flat session file (DROP-CANDIDATE)

`save_session`/`load_session` write `<dir>/<session_id>.json` (default dir
`.port_sessions`) containing `session_id`, `messages` (array of strings),
`input_tokens`, `output_tokens`. Nothing in the agent calls it; only a unit test
does. Keep only if option (b) of 4.4 is chosen.

### 4.6 Other files near the loop

- Scratchpad: directory `<scratchpad_root>/<session_id>` created (with parents)
  at session start, exposed to the model via context (8.2) as a place for
  throwaway files, and recorded in the session file.
- Context sources read at start-up are listed in section 8.

---

## 5. The turn loop

### 5.1 Entry points

`run(prompt)`: new session. Generates the id, creates the scratchpad,
processes the prompt, persists the session, returns the result.
`resume(prompt, stored_session)`: rebuilds the in-memory session from the stored
messages (section 10), appends the new prompt, continues under the same
session id.
Both add the run's usage and cost to agent-instance cumulative counters
(`cumulative_usage`, `cumulative_cost_usd`) used by `/cost`-style reporting.

### 5.2 Stop reasons (the `stop_reason` field, plus persisted/CLI value)

| Value | When |
|---|---|
| the backend's finish reason (e.g. `stop`, `end_turn`) | the model answered without tool calls |
| `tool_calls`/other | can appear as the stop reason only via the no-tool-calls path (a model that says `tool_calls` but returns none) |
| `max_turns` | loop reached `max_turns` without a tool-free answer |
| `budget_exceeded` | any configured budget exceeded (5.3) |
| `resource_exhausted` | system-resource threshold tripped (11.2) |
| `backend_error` | transport/protocol error not recovered (7.6) |
| `prompt_too_long` | context pre-flight cannot fit the next call (7.2) |
| `escalated_completed` / `escalated_failed` | external CLI escalation outcome (11) |
| `null` | a purely local slash command that returns without a model call |

### 5.3 Budgets (`BudgetConfig`), all optional, all strictly greater-than checks

| Field | Counted over | Failure explanation (human text; wording free) |
|---|---|---|
| max_total_tokens | `usage.total_tokens` | token budget exceeded, with counts |
| max_input_tokens | `usage.input_tokens` | |
| max_output_tokens | `usage.output_tokens` | |
| max_reasoning_tokens | `usage.reasoning_tokens` | |
| max_total_cost_usd | estimated cost | |
| max_tool_calls | tool calls requested | |
| max_delegated_tasks | sub-agent tasks | |
| max_model_calls | model calls | |
| max_session_turns | turns across the whole session incl. resumed history | |

Order of evaluation: token budgets (total, input, output, reasoning), then cost,
tool calls, delegated tasks, model calls, session turns; the first breach wins
and produces the explanation. Note: `max_output_tokens` and `max_input_tokens`
also feed the context pre-flight (9.4), and `max_output_tokens` is NOT forwarded
to the backend as a `max_tokens` request parameter (it only reserves space in the
arithmetic). **Recommendation:** forward it as `max_tokens`.

Budget overrides from workspace hook-policy manifests are applied at agent
construction ONLY for fields the caller left unset (caller wins).

### 5.4 The loop, step by step

Pre-loop (per `run`/`resume` call):
1. Slash-command pre-processing. A prompt beginning with a slash command may be
   fully handled locally (result with `turns=0`, no model call, `stop_reason`
   null, persisted nothing) or may rewrite the prompt and continue.
   (Slash commands are another spec; the hook point is: handled-and-no-query returns
   immediately; otherwise the possibly rewritten prompt continues.)
2. The prompt passes through before-prompt hook injections (workspace hook
   policy, then plugin hooks; each may prepend reminder text). On resume the
   plugin resume hook also runs. (Hooks: other specs. If the rewrite drops
   plugins/hook-policy, drop these two steps.)
3. Register the run with the agent manager (parent/child, label `root` or
   `resume` or caller-supplied).
4. Create the session (3.1) or take the resumed one; append the prompt as a
   `user` message.
5. Restore counters. New session: all zero. Resumed: usage, cost, tool-call count,
   turn count, model-call count and escalation count are read back from the stored
   session file (usage/cost/tool_calls/turns from the top-level keys,
   `model_calls`/`escalations` from `budget_state`); `delegated_tasks` is
   recomputed as the number of `file_history` entries whose `action` is a
   delegation action. If the stored file cannot be read (I/O error), counters
   start at zero.
6. Pre-flight budget check with restored counters. If already exceeded: stop
   with `budget_exceeded`, `turns=0`, `tool_calls=0` in the result, escalation
   offered (11), persist, return.
7. Pre-flight resource check (11.2). If exceeded: stop `resource_exhausted`,
   escalation offered, persist, return.

Per turn, for `turn_index` from 1 to `max_turns` inclusive:
1. Context reduction (section 7): (a) microcompact (dropped in rewrite), (b)
   snip if `auto_snip_threshold_tokens` set and > 0 (target = that threshold,
   no compaction), (c) heuristic compaction if `auto_compact_threshold_tokens`
   set and > 0 (target = that threshold, snip then compaction allowed).
2. Prompt-length pre-flight (7.2). It may spend a model call (LLM compaction);
   if so its usage and one model call are added to the totals and the budgets
   are re-checked (`budget_exceeded` stops, escalation offered). If the
   pre-flight says stop, return `prompt_too_long` with `turns = turn_index - 1`
   and no escalation.
3. Resource check (11.2); trip -> `resource_exhausted`, `turns = turn_index`, escalate, persist.
4. Model call (section 6/7). On a prompt-too-long style error one reactive
   compaction + retry is attempted (7.5). Other errors -> `backend_error`
   result with `final_output` = the error message text, `turns = turn_index - 1`,
   after-turn hooks, escalation offered, persist.
5. Account: model_calls +1, usage added, cost recomputed from CUMULATIVE usage
   x pricing (so restored cost is not preserved precisely; it is recomputed from
   usage), `last_content` = this turn's text.
6. Budget check; breach -> `budget_exceeded` (`turns = turn_index`), escalation
   offered, persist, return. (The assistant message of this turn is already in
   the transcript but its tool calls are not executed.)
7. If the turn has NO tool calls:
   - the text is appended to an accumulator of response segments;
   - if `finish_reason` is `length` or `max_tokens` (output truncated): append a
     user-role reminder asking the model to continue exactly where it stopped
     without repeating (metadata `kind=continuation_request`, `continuation_index`
     = number of segments, id `continuation_<turn_index>`), emit a
     `continuation_request` event, and go to the next turn (this consumes a turn);
   - else finish: `final_output` = all accumulated segments concatenated (no
     separator), `stop_reason` = the finish reason, after-turn hooks, persist, return.
8. If the turn HAS tool calls: clear the segment accumulator (segments from
   before tool calls are not part of the final output) and process each call
   in order (5.5). Then loop.
After the last turn without a tool-free answer: `stop_reason` `max_turns`,
`turns = max_turns`, `final_output` = last turn's text, or a fixed "stopped:
max turns reached" explanation if the text is empty; persist; no escalation.

### 5.5 Per tool call

For each call in the assistant turn, in order:
1. If this `tool_call.id` was already completed earlier IN THIS RUN: append a tool
   message with a short fixed marker saying it is a duplicate already executed;
   do not execute and do not count it. (Regression-tested; some local servers
   re-emit ids.)
2. Count it: `tool_calls += 1`; if the tool is the sub-agent tool, add the
   number of delegated task units its arguments describe.
3. Budget check with the new counts (tool-call and delegated-task limits). On
   breach: emit `task_budget_exceeded` event, stop `budget_exceeded`
   (escalation offered), persist, return. **DO NOT REPLICATE:** the assistant
   message with the tool calls is left in the transcript without matching tool
   results, which produces an invalid conversation if the session is later
   resumed (every tool_call must be answered). Intended: answer every
   outstanding call with a tool message stating it was not run because of the
   budget before stopping.
4. Create a streaming tool message (`start_tool`) and emit `tool_start`.
5. Plugin and hook-policy pre-flight and block decisions (other specs): a block
   produces a failed tool result without running the tool; hook-policy blocks carry
   `error_kind=permission_denied`.
6. Execute: sub-agent tool and skill tool are handled by the runtime; everything
   else goes to the tool executor, which yields incremental `delta` updates (each
   appended to the tool message and emitted as `tool_delta` with `stream` name and
   `delta`) and finally a result {`ok`, `content`, `metadata`}. A tool with no
   final result is an internal error.
7. Post: record result for plugins; collect plugin and hook-policy result messages
   into the result metadata; if `error_kind == permission_denied`, emit
   `tool_permission_denial` (reason = content, source `hook_policy` or `tool_runtime`).
8. Finalise the tool message: content = serialised result JSON (2.5) after
   redaction (3.5), metadata merged (`phase=completed` etc.), `stop_reason`
   `tool_completed`; remember the id as completed; emit `tool_result` (ok + metadata).
9. Follow-up events from runtime tools (plan/task/etc.; other specs); if plugin
   or policy produced messages, append one `user` reminder message summarising them
   (`kind=plugin_tool_runtime`).
10. Refresh runtime views: certain successful tools (plan/task/todo, config_set,
    search/remote/account/team/workflow/worktree/memory_* tools, `ask_user_question`,
    `remote_trigger`) invalidate the context caches and reload the matching runtime
    object from disk, and a `cwd_update` string in the result metadata switches the
    agent to a new working directory (rebuilding every runtime and the tool registry).
11. Append a file-history entry if applicable (5.10).
Tool calls within one assistant turn run sequentially in order, never in parallel.

### 5.6 Tool errors the model should see

Unknown tool names, invalid argument shapes and permission denials are returned
as normal failed tool results (`ok=false`, readable content, `error_kind` in
metadata where applicable), not as loop errors. Write tools without write
permission fail with `error_kind=permission_denied` and a message telling the
operator which flag to rerun with.

### 5.7 Tool allowlist

`tool_allowlist` (set by the HTTP service per session): when set, ONLY those tool
names exist for the agent; an empty list means no tools at all. It is applied
whenever the registry is built or rebuilt (including after `cwd_update`). This is
stronger than permissions, which only deny at call time while still advertising
the tool. When there are no tools the request omits `tools` and `tool_choice` entirely (7.1).

### 5.8 Tools advertised

On each model call the full current registry is sent as OpenAI function tools
`{"type":"function","function":{"name","description","parameters"}}`, with
`tool_choice` `auto`.

### 5.9 Known counting quirks (DO NOT REPLICATE)

- Persisted `tool_calls` on a resumed run = previous + (result.tool_calls, which
  already contains previous) -> double counting. Intended: cumulative once.
- Persisted `budget_state.model_calls` adds the run's `turns`, not its real number
  of model calls (continuations and LLM compactions are model calls too, and a
  turn with an error is not). Intended: persist the real count.
- `turns` in a run result is per-call, while the persisted `turns` and
  `max_session_turns` are cumulative; keep both meanings but name them clearly.
- `max_turns` is per call, not cumulative; a resumed session gets a fresh allowance.
- `_compact_consecutive_failures` (7.7) is instance state never reset except by
  a successful compaction; not persisted.

### 5.10 File history entries

Appended per tool call whose result metadata contains a `path`, a `command`, or
a delegation `action`. Entry keys: `timestamp` (UTC ISO-8601), `turn_index`,
`tool_call_id`, `tool_name`, `ok`, `history_entry_id`
(`<turn>:<call id>:<tool>`), `result_preview` (whitespace-collapsed, 220 chars max
with `...`), all keys of the result metadata merged in, plus `history_kind` =
`file_change` (path present; also sets `changed_paths` [path] and, when the
metadata has `before_sha256`/`after_sha256`, `before_snapshot_id`/`after_snapshot_id`
= `<path>:<first 12 hex>`), `shell` (command present), `delegation` (sub-agent;
adds `delegate_batch_count`, `dependency_skips` when present) or `tool`.
Also stored here: escalation records (`action=escalate_claude_code`, 11.3).
History is persisted in the session file and replayed on resume (section 10).

---

## 6. Events

`AgentRunResult.events` is the ordered list of event dicts for this run
(persisted only in memory/returned, NOT written into the session file). GUI and
service consumers read them. Names are identifiers and must be kept if a UI is kept.

Model stream events (only in streaming mode; one dict per backend event):
`message_start`, `content_delta` (field `delta`), `tool_call_delta`
(`tool_call_index`, `tool_call_id`, `tool_name`, `arguments_delta`), `usage`
(`usage` object), `message_stop` (`finish_reason`). Every event dict has the
keys `type`, `delta`, `tool_call_index`, `tool_call_id`, `tool_name`,
`arguments_delta`, `finish_reason`, `usage`, `raw_event`.

Runtime events (always `type` plus listed fields; all carry `turn_index` unless
noted): `tool_start` (`tool_name`, `tool_call_id`, `message_id`), `tool_delta`
(+`stream`, `delta`), `tool_result` (+`ok`, `metadata`), `tool_permission_denial`
(+`reason`, `source`), `task_budget_exceeded` (+`tool_name`, `tool_call_id`,
`reason`), `continuation_request` (`reason`, `continuation_index`),
`prompt_length_check`, `prompt_length_recovery`, `prompt_length_warning`,
`auto_compact_summary`, `auto_compact_failed`, `auto_compact_circuit_breaker`,
`snip_boundary`, `reactive_snip_boundary`, `compact_boundary`,
`reactive_compact_boundary`, `reactive_compact_retry`, `microcompact`, and
plugin/hook-policy events (other specs). Field lists for the context events are in section 7.

In non-streaming mode no model stream events are emitted (only runtime events).

---

## 7. Context-pressure management

Goal: keep each request inside the model's context window. Layers, cheapest first.

### 7.1 Message eligibility concepts

- **Protected prefix** = leading messages that are never touched: the system
  message, and, if the second message is a user message starting with the
  system-reminder opening tag, that reminder too (so 0, 1 or 2 messages). Already-
  compacted boundary messages are NOT part of the prefix by this definition in
  the heuristic path, but ARE skipped when re-compacting in the LLM path.
- **Preserved tail** = the last `compact_preserve_messages` messages (CLI default 4;
  halved, min 1, in reactive mode).

### 7.2 Prompt-length pre-flight (every turn, after 5.4 step 1)

Compute the token budget snapshot (9.4). If neither the soft nor the hard limit
is exceeded: nothing happens. Otherwise:
1. emit `prompt_length_check` (turn, projected/soft/hard limits, overflows, exceeds_hard).
2. Target = the hard limit if it is exceeded, else the soft limit.
3. Run context-pressure reduction (7.3, compaction allowed) toward the target. If it
   changed anything: recompute; emit `prompt_length_recovery` (strategy label
   `heuristic`); if now under both limits, proceed to the model call.
4. If still over: if the compaction circuit breaker is open (>= 3 consecutive
   failures, 7.7) emit `auto_compact_circuit_breaker` and skip step 5.
5. Else if the session has more messages beyond the protected prefix than the
   preserve count: run LLM compaction (7.5) with fixed automatic instructions
   (preserve the active task, recent file changes, failures, pending work and
   exact next step). Success: reset failure counter, recompute, emit
   `auto_compact_summary` (pre/post/true-post token counts, summary usage tokens,
   ptl retries, new projected/limits/flags). If now under both limits, proceed
   (the summary's usage and one model call are charged to the run). If still
   over, return the charge and, only if the HARD limit is exceeded, stop with
   `prompt_too_long`. Failure: increment the failure counter, emit
   `auto_compact_failed` (reason, count).
6. After all that, if the hard limit is still exceeded: stop with `prompt_too_long`
   and an explanation listing projected tokens, hard limit and soft limit; no model
   call is made. Otherwise emit `prompt_length_warning` and proceed anyway (soft
   overflow only is a warning, not a stop).
Test-defined behaviours: with a backend mock that raises if called, a hard overflow
yields `prompt_too_long` and `final_output` explaining the stop; with a snapshot fake,
auto compaction runs before the model call and the next request contains the summary.

### 7.3 Reduction loop and snipping (no model call)

`reduce(target, allow_compaction, reactive)`: repeat at most 6 times:
measure the session's context tokens (9.1); if <= target stop; try a snip pass;
if it removed something continue; else if compaction allowed try a heuristic
compaction pass (7.4): if it compacted, continue when reactive, else stop; else stop.
Returns whether anything changed.

Snip pass: candidates are messages between the protected prefix and the preserved
tail that are snippable: tool-role messages; assistant messages that have tool
calls or content longer than 600 chars; user messages whose `kind` is
`continuation_request` or `file_history_replay`. Never `compact_boundary`,
`snipped_message` or `file_history_replay` kinds (note the contradiction: the
user-`file_history_replay` clause can never be true because that kind is excluded
first; replay messages are therefore never snipped - fine, keep them unsnippable).
Walk candidates oldest first; for each, build a replacement text (a system-reminder
style note stating which kind of message was snipped, its message id, and a
whitespace-collapsed preview of its old content limited to 120 chars); only
replace if the replacement is strictly shorter in tokens than the original. The
replacement keeps role and ids (tool messages keep `tool_call_id` and `name`, so
the conversation remains valid), sets `state=tombstoned`,
`stop_reason=snipped_for_context`, metadata `kind=snipped_message`, estimated
original/replacement tokens, turn index, source role/message id/kind/lineage/revision.
NOTE: for an assistant message with tool calls the `tool_calls` field is kept
(only content is replaced), so pairing stays valid. Non-reactive passes stop as
soon as the running total is <= target; reactive passes ignore the target but
stop after 3 snips. Emit `snip_boundary` (or `reactive_snip_boundary`): count of
snipped messages, estimated tokens removed, snipped message ids.

### 7.4 Heuristic compaction pass (no model call)

Replaces all messages between the protected prefix and the preserved tail with one
boundary message. Needs at least one candidate. The boundary message has role
`system`, a text body containing: a statement that earlier history was compacted,
a bullet list of up to 24 of the compacted messages in order (label = role, or
`tool:<name>` for tool messages; whitespace-collapsed text capped at 160 chars,
literal `(empty)` when blank), a `... plus N older messages` line if more than 24,
and an instruction to treat the preserved tail as the working set. Id
`compact_boundary_<turn>_<count>`. Metadata: `kind=compact_boundary`, lineage
fields, `turn_index`, `compacted_message_count`, `estimated_tokens_before`,
`estimated_tokens_removed`, `preserved_tail_count`, `preserved_tail_ids`,
`compaction_depth` (1 + max depth among compacted messages that carry it; nested
compactions deepen), `nested_compaction_count` (how many earlier boundaries were
swallowed), `compacted_kinds` (count by `kind` or role), `compacted_lineage_ids`,
`preserved_tail_lineage_ids`, `max_source_revision`, `max_source_mutation_serial`,
`source_mutation_totals`, `compacted_revision_total`, `compacted_message_ids`.
Event `compact_boundary` (or `reactive_compact_boundary`) carries turn index, compacted
message count, estimated tokens before/removed, preserved tail count and ids,
depth, nested count, compacted ids.
**Caveat (DO NOT REPLICATE):** compaction may split a tool-call/tool-result pair
(an assistant message with tool calls compacted while its results stay in the tail,
or the reverse), which strict backends reject. Intended: when choosing the cut, move
it so that no tail tool message lacks its assistant call and no compacted assistant
call is orphaned from a kept result (the dead session-memory module has a reasonable
two-pass algorithm for this: extend the kept region backwards until every kept
tool result's call is kept, repeating until stable).
Test-defined: with threshold 80 and preserve 1, one `compact_boundary` event is
emitted, the next request contains exactly one compaction note, and the transcript has
one message with `kind=compact_boundary`, `compaction_depth=1`, `nested_compaction_count=0`,
and a `preserved_tail_ids` key.

### 7.5 LLM compaction (`compact_conversation`, also the `/compact` command)

Inputs: the agent, optional custom instructions. Steps:
1. No session or no messages -> error "not enough messages".
2. (Dead path) try session-memory compaction when no custom instructions -> dropped.
3. Prefix = leading messages whose `kind` is `compact_boundary`. Tail = last
   `compact_preserve_messages` (min 1) of the rest. Candidates = the middle; none -> error.
4. Pre-compaction token estimate = sum over ALL messages (content only).
5. Build the request: every non-blank system-prompt part as a system message, then
   the candidates in OpenAI form, then a final user message containing the compaction
   instruction (see below). Call the model with NO tools, using the normal client.
6. Response classification: exception whose text contains both "prompt" and "long"
   -> prompt-too-long; empty/whitespace response -> error `empty_response`; response
   text containing one of the phrases `prompt is too long`, `prompt_too_long`,
   `context_length_exceeded` -> prompt-too-long; otherwise success.
7. Prompt-too-long retry: up to 3 retries; each drops the OLDEST ~20% (at least 1)
   of API-round groups from the candidates (a group = a run of messages starting at an
   assistant message whose id differs from the previous assistant's id; at least 2
   groups are required and one is always kept); if the first remaining message is
   an assistant, prepend a synthetic user marker message (id `ptl_retry_marker`,
   metadata `kind=ptl_retry_marker`, `is_meta`) so the sequence starts with a user turn; a
   stale marker from a previous retry is stripped first. After the retries are
   exhausted or nothing can be dropped: result with error "compact request itself too long".
   Usage of all attempts is summed.
8. Success: parse the response: delete any `<analysis>...</analysis>` scratch block,
   unwrap `<summary>...</summary>` to `Summary:\n<text>`, collapse runs of blank lines,
   trim. The replacement is: prefix + boundary message + summary message + tail, where
   the boundary is a `user`-role reminder noting how many messages / ~tokens were
   compacted (id `compact_boundary`, `kind=compact_boundary`) and the summary is a
   `user` message (id `compact_summary`, metadata `kind=compact_summary`,
   `is_compact_summary`) saying the session continues from a previous conversation that
   ran out of context, followed by the formatted summary. (Optionally, a path to a full
   transcript and/or a "continue without asking" instruction can be appended; the
   existing callers use neither.)
9. Result record: boundary message, summary messages, kept messages, pre/post/"true
   post" token counts (true post excludes the unchanged prefix), summary text, usage,
   error (null on success), retry count.
What the compaction instruction must convey (write fresh text): respond with plain
text only and NO tool calls (tools are not offered, but local models try anyway);
produce a private analysis block then a summary block; the summary must have these
nine sections in order: primary request and intent; key technical concepts; files and
code sections touched (with why they matter, snippets where relevant); errors and
fixes including user corrections; problem solving; ALL user messages that are not tool
results; pending tasks; current work (precise, most recent first); optional next step,
which must stay in line with the user's latest explicit request and quote the most recent
exchange verbatim to prevent drift. Custom instructions are appended under a heading.
The instruction is restated as a short reminder at the end ("no tools, plain text only").
Rated USEFUL; local small models will give poorer summaries than the original target, so
keep the heuristic path (7.4) as the always-available fallback.

### 7.6 Reactive compaction (backend says the prompt is too long)

When a model call raises an error whose lowercase text contains any of: `prompt is too
long`, `maximum context length`, `context length exceeded`, `too many tokens`, `input too
long`, `context window` -> run reduction with target 0, compaction allowed, reactive mode
(7.3: snips up to 3 per pass, repeated, then a heuristic compaction with a halved preserved
tail, repeated up to the 6-iteration cap). If anything changed, retry the model call ONCE;
on success emit `reactive_compact_retry` and continue the normal flow (budget check, tool
handling, continuation). If the retry errors or nothing changed, return `backend_error`.
This is the recovery for backends whose context window differs from the inferred one (9.3).

### 7.7 Circuit breaker

LLM compaction failures counted on the agent instance; at >= 3 consecutive failures
LLM compaction is skipped (event `auto_compact_circuit_breaker`) until one succeeds
(it cannot succeed while skipped, so in practice it stays open for the life of the process
- **DO NOT REPLICATE:** reset it at the start of each new run).

### 7.8 Microcompact (DROP-CANDIDATE)

Described only for completeness: if more than 60 minutes passed since the last assistant
message's `timestamp`/`created_at` metadata, tool results of eight named tools beyond
the 3 most recent are replaced by a short "cleared" marker (metadata
`microcompact_cleared`, original token estimate); emits event `microcompact`. Nothing ever
writes those metadata keys, so it never triggers, and the cache-expiry premise does not
hold for local servers. Do not implement. (If a time-based clear is ever wanted, record
`created_at` on every message first.)

### 7.9 Session-memory compaction (DROP-CANDIDATE)

Described only for completeness: reads a summary file `~/.claude/session-memory/session.md`
(nine headed sections), and if present and not just a template, uses it as the compaction
summary without a model call, keeping messages after the "last summarised message id" with
minimum 10,000 tokens / 5 text messages and a hard cap of 40,000 tokens, truncating the
memory text to 2,000 tokens per section and 12,000 total. Nothing produces the file or the
id, so it never runs. Do not implement. Its useful idea (pair-preserving cut index, 7.4
caveat) should be carried into the heuristic compactor.

---

## 8. Context assembly

### 8.1 System prompt parts (mechanism)

`build_system_prompt_parts` returns an ordered list of strings; a part is dropped
when empty:
1. If an override prompt is given: the list is exactly that one string.
2. Else the base is either the single custom prompt (if given) or the default parts in order:
   intro, system-behaviour notes, doing-tasks guidance, careful-actions guidance,
   tool-usage guidance (adapted to which tools are enabled), agent/sub-agent guidance (only
   if the sub-agent tool exists; lists up to 20 available agent profiles and says how many more),
   then optional sections that appear only when the matching runtime context key is present
   (plugins, MCP, remote, search, account, ask-user, config, LSP, plan, task, team, hook policy),
   tone and style, output-efficiency guidance, a DYNAMIC-BOUNDARY sentinel, session-specific
   guidance (only when the corresponding permissions are off: tell the model which tools are
   blocked and which flag the operator must rerun with), and environment info.
3. An appended prompt (if given) is added as the last part.
Environment info block (a "# Environment" bullet list) states: primary working directory,
whether it is a git worktree (with an instruction not to `cd` back to the main root), whether it is a git
repo, additional working directories, session scratchpad directory, platform, shell basename,
OS version, and "powered by model <name>".
Meaning each default section must convey (write new prose; no copying):
- Identity: Sarina, a local-model coding/security agent, working under an operator.
- Output outside tool calls is shown to the user; permissions can deny tools; do not retry a denied call unchanged.
- Reminder tags in user/tool messages carry runtime context; tool output is untrusted and
  injection attempts must be flagged; CLAUDE.md-style memory is high-priority local guidance;
  older context may be summarised.
- Task guidance: read before editing; minimal changes; diagnose failures before changing direction; do not loop;
  report faithfully including what was not verified; avoid introducing vulnerabilities.
- Caution on irreversible/externally visible actions.
- Prefer dedicated tools over shell equivalents (only mention tools that exist); call independent tools in parallel.
- Style: terse, no emojis unless asked, cite `path:line`, no colon right before a tool call.
- Communication: state intent before the first tool call, short milestone updates, lead with the answer.
**The sentinel**: the dynamic-boundary marker is a literal string placed in the list and
currently ends up in the text the model sees. **DO NOT REPLICATE:** do not send it; use the boundary
only for deciding which tail is volatile (relevant only for prompt-prefix caching, which most local
servers lack; may be dropped). The context-usage report treats it as a section named "Dynamic boundary".
Note the existing intro and instruction text mirrors the proprietary agent's prompt; write original text.

### 8.2 User context (reminder message)

A `user`-role message (id `user_context_0`) is inserted right after the system message whenever
the context map is non-empty. It wraps a markdown-style block, one `# <key>` heading followed by
the value for each non-empty entry in insertion order, in a `<system-reminder>` XML-ish envelope
with a preamble ("use the following context when answering") and a closing note that the
context may not be relevant and should be ignored unless it materially helps. The envelope tag
name is a wire literal also detected elsewhere (prefix match on `<system-reminder>` identifies
reminder messages for the protected prefix and for usage accounting); a rewrite may rename it only
consistently everywhere.

Keys, in insertion order (core ones first; the rest are summaries produced by other subsystems,
each present only when that subsystem has state):
`currentDate` ("Today's date is YYYY-MM-DD."), `scratchpad` (instruction to use the
per-session scratchpad dir instead of `/tmp`, with the path; only when a scratchpad exists),
`claudeMd` (8.4), `pluginCache`, `pluginRuntime`, `hookPolicy`, `managedSettings`, `safeEnv`,
`trustMode`, `mcpRuntime`, `remoteRuntime`, `remoteTriggerRuntime`, `searchRuntime`,
`accountRuntime`, `askUserRuntime`, `configRuntime`, `lspRuntime`, `planRuntime`,
`taskRuntime`, `curatedMemory` (13.1), `teamRuntime`, `workflowRuntime`, `worktreeRuntime`.
When `disable_claude_md_discovery` is on, ONLY `currentDate` and `scratchpad` are produced
(all discovery-based sections are skipped, including curated memory).
Context maps are cached in-process keyed by (cwd, additional dirs, disable flag, scratchpad)
and invalidated explicitly (after state-changing tools and cwd changes). Keep a cache only if
cheap; correctness requires re-reading after tools that change that state.

### 8.3 System context

A map appended to the system message as `key: value` lines (non-empty values only,
one line per key) after the prompt parts: `gitStatus` (8.5), `cacheBreaker` (only if a
global injection string was set: `[CACHE_BREAKER: <value>]`; test-only feature,
DROP-CANDIDATE), `scratchpadDirectory` (path).

### 8.4 CLAUDE.md-style memory discovery (the `claudeMd` entry)

Files are collected in this order, de-duplicated by resolved path (first occurrence wins):
1. global: `~/.claude/CLAUDE.md`.
2. for each directory from the filesystem root DOWN to the cwd (outermost first): in this
   order, `CLAUDE.md`, `.claude/CLAUDE.md`, `CLAUDE.local.md` in that directory (those that
   are files), followed by every `*.md` file in `<dir>/.claude/rules/` in sorted name order.
3. for each additional working directory: the same per-directory set as in 2 (no ancestor walk).
Each file is read as UTF-8 with replacement of bad bytes, stripped; empty files are skipped; a
file longer than 40,000 characters is cut at 40,000 and a notice appended that it was
truncated. Result text = a fixed instruction sentence (instructions are shown below and
override default behaviour when they directly apply) followed by one block per file: a
`## <absolute path>` heading and the content, separated by blank lines. If no non-empty file
was found the entry is absent. Note the file names `CLAUDE.md`/`.claude/` are part of the
Claude Code convention this project reads; keep reading them (project and the existing
CLAUDE.md of this repo depend on it) and consider also reading `SARINA.md`/`.sarina/` as a
future addition (not existing behaviour).
Consumers: the context-usage accounting parses this text back into per-file token counts by
matching `## <absolute path>` header lines (absolute POSIX or drive-letter paths).

### 8.5 Git status snapshot (`gitStatus`)

Present only if cwd is inside a git work tree (a `.git` entry in cwd, or `git rev-parse
--is-inside-work-tree` says true). All git commands run with a 2-second timeout; any failure
yields "unknown"/absent for that item. Contents, as paragraphs separated by blank lines:
a sentence that this is a snapshot at conversation start that does not update; `Current
branch: <name or (unknown)>`; `Main branch: <name or (unknown)>` (from
`refs/remotes/origin/HEAD` if it exists, else `main` or `master` if that local branch
exists); `Git user: <user.name>` (omitted if unset); `Status:` followed by `git status
--short` (with optional-locks disabled) truncated to 2,000 chars plus a truncation note, or
`(clean)`; `Recent commits:` followed by the last 5 one-line log entries or `(none)`.

### 8.6 Curated-memory ranking is dead in the live path (DO NOT REPLICATE)

`build_session(user_prompt)` is designed to replace the full curated-memory block with a
relevance-ranked subset (13.1) when a prompt is available. The live `run` path calls it with
NO prompt and appends the prompt afterwards, so ranking is never applied in real runs; it is
only used by the context-report command. **Intended:** at the start of a new session, rank
memory against the first user prompt (one place, before the session is created). On resume
the frozen stored context is reused unchanged (no re-ranking).

### 8.7 Environment context report (`/context`, DROP-CANDIDATE)

A markdown report of environment, user context and system context. Not needed by the loop.

---

## 9. Token counting and the budget arithmetic

### 9.1 What is counted

`collect_context_usage(session, model)`:
- System prompt: tokens of each system-prompt part (sections named by their first
  markdown heading line, else "Section N", sentinel "Dynamic boundary") plus each non-empty
  system-context entry as `key: value`.
- User context: each non-empty entry counted as `# key\nvalue` (matching how it is
  rendered), but the reminder MESSAGE itself (index 1, role user, content starts with the
  reminder tag, only when user_context is non-empty) is skipped when walking messages to avoid
  double counting. The system message (index 0) is skipped likewise.
- Messages: user = content; assistant = content plus, for each tool call, the tokens of its
  JSON-serialised object (ASCII-escaped) and attributed to the call's function name; tool =
  content, attributed to the message `name`. Reports totals by category (System prompt, User
  context, User messages, Assistant messages, Tool calls, Tool results, Free space = window - total,
  min 0) and per-tool call/result tokens sorted descending by total.
Total = sum of the six categories (excluding Free space). Because the compaction boundary and
snip tombstone messages are ordinary messages they are counted as such.

### 9.2 Counter selection (`tokenizer_runtime`)

Cached per (model, three env settings). Order: if the model name looks like an OpenAI
model (starts with `gpt`, `o1`, `o3`, `o4`, or contains `gpt-4`, `gpt-5`, `openai`) prefer a
tiktoken-style tokenizer: encoding by model name, else `o200k_base` for gpt/o-series names,
`cl100k_base` for gpt-3.5/gpt-4 names; fall back to a HuggingFace tokenizer; otherwise
the order is reversed. The HuggingFace path is tried only when a reference exists: reference =
`SARINA_TOKENIZER_PATH`, else `SARINA_TOKENIZER_MODEL`, else the model name; it is eligible only
if the reference is an existing path or contains a slash/backslash; loading is local files
only (never downloads); `SARINA_TOKENIZER_TRUST_REMOTE_CODE` in `1/true/yes/on`
enables remote-code tokenizers. If all fail: heuristic = max(1, ceil(chars/4)); empty text is 0
for real tokenizers and 0 for the heuristic. Each counter reports a backend name
(`tiktoken`, `transformers`, `heuristic`), a source description and an `accurate` flag
(false for heuristic); these are displayed in reports.
Rewrite guidance: Go has no cheap equivalent of those libraries. Implement the heuristic as
the default, optionally refine with a calibration factor learned from the backend's
reported `usage.prompt_tokens` versus the heuristic estimate for the same request (real
local usage numbers are available on every response and are better than any offline tokenizer
for the actual served model). That is a recommendation, not existing behaviour.

### 9.3 Context window inference

By lower-cased model name, first match wins: contains `qwen3-coder` -> 256,000;
contains `devstral` -> 256,000; contains `qwen` -> 131,072; contains `claude` -> 200,000;
contains `gpt-4.1` or `gpt-4o` -> 128,000; anything else 128,000. There is no override and
no query to the backend. **Recommendation:** accept an explicit configured value
(`SARINA_CONTEXT_WINDOW` and a CLI flag), prefer it, and where a backend exposes it
(Ollama `/api/show`, vLLM `/v1/models` `max_model_len`, llama.cpp `/props`), read it at start-up.
Smaller servers (e.g. Ollama default 2k-8k `num_ctx`) will otherwise fail through the reactive path
(7.6) only after a rejected request; some backends silently truncate instead of erroring.

### 9.4 Token budget snapshot (`calculate_token_budget`)

Constants: default output reserve 4,096; minimum output reserve 1,024; per-message
framing overhead 5; extra 2 if the message has a name; 12 per tool call; flat +3 for the
conversation; schema overhead 256 when a structured-output schema is configured; auto-compact
buffer 13,000.
- window = inferred context window (9.3)
- framing overhead = sum over messages (5 + (2 if name) + 12 * len(tool_calls)) + 3
- reserved output = `max_output_tokens` if set (min 0), else min(4096, max(window/16, 1024))
- reserved schema = 256 if schema else 0
- hard input limit = max(window - reserved output - reserved schema, 0); if `max_input_tokens`
  is set, hard = min(hard, max(max_input_tokens, 0))
- compaction buffer = min(13000, max(window/10, 1024)) (integer division)
- soft input limit = max(hard - buffer, 0)
- projected input = counted context tokens (9.1) + framing overhead
- overflow = max(projected - hard, 0); soft overflow = max(projected - soft, 0);
  `exceeds_hard` = overflow > 0; `exceeds_soft` = soft overflow > 0.
Snapshot fields: model, window, projected, message tokens, framing overhead, reserved
output/compaction buffer/schema tokens, hard and soft limits, both overflows and flags,
counter backend/source/accurate. A markdown formatter exists for a `/token-budget`-type
command (DROP-CANDIDATE; any human-readable rendering is acceptable).
Note: tool definitions (the `tools` array sent every request, easily thousands of tokens) are
NOT counted in the projection. **Recommendation:** count the serialised tool specs.

---

## 10. Resume behaviour

`resume(prompt, stored)`:
1. Rebuild messages from the stored transcript entries (3.6). The STORED system prompt parts,
   user context and system context are reused as-is (frozen at creation; a resumed session
   never re-reads CLAUDE.md, git status, memory, or the date).
2. File-history replay: if the stored `file_history` is non-empty, append ONE user reminder
   message (id `file_history_replay_<count>`, metadata `kind=file_history_replay`,
   `file_history_count`, `file_history_unique_paths`, `file_history_snapshot_count`) summarising:
   number of entries, number of unique changed paths, number of snapshot ids, a preview of the
   first 4 paths with "+N more", then the last 10 entries one per line with `action` (or tool name),
   entry id, turn, path, command, child-session count, batch count, dependency skips and the
   before/after snapshot ids and previews and result preview; a line noting older entries omitted;
   and an instruction to use the replay to avoid repeating prior edits/commands. Skipped if the
   latest existing replay message already has the same entry count.
3. Compaction replay: if the transcript contains any `compact_boundary` or `snipped_message` kind and
   no `compaction_replay` message exists yet, append one user reminder (id
   `compaction_replay_<boundaries>_<snipped>`) stating the counts, the latest boundary's id, depth,
   compacted-lineage count, max source mutation serial, per-kind mutation totals, up to 4 preserved
   tail ids, up to the last 3 snipped ids and lineages, and instructing the model to treat the surviving
   transcript plus summaries as authoritative.
4. Restore plugin session state from the stored `plugin_state` (other spec); scratchpad is the stored
   path if present, else created fresh.
5. The new prompt then follows 5.4. Counters are restored from the stored file as in 5.4 step 5, so the
   cumulative budgets (`max_model_calls`, `max_session_turns`, `max_tool_calls`, tokens, cost) and the
   escalation cap carry across resumes; `max_turns` does not.
6. Persisting (4.1) rewrites the same file under the same id: `turns` = previous + this run's turns;
   `tool_calls` see quirk 5.9; usage/cost = the cumulative figures; `file_history` = the cumulative list;
   `messages` = the full current transcript (including injected replay/compaction reminders,
   which accumulate in the file).
CLI resume (`agent-resume <session_id>`): loads the stored model/runtime config from the file and
applies explicit command-line overrides on top (model, base URL, API key, temperature, timeout,
pricing, max turns, permissions only ever WIDENED (flags OR-ed, never narrowed), stream, snip/compact
thresholds, budgets individually, escalation settings, response schema, scratchpad root). The chat
loop (`agent-chat`) keeps a current session id: first prompt `run`, subsequent prompts `resume` the file just written.
Messages left by a run that stopped mid-turn (budget stop, 5.5 step 3) may contain unanswered tool calls;
see the intended fix there.

---

## 11. Escalation and resource checks

### 11.1 Escalation to the real `claude` CLI (opt-in)

Config: `enabled` (default false; CLI `--escalate-to-claude-code`), `cli_path` (default `claude`),
`trigger_stop_reasons` (default `backend_error`, `resource_exhausted`; CLI may add
`budget_exceeded`; repeatable `--escalation-trigger`), `max_escalations` (default 1),
`timeout_seconds` (default 300), resource thresholds (11.2), `extra_cli_args` (config file only).
When a hard stop is about to be returned (`backend_error` incl. unrecovered reactive failure,
`budget_exceeded`, `resource_exhausted`; NOT `prompt_too_long`, NOT `max_turns`), the runtime
may hand the task to the CLI. All must hold: enabled; the stop reason is in the trigger list;
escalations so far in this session (restored from `budget_state.escalations` on resume) < max;
the CLI binary is found on PATH. If any fails the original result is returned unchanged
(test: disabled by default -> unchanged; budget stops are not escalated by default; CLI absent -> unchanged).
Otherwise:
1. Register a child agent record labelled `claude_code_escalation`.
2. Build a self-contained prompt: the original (effective) task, why we are escalating
   (the stop reason; for `resource_exhausted` also the memory/CPU/GPU percentages), and a digest of
   up to the last 8 user/assistant/tool messages, each non-blank one as `[role] text` capped at 400
   chars (`(no prior progress)` if none), and a request to continue and finish.
3. Run the CLI as a subprocess with the arguments: `-p <prompt> --output-format json --cwd <cwd> --bare`,
   then `--add-dir <dir>` for each additional directory, then permission flags, then the extra args.
   Permission flags: if the local agent was allowed to write files or run shell commands,
   `--permission-mode acceptEdits`; otherwise none. NEVER emit a bypass-permissions mode (tested).
   Timeout per config. Stdout/stderr previews are kept to 2,000 chars each.
4. Parse stdout as one JSON object. Success requires exit code 0, a parseable object, and
   `is_error` falsy. Fields read: `result` (final text), `session_id`, `total_cost_usd`.
   Timeouts, launch failures, non-zero exit, unparseable/non-object JSON, and `is_error` all yield
   a failed escalation (never an exception); `stop_reason` is `escalated_completed` or `escalated_failed`.
5. Always append a file-history entry: `action=escalate_claude_code`, `trigger_reason`, `ok`,
   `cli_session_id`, `cli_stop_reason`, `total_cost_usd`, previews, `error`. On failure, the original
   result is returned with only that entry added (stop reason unchanged). On success: append an
   assistant message (id `escalation_<child id>`) with the CLI's final text, set `final_output` and
   `stop_reason` to the escalated values, refresh the transcript, and add the CLI's cost to the total.
Rating USEFUL: valuable on a small local model; for a security distro it sends the task to a
remote service, so it must remain opt-in and visible. The escalation count persisted in
`budget_state.escalations` is the number of escalation entries in file history.

### 11.2 System resource pressure

Telemetry: memory percent and CPU percent (CPU sampled over 0.1 s) via the optional `psutil`;
GPU memory percent of the FIRST GPU from `nvidia-smi --query-gpu=memory.used,memory.total
--format=csv,noheader,nounits` (5-second timeout; absent binary/failure/zero total -> unknown);
free disk MB of the workspace filesystem. Without the optional library the snapshot is marked
`available=false` (memory and CPU checks are skipped; GPU and disk still run). In Go, use native
facilities and always mark `available=true`.
Threshold defaults: memory 90%, CPU 90%, GPU memory 90%, min disk free unset; `--disable-resource-check`
unsets the three percentages. Evaluation order and comparison: memory `>=`, then CPU `>=`, then GPU `>=`,
then disk free `<=` minimum; the first breach returns a reason sentence with the observed
value and threshold. The check runs BEFORE the first model call and BEFORE every turn's model call
(test: memory 97% with the default threshold stops the run before any backend call with
`resource_exhausted`). **Concern:** with default thresholds a busy workstation running the model
itself (CPU spikes to 100% during inference on a local GPU/CPU-bound server on the same host) can trip
the CPU threshold between turns and abort work. **Recommendation:** make CPU checking opt-in and
treat the check as advisory (log + escalate only if escalation is enabled; otherwise continue).
Resource trips are also `escalation` triggers (11.1) but are only meaningful when escalation is enabled;
when it is not, they still STOP the run today.

---

## 12. OpenAI-compatible client behaviour

Transport: plain HTTP(S) POST with JSON body to `<base_url>/chat/completions`; headers
`Authorization: Bearer <api_key>` and `Content-Type: application/json`. No retries, no backoff, no
proxy configuration beyond the platform default, no connection reuse requirement.

### 12.1 Request body

`model`, `messages` (2.5 shape), `temperature`, `stream` (bool). If the tools list is non-empty: `tools`
and `tool_choice: "auto"`; if empty, BOTH are omitted (some servers reject `tool_choice` with an empty
tools array - a regression-driven rule, keep). When streaming: `stream_options: {"include_usage": true}`.
If a structured-output schema is configured: `response_format: {"type": "json_schema", "json_schema":
{"name", "schema", "strict"}}`. Not sent: `max_tokens`, `top_p`, seed, stop, parallel_tool_calls,
reasoning/thinking controls, `chat_template_kwargs`.
Consider adding (new, optional, per-model flags): `max_tokens`, `reasoning_effort`, and a way to pass
`extra_body` for server-specific options like vLLM `guided_*` or Ollama `num_ctx`.

### 12.2 Non-streaming response parsing

- Body must be a JSON object; else error "invalid JSON" / "malformed JSON payload".
- `choices` must be a non-empty list whose first entry is an object with a `message` object; only
  choice 0 is used. Errors: no choices; malformed choice; no assistant message.
- `content`: null -> `""`; string as-is; a list is flattened: strings kept, `{"type":"text","text"}`
  and any dict with a string `text` contribute that text, other items are serialised to ASCII JSON
  and appended, all concatenated with no separator; any other type -> its string form.
- `finish_reason`: kept as string; non-string -> stringified; null stays null (then `stop_reason` null).
- `usage`: see 12.4.
- Tool calls: from `message.tool_calls` (list). Each must be an object whose `function` (default empty
  object) is an object with a non-empty string `name` (else error "tool call missing function
  name"); `id` is kept if a non-empty string, else synthesised as `call_<index>`. `arguments`: absent/null
  -> `{}`; a dict is accepted as-is (some servers return parsed objects); a string is trimmed, empty ->
  `{}`, otherwise parsed as JSON and MUST be an object (else error naming the bad payload); any other
  type -> error. Legacy `message.function_call` (single object) is accepted when no `tool_calls`
  list is present: id fixed to `call_0`.
- `raw_message` retains the original provider message object.
**Local-model quirks to handle in the rewrite (not handled today; each is an observed failure class):**
1. Tool calls emitted as TEXT in `content` instead of `tool_calls` (Qwen2.5/3 `<tool_call>{...}</tool_call>`
   blocks, Hermes-style, Llama `<|python_tag|>`, bare JSON objects with `name`/`arguments`) when the
   server lacks the matching tool parser. Today this ends the run with the raw text as the answer.
   Recommended: a post-parse fallback that, when `tool_calls` is empty and `content` contains a
   well-formed call for a known tool name, converts it (and strips the text); make it opt-in or well-tested.
2. Reasoning text: servers differ - `<think>...</think>` inline in `content`, or a separate
   `reasoning_content`/`reasoning` field on the message (vLLM, DeepSeek-style, Ollama thinking). Today inline
   think text stays in `content` (and is returned to the user and re-fed to the model) and separate reasoning
   fields are ignored. Recommended: strip inline think blocks from stored/replayed assistant content, optionally
   surface reasoning separately, and never send it back.
3. Malformed argument JSON from small models (trailing commas, single quotes, unescaped newlines, truncated
   string when `finish_reason=length`). Today it is a fatal `backend_error`. Recommended: attempt a lenient
   repair, and if still invalid return a failed TOOL RESULT to the model explaining the parse error (so it can
   retry) instead of aborting the run.
4. Duplicate or empty tool-call ids (Ollama has returned the same id for every call; some servers return `""`):
   ids must be made unique per assistant message (a synthesised id keyed by position), or the duplicate-id guard of 5.5 will
   wrongly swallow legitimate calls (the guard compares across the whole run).
5. Missing usage: many servers omit `usage`, or omit it in stream unless `stream_options` is supported. Totals then
   read 0 and token budgets never trip; fall back to the local estimate (9.2) when usage is absent.
6. Several tool calls in one message with parallel semantics; `tool_choice: "auto"` is the only mode used.
7. `finish_reason` of `tool_calls` with an empty `tool_calls` list, or `stop` with tool calls present (llama.cpp):
   dispatch on the presence of parsed tool calls, never on the finish reason (the existing loop already does).
8. Non-OpenAI error bodies (Ollama `{"error": "..."}`) - surface the text.

### 12.3 Streaming (server-sent events)

Request as 12.1 with `stream: true`. Parsing is line-based: lines starting with `data:` have the prefix removed
and are accumulated; a BLANK line ends an event: the accumulated lines are joined with newlines; the payload
`[DONE]` terminates the stream; anything else must parse as a JSON object, else an error ("invalid JSON in
streaming response"). A trailing event without a final blank line is also processed at EOF. Comment lines
(starting with `:`) and other field names (`event:`, `id:`, `retry:`) are ignored (lines not starting `data:`
are dropped). Bytes decode as UTF-8 with replacement. Event translation per payload, in this order:
1. If the payload has a usage object whose total is non-zero: emit a `usage` event (with the payload as raw).
2. For each choice (all entries, not only the first): `delta.content` non-empty string -> `content_delta`;
   each `delta.tool_calls` entry (objects only) -> `tool_call_delta` with `index` (int, else 0), `id` (string or
   null), `function.name` (string or null), `function.arguments` (string or empty); a non-null `finish_reason`
   (stringified if needed) -> `message_stop`.
A `message_start` event is emitted when the response is opened, before any payload. Usage across the stream is the
SUM of all `usage` events (servers that repeat cumulative usage in every chunk will be over-counted;
**recommendation:** take the last non-zero usage rather than summing). Because content is accumulated in place the
streamed text is available to the caller incrementally (CLI/GUI print deltas as they arrive; the loop itself only
needs the final message).
Servers that omit the final usage chunk simply contribute zero.
Quirks: `delta.content` may contain a reasoning prefix; `delta.reasoning_content` is ignored (see 12.2 item 2);
tool-call name fragments are assigned not concatenated (a server that streams the name in pieces breaks - rare);
HTTP keep-alive comment lines are harmless.

### 12.4 Usage field mapping

`input_tokens` = first NON-ZERO of: `input_tokens`, `prompt_tokens`, `prompt_eval_count` (Ollama native).
`output_tokens` = first non-zero of `output_tokens`, `completion_tokens`, `eval_count`.
`cache_creation_input_tokens` and `cache_read_input_tokens` read from the same-named keys only
(OpenAI's `prompt_tokens_details.cached_tokens` is NOT read; **recommendation:** map it to cache-read).
`reasoning_tokens` = top-level `reasoning_tokens` if non-zero, else `completion_tokens_details.reasoning_tokens`.
Number coercion: ints as-is, floats truncated, numeric strings parsed, anything else (including booleans and
unparseable strings) -> 0. A non-object `usage` -> all zeros.

### 12.5 Errors and exception classes

A single error type for backend problems. Messages (wording free): HTTP status errors include the status code and the
response body text (UTF-8, replacement on bad bytes) - this body text is what the prompt-too-long
heuristics (7.6) match against, so the body MUST be included; connection failures include the base URL and the reason;
invalid or malformed JSON; no choices; malformed choice/message; bad tool-call shapes; invalid tool-argument JSON.
There is NO distinction between retryable and fatal errors and no timeout-specific error today beyond the
transport's (a timeout during `urlopen`/read surfaces as an unhandled socket timeout rather than this error type
in the non-streaming read path: **DO NOT REPLICATE**, wrap timeouts and treat as `backend_error`).
All backend errors in the loop end the run as `backend_error` (after at most the one reactive-compaction retry),
with the error text as `final_output`.
**Recommendation:** retry once on connection reset/5xx/429 with a short delay before giving up; escalation (11) then
becomes the last resort.

### 12.6 Streaming-mode loop caveat (DO NOT REPLICATE)

In streaming mode, tool-call arguments are parsed with a plain JSON decode OUTSIDE the error type, so a malformed
argument string raises a raw JSON decode error that is NOT caught by the loop and crashes the run (and the in-flight
assistant message is already in the session). Intended: same handling as 12.2 item 3 in both modes.

---

## 13. Hooks into the original (non-derived) modules

These modules are original and are specified (where needed) by their own specs. Where the loop touches them:

### 13.1 `memory_runtime` (agent-curated memory)

- Construction: each agent builds a memory runtime for its cwd; `cwd_update` rebuilds it; after any tool
  whose name starts `memory_` succeeds the loop reloads it and clears the context caches (5.5 step 10).
- Context assembly (8.2): the `curatedMemory` user-context key is the memory runtime's rendered prompt block
  (empty -> key absent; never produced when CLAUDE.md discovery is disabled).
- Ranked injection: `render_relevant_prompt_block(first_prompt)` should replace that block at session creation
  (currently dead, 8.6). Behaviour recap: if the combined number of entries across the two scopes is <= 8, output
  equals the full block; otherwise rank entries by count of shared lower-cased `[a-z0-9']+` words with the prompt,
  keep the top 8 in ORIGINAL order, and mention how many were omitted.
- Model-callable tools `memory_add`, `memory_replace`, `memory_remove`, `session_search` are registered in the tool
  registry and receive the memory runtime via the tool context; there is intentionally no read tool.
- Chat exit hook (`agent-chat`/`voice-chat`, default on, `--no-auto-memory-review` disables): after `/exit`, `/quit`, EOF or
  interrupt, if a session id exists, load the saved session file, call the memory-review proposer on its messages with the
  agent's client, stage proposals in the review queue and print one status line (`memory_review: proposed N entry/entries
  (P pending, B blocked) -- run 'sarina memory-review-list' ...`); any exception is swallowed and reported as
  `memory_review_skipped=error (...)`. The loop function's own parameter defaults to OFF; the CLI turns it on. Nothing is
  written to memory automatically; approval is a separate command. Exit codes: 0 for normal exit/EOF, 130 for interrupt.

### 13.2 `session_search_fts`

Provides the always-available lexical `session_search` tool and the `session-search` command over saved session files
(it reads the 4.2 format: `messages[].content` and `session_id`). The core must keep that format stable (or update this
module in the same change). No loop hook other than tool registration.

### 13.3 `memory_security`

Every memory write (tool call or approved review proposal) goes through one scan: it blocks entries containing invisible/
formatting Unicode (zero-width, bidi controls, tag characters, any `Cf` category), injection-style instruction phrases,
and credential/secret shapes; then (13.4) consults Sentry. Specified in the memory spec; the core only needs to call it
through the memory runtime and not bypass it.

### 13.4 `sentry_governance`

Opt-in via `SENTRY_REVIEW_URL` (full URL, e.g. `http://127.0.0.1:8930/v1/review`). One POST per memory-text scan with JSON body
`{"kind": "memory_proposal", "session_id": "", "text": <entry>}`, 2-second timeout. Response JSON object with `decision`
(`"deny"` means deny; anything else allow) and `reasons` (array of strings). Fails OPEN on any error/absence/non-object
(returns no verdict); fails CLOSED on an explicit deny (the memory write is rejected with a message including the reasons). The
core loop does not call it directly; the call site count must stay at exactly one (the memory scanner).
Note: `session_id` is always sent empty from the memory path today.

### 13.5 Chymaera_Sentry polling and the HTTP service

- Sentry: see 4.4 (decision needed on the file shape).
- The HTTP service constructs agents with an optional `tool_allowlist` (5.7) and uses `run`/`resume` and the session pool;
  its endpoints are another spec. Relevant core requirements: an agent object per session, reusable across multiple
  chat turns via `resume`; `tool_allowlist=[]` must mean zero tools (Sentry's advisory sessions rely on that).

---

## 14. Acceptance scenarios derived from the tests (core behaviours to re-implement as Go tests)

Each of these exists today as an offline test using a fake backend:
1. Two-response script: turn 1 returns assistant text plus one `read_file` call (`finish_reason: tool_calls`), turn 2 returns
   text (`stop`). Result: `final_output` is turn 2's text, `tool_calls == 1`, transcript has >= 5 entries
   (system, context reminder, user, assistant, tool, assistant).
2. Run then resume with a new prompt: the second request's message list contains the first prompt, the first answer and the new
   prompt; same `session_id`; file exists at `<session_dir>/<id>.json`; loading it gives the stored form.
3. Streaming text over three SSE events with usage on the last: `final_output` joined, usage mapped from `prompt_tokens`/
   `completion_tokens`, events include `content_delta`, scratchpad directory exists, the single assistant message's
   `mutation_totals` as in 3.4.
4. Streaming tool call split across two fragments: arguments reassembled; usage summed across both model calls (9+11
   input, 4+2 output in the test); the assistant message carries tool calls and `assistant_tool_call_delta >= 2`.
5. Transcript blocks: assistant-with-call has `text` and `tool_call` blocks; the tool message's first block is `tool_result`.
6. `max_total_tokens=20` with a response using 42 tokens: `stop_reason=budget_exceeded`, output mentions the token budget,
   `usage.total_tokens == 42`.
7. Pre-flight hard overflow with a backend that raises if called: `stop_reason=prompt_too_long`.
8. Truncation continuation: `finish_reason=length` then `stop`: final output is the concatenation `"Part 1 Part 2"`, one
   `continuation_request` event, the second request contains the continue reminder.
9. Model-call budget carried across resume: with `max_model_calls=1`, the first run answers normally, the resumed run
   returns `budget_exceeded` mentioning the model-call budget.
10. Response schema: the first request's `response_format` equals the json_schema object of 12.1.
11. Duplicate tool-call id across two turns: executed once, run completes, `tool_calls == 1`.
12. Escalation: disabled -> `backend_error` unchanged, no history entry; enabled + CLI ok -> `escalated_completed`, final text
    replaced, one history entry, child agent labelled `claude_code_escalation`; CLI missing -> unchanged and not launched; CLI fails ->
    `backend_error` with one failed history entry; budget stop not escalated by default; resource trip escalates; escalation cap
    persisted (`budget_state.escalations == 1`) and respected on resume.
13. Local slash command (e.g. permissions): `turns == 0`, `tool_calls == 0`, output contains the report, no backend call.
14. Compaction at threshold 80 / preserve 1: see 7.4.
15. Client parsing: tool-call response -> one call with parsed arguments; usage aliases (OpenAI, Ollama, nested reasoning);
    content normalisation; JSON-string numbers; invalid arguments rejected; join-URL behaviour with and without slashes.
16. Escalation helper: never a bypass flag; write/shell permissions -> accept-edits; malformed/non-object/timeout/OS-error from the CLI
    never raise; the command contains the verified flag set.
17. Resource evaluation: order memory > CPU > GPU > disk; disabled thresholds never trip; GPU check works with memory unavailable;
    GPU parsing of the first line of the smi output.
18. Config store: merge order, set-then-read, update in place without re-reading other sources.
19. Tokenizer selection: GPT names prefer tiktoken-like, an env override can pick the HuggingFace-like backend, all-fail falls back to the heuristic.

---

## 15. Findings and decisions for the owner

1. **Sentry file-format mismatch** (4.4): the integration reads a shape the agent does not write. Needs a decision before freezing the Go format.
2. **API key persisted in cleartext** in every session file (4.2). Stop persisting it.
3. **Dead code to leave out**: microcompact (7.8), session-memory compaction (7.9), curated-memory ranking in the live path (8.6, fix rather than port), the port-context module, the legacy flat session writer unless 4.4 option (b) is chosen.
4. **Correctness bugs not to copy**: tool-call/result orphaning on budget stop (5.5.3) and on compaction (7.4), counter double-counting on resume (5.9), uncaught argument-parse error in streaming mode (12.6), socket timeouts escaping the error type (12.5), dynamic-boundary sentinel leaking into the prompt (8.1), never-resetting circuit breaker (7.7), usage summed across repeated stream usage chunks (12.3).
5. **Local-model robustness gaps worth fixing in the rewrite** (12.2): text-embedded tool calls, think blocks, lenient argument repair with error-back-to-model, id uniqueness, missing-usage estimation, configurable context window (9.3) and `max_tokens`.
6. **Resource thresholds** can abort a healthy local run on the same host as the model (11.2); make advisory.
7. Escalation sends task context to a remote service; keep it off by default and visible.

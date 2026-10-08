# Go core: status

Module `github.com/Artifex-Technologies/Sarina/go` (Go 1.27, standard library only).
Written clean-room from `docs/spec/README.md`, `00-must-preserve.md`, `01-agent-core.md`
and section 1 of `02-tools.md`. Nothing under `sarina/` or `tests/` was read.

## Layout

| Package | What it holds |
| --- | --- |
| `config` | model/runtime config, env handling (`OPENAI_*`, `SARINA_CONTEXT_WINDOW`), tolerant loaders |
| `llm` | OpenAI-compatible client (non-stream and SSE), usage mapping, lenient JSON repair, local-model normalisation |
| `session` | message model, secret redaction, session file (atomic write, tolerant load), pairing repair |
| `tools` | registry, allowlist filter, `Builder` (re-applies allowlist on every rebuild), `RegisterProvider`, `read_file` |
| `agent` | turn loop, budgets, snip + heuristic compaction, pre-flight, reactive retry, resume, prompt/context assembly |
| `cli`, `cmd/sarina` | `chat` (one-shot or interactive, Ctrl-C) and `resume`; `cli.Register` adds subcommands by name |

## Done

- Session file exactly as spec 01 section 4.2 (key order, ASCII-only, 2-space indent, objects in `messages`).
  The API key is never written; the loader ignores one if present. Written via temp file + rename.
- All DO NOT REPLICATE items in spec 01 that fall in this scope: orphaned tool calls on budget/interrupt stops
  (every call is always answered), compaction splitting call/result pairs, resume double counting
  (`tool_calls`, real `model_calls`), streaming argument-parse crash, timeouts escaping the error type,
  dynamic-boundary sentinel leaking into the prompt, `.port_sessions` anchored at the workspace.
  Also: pairing repair of old/dangling files on resume, usage taken as last chunk (not summed),
  `max_output_tokens` forwarded as `max_tokens`, tool specs counted in the pre-flight, one retry on 5xx/429/connect errors.
- Local-model handling: text-embedded calls (`<tool_call>`, `<|python_tag|>`, bare/fenced JSON, only for tools that exist),
  `<think>` blocks and `reasoning_content` split out and never replayed, lenient argument repair with errors returned to the
  model as a failed tool result, truncated arguments refused when the stop was `length`, empty/duplicate ids made unique per
  message, missing usage estimated (flagged `usage_estimated`), streamed parallel calls that reuse index 0.
- `tool_allowlist`: nil = all tools, non-nil (even empty) = exactly those; empty means zero tools and the request omits
  `tools`/`tool_choice`. `cwd_update` rebuilds the registry through the same builder.
- Tests: scenarios 1-11, 14 and 15 of spec 01 section 14, plus a test per bug fix above (`go test ./...`).

## Deliberate deviations

- The heuristic compaction boundary note has role `user` (wrapped in a reminder), not `system`: several local chat
  templates reject a system message after the first. Its `kind`/metadata are as specified.
- Tool-call counting after a budget stop in the pre-flight path reports the cumulative count, not 0.
- Token counting is the chars/4 heuristic only (no calibration against `prompt_tokens` yet).

## Missing (not in this slice)

- Scenarios 12 (escalation), 13 (slash commands), 16-17 (escalation helper, resource checks), 18 (layered
  `.claude/settings.json` store), 19 (tokenizer selection: Go uses the heuristic). Config fields for escalation are
  persisted for format compatibility but nothing acts on them. No resource-pressure check.
- Full tool set (write/edit/bash/glob/grep/web, task/plan/memory tools), hook policy, plugins, sub-agents.
- Memory runtime, Sentry governance call, `session_search`, curated-memory ranking at session start.
- Limbo tools/commands (the `tools.RegisterProvider` and `cli.Register` hooks are ready for them).
- HTTP service (port 8899), GUI, fleet commands, `agent-resume` style flag parity beyond what `chat`/`resume` expose.

## Core tool set (status)

Written clean-room from `docs/spec/02-tools.md` (all of it), `00-must-preserve.md` and the Go code above. Nothing under
`sarina/` or `tests/` was read. Standard library only. Wired into `tools.Builtins()` (so into every default registry);
the CLI flags `--allow-write`, `--allow-shell`, `--unsafe` already feed `tools.Context.Permissions`.

| Tool(s) | Needs |
| --- | --- |
| `read_file` (`start_line`/`end_line`, `N: ` prefixes), `list_dir`, `glob_search` (`**`), `grep_search` (RE2), `TaskOutput`, `task_list` (also `task_id` detail and `actionable_only`), `plan_get` | nothing |
| `write_file`, `edit_file`, `task_create`, `task_update` (status transitions, dependency edits), `todo_write`, `update_plan` (also `clear`) | `--allow-write` |
| `bash` (also `run_in_background`, `timeout_seconds`), `TaskStop` | `--allow-shell` (destructive commands also need `--unsafe`) |

Design points worth knowing:

- **One gate.** `Tool.Requires` (`PermNone/PermWrite/PermShell`) is checked in `Registry.execute` before the handler and
  before schema validation, on both `Execute` and `ExecuteStream`. A test fails if a tool is added without an explicit
  decision. Denials carry `error_kind=permission_denied` on every path, streaming included; the agent turns them into
  `tool_permission_denial` events.
- **Schema enforced centrally** (`tools.ValidateArgs`): required, type, enum, `minimum`/`maximum`, `minLength`, nested
  arrays/objects. Numeric strings and "true"/"false" are coerced (local models send them); a JSON boolean is never a number.
- **Secret-file guard** (`secret.go`) applies to read, edit, **write**, and grep (path = refuse, directory walk = skip
  silently). Case-insensitive, either separator, trailing dots/spaces and `:stream` suffixes ignored, checked on both the
  given and the symlink-resolved path, plus same-directory hard links. `list_dir`/`glob_search` show names only. `bash` is
  deliberately not covered (explicit intent), as in the spec. Writes resolve through the deepest existing ancestor, so
  symlinked directories and dangling links cannot leave the workspace.
- **Destructive check** (`destructive.go` on `tools/shellparse`): a real shell reader (separators, subshells, `$()`,
  backticks, process substitution, heredocs, quoting incl. `$'..'`, `-c` strings, here-docs fed to shells), env prefixes, and
  wrappers (sudo, env, command, nice, timeout, xargs, find -exec, su -c, ...). Program names are matched by basename; a name or
  script built at run time is refused rather than guessed. Also: `curl|sh`, fork bombs, redirect/tee targets outside the
  workspace or on devices, git history-destroying subcommands, recursive chmod/chown, power control, inline interpreter
  code that touches files/processes (heuristic token list). It is a guard-rail, not a sandbox: ssh and unknown programs
  are not inspected; real boundaries are `--allow-shell` and Limbo.
- **Child environment:** parent env minus any name containing SECRET, TOKEN, PASSWORD, PRIVATE_KEY, API_KEY, CREDENTIAL, AUTH
  (substring, case-insensitive), then `Context.SessionEnv`, then `Context.ExtraEnv` (operator layers, unfiltered). Values are
  never put in results or metadata. Nothing sets `SessionEnv`/`ExtraEnv` yet (hook policy and `/env` are not built).
- **Processes:** every shell runs in its own process group (Windows: new group + `taskkill /T`); timeouts and cancellation kill
  the whole tree; finished foreground commands are reaped so they leave no strays. `bash run_in_background` starts a
  detached process in a `ProcManager` (max 16); `TaskOutput` waits/peeks at its real output, `TaskStop` terminates the tree
  (SIGTERM, then SIGKILL after 2 s) and records `killed`. These are separate from the to-do `task_*` tools, and errors say so.
  The default manager is process-wide; `cli` stops all of them when a conversation ends. They are not persisted across restarts.
- **To-do/plan:** `.port_sessions/task_runtime.json` and `plan_runtime.json`, atomic writes, tolerant loads. `update_plan`
  merges into tasks by id and never deletes (`sync_tasks` default true; `prune_tasks` removes only plan-created tasks);
  `update_plan` with `clear` leaves tasks alone unless `prune_tasks`, then removes only plan-created ones (the old `plan_clear`; see notes/todo-tools-merge.md). `todo_write` still replaces the whole
  list (its stated purpose; the tool description says so) but keeps the id/created_at of a task with the same title.
  Also fixed: duplicate ids refused, unknown `task_id` in `task_list` is an error, plan steps numbered among kept steps, a task blocked by a
  reason only is not released by unrelated completions.
- **Guards:** edit-loop (6th attempt per path refused, failed edits count), summary-markdown churn (5th refused), self-authored
  banners on `read_file` and `bash`. State lives in `Context.Runtimes`, so it resets when the agent rebuilds the context.
- **Agent change:** with `stream_model_responses`, `Registry.ExecuteStream` is used so bash stdout/stderr arrive as live
  `tool_delta` events (`stream` = stdout/stderr); other tools are replayed in 400-char chunks as before. `read_file` no longer
  returns `path` metadata (the agent records any result with a `path` as a file change); two agent tests that used it as a
  stand-in now use `write_file`.

Tests: `go test ./...` and `go vet ./...` pass on Windows (Git Bash) and on Arch under WSL (also with `-race` for
tools/agent/cli). Unix-only tests (grandchildren killed on timeout/TaskStop, no strays) live in `alive_unix_test.go`; bash
tests skip if no bash is found. Adversarial coverage: ~400 destructive/allowed command lines in `destructive_test.go`,
secret-guard bypasses (case, `..`, absolute, symlink, hard link, write/edit/grep, directory symlink escape, dangling link) in
`core_test.go`, schema bounds, env-filter and process-kill tests in `bash_test.go`.

Not built here (by instruction): plan mode, skills, MCP, memory, sub-agents, web, notebook, sleep, LSP, account/remote/team/
workflow tools, hook policy, `/env`, `ask_user_question`, worktree tools, `tool_search`.

## Plan mode and skills (status)

Written clean-room from `docs/spec/06-new-features.md` sections 1 and 2, the dispatch notes in `04-cli.md`, and the Go
code above. Nothing under `sarina/` or `tests/` was read. Standard library only. `go vet` and `go test ./...` pass.

**Plan mode** (`agent/plan.go`, `tools/plan.go`, `session/plan.go`, `cli/plancmd.go`)

- State lives in the session file under the existing `plugin_state` object, key `plan_mode`
  (`{"mode","pending","note"}`). No top-level key was added, and a normal session with no note writes nothing, so its file
  is byte-identical to before.
- `Tool.Class` (`ClassReadOnly` / `ClassMutating` / `ClassPlanControl`) is the explicit per-tool decision. The zero value is
  "undecided" and is treated as mutating. `TestEveryToolHasAnExplicitPlanModeClass` fails for any tool in `Builtins()`
  with no class or no row in its table.
- `Builder` applies, in order: allowlist, then `Narrow`, then `PlanMode`. All three only remove tools, so the allowlist
  always wins and an empty one stays empty. Tools are not offered in the request at all; a call to a withheld tool is
  "unknown tool". The agent refreshes the registry (not the whole tool context) whenever the mode changes, so a call
  after `enter_plan_mode` in the same model turn already sees the filtered registry. `rebuildTools` re-applies it.
- `exit_plan_mode` only submits. The only way back to normal is `Agent.applyDecision` (approve/revise/reject), reached from
  the interactive prompt, `/plan approve|revise|reject`, or `resume --approve-plan|--revise-plan <text>|--reject-plan`.
  `approve`/`revise` need a submitted plan; `reject` always works. `exit_plan_mode` is not offered outside plan mode.
- Non-interactive: with no decider the run ends with stop reason `plan_pending` (`Result.Plan`, `FinalOutput` = the plan),
  every remaining call in that model turn is answered "not run". The CLI exits with code 3 and prints the resume hint.
  Interactive chat reads the decision from stdin; end of input or Ctrl-C leaves the plan pending (never approves).
- A decision made outside a turn is appended to the transcript as a reminder message, so the approved plan (kept as
  `note`) is in the next turn's context. Not yet re-injected after compaction (session notes, spec section 3, are not built).

**Skills** (`skills/`, `cli/skillcmd.go`, `cli/slash.go`)

- Front matter is a small hand-written subset of YAML (flat keys, quotes, bools, inline/block/comma lists); no dependency.
- Search path: `~/.sarina/skills`, `SARINA_SKILLS_DIR` (PATH-style list), `paths` in `~/.sarina/skills.json`, and only with
  `--trust-workspace-skills`, `<cwd>/.sarina/skills` last (it can never shadow an operator skill). The flag prints a
  `[notice]` in chat/run and a `NOTICE:` line in `skill list`. Files: `name.md` or `name/SKILL.md`; symlinks, dotfiles,
  `README.md` and files over 64 KiB are skipped (the first and last are reported).
- `Scanner` interface plus `BasicScanner` (invisible/bidi `Cf` characters, credential shapes, override phrases). It runs on the
  whole file before parsing; a blocked skill is reported by `skill list` and never loaded. Replace it by passing `Options.Scanner`.
- `skill` tool: built only when some skill is `model_invocable`, lists only those, and refuses the rest. `allowed_tools` is
  reported in result metadata and the agent narrows its registry for the rest of that run (`Agent.NarrowTools`, intersecting,
  cleared when the run ends). `/skill <name> [args]` and `sarina skill run` narrow the same way for that run; `skill list`
  shows name, description, source, path, model-invocable. Collisions are reported (earlier path wins).
- Slash mechanism (`cli.RegisterSlash`): local reply or prompt result, case-insensitive; unknown plain names reply
  "Unknown skill: x", names with other characters go to the model. Only `/help`, `/plan` and `/skill` exist. A prompt result
  sends the expanded text as the user message (the literal `/skill ...` line is not kept as a separate message).
- `cli.Env.Home` overrides the home directory (tests); empty means the real one.

Not done: operator config file schema beyond `paths`; skills are not re-scanned if a file changes after startup; no bundled skills.

## Context management: summarising compaction, session notes, clearing, window discovery (status)

Written clean-room from `docs/spec/01-agent-core.md` (compaction, context assembly, resume), `06-new-features.md`
section 3 and the Go code above. Nothing under `sarina/` or `tests/` was read. Standard library only.
`go vet ./...` and `go test ./...` pass. The summarising and notes prompts are new text.

**Order when the context fills** (cheapest first): size-based clearing, then snipping, then compaction, which itself
prefers fresh session notes (no model call), then a summarising model call, then the heuristic digest (cannot fail).

- **Summarising compaction** (`agent/summarize.go`). The older history is flattened to text (so every backend accepts it,
  whatever the cut points) and sent tool-free with a fresh nine-point-style instruction; `<analysis>` is dropped and
  `<summary>` unwrapped. A prompt-too-long error (or a short reply saying so) drops the oldest ~20% of rounds and retries, up
  to 3 times; the text is also pre-trimmed to the window. Empty replies, errors and summaries rejected by the scanner count as
  failures and fall back to the heuristic digest. **Circuit breaker:** 3 consecutive failures skip the LLM step (event
  `auto_compact_circuit_breaker`); the count lives on the per-call `run`, so it is closed at the start of every Run/Resume
  (spec 01 section 7.7's never-resetting breaker is fixed). Events: `auto_compact_summary`
  (`summary_source` notes|llm, usage, retries), `auto_compact_failed`. One message (kind `compact_boundary`, with the
  usual depth/nesting metadata plus `summary_source`) holds the summary; a separate `compact_summary` message was not kept.
- **Pair-safe cuts** (`pairSafeCut`, shared by every path). The tail never starts on a tool result; the cut moves back to the
  assistant message that made the call. A test sweeps rounds x preserve x every path (heuristic, notes, LLM, LLM failing,
  reactive) and asserts no orphaned call or result.
- **Where LLM compaction runs.** Pre-flight (after clearing and snipping). The operator's explicit
  `--auto-compact-threshold` path stays model-free (fresh notes if any, else the digest) so its documented behaviour is
  unchanged; reactive (backend said "too long") tries fresh notes, then the old reactive reduction. Side calls (notes,
  summaries) are billed to the run: usage added once, estimated if the server reports none, and counted in `model_calls`.
- **Session notes** (`agent/notes.go`). `.port_sessions/agent/<id>.notes.md` with the six fixed headings (task/goal, current
  state, files touched, decisions, open questions, next steps); the model reply is normalised to exactly that shape (fewer than
  3 recognisable sections = failure, sections capped at 2500 runes). Refreshed after a tool-using turn when N turns passed
  (default 6) or, with at least 2 turns since the last refresh, when context use is >= 50% of the window. Coverage lives in
  the session file at `plugin_state.session_notes = {last_summarised_index, last_summarised_message_id}`; the id lets a shifted
  or truncated transcript re-find (or distrust) the index. Any failure emits `session_notes_failed`, resets the counter and
  never fails the turn; the previous notes stay. Compaction uses the notes only if they cover everything that would be folded.
  Notes go through `skills.Scanner` (`Options.Scanner`, default `skills.BasicScanner`) before any re-injection; a blocked file
  is ignored (event `session_notes_blocked`). Stale notes are given to the summariser as context and appended to a heuristic
  digest. Plan notes (`plugin_state.plan_mode`) are untouched.
- **Tool-output clearing** (`agent/clear.go`). At > 60% of the window, tool results older than the latest 4 and larger than
  500 tokens become a one-line marker (tool, size, "cleared"). Role/name/`tool_call_id` stay. Metadata `tool_output_cleared`
  makes it idempotent (survives save/load). Event `tool_output_cleared`. Runs at the top of every turn, before snip/compaction.
- **Config.** `runtime_config.context_management` (`config.ContextConfig`, omitted from the file unless something differs from
  the defaults). Switches: `--no-session-notes`, `--notes-every N`, `--no-clear-tool-outputs`, `--no-llm-compaction`,
  `--no-context-discovery`; env `SARINA_SESSION_NOTES=0`, `SARINA_NOTES_EVERY`, `SARINA_CLEAR_TOOL_OUTPUTS=0`,
  `SARINA_LLM_COMPACTION=0`, `SARINA_CONTEXT_DISCOVERY=0`. A feature is off if either config or env turns it off; numbers
  are config, then env, then default. Percent thresholds/keep/min are config-file only.
- **Context-window discovery** (`llm/discover.go`). Order: `--context-window`, `SARINA_CONTEXT_WINDOW`, then the backend, then
  name inference. Backend probes, 3 s total, first answer wins: Ollama `/api/ps` (loaded model's real context), `/api/show`
  (`num_ctx` parameter, else `*.context_length`), llama.cpp `/props` (`default_generation_settings.n_ctx`), then
  `/v1/models` (`max_model_len`, `context_length`, `context_window`, `meta.n_ctx*`, ...; a single-entry listing is used even if
  the id differs). Cached per base URL + model for 5 minutes, including "server said nothing"; an unreachable server aborts
  the remaining probes. `agent.Options.Discoverer` is nil by default (library callers and tests never probe); the CLI installs
  `llm.DefaultDiscoverer` unless a test injects a client. A discovered value emits a `context_window` event at run start.
  Caveat: Ollama without a loaded model and without `num_ctx` reports the model's trained maximum, which can exceed the
  `num_ctx` it will actually serve; the reactive path still covers that.
- Spec 01 items in scope: usage/model-call double counting on resume, pair splitting, and the breaker were already or are now
  fixed; nothing else in that list touches this code.
- **Deliberate deviations:** one boundary message instead of boundary + summary; the heuristic `compact_boundary` text is
  unchanged; the notes/summary prompts do not follow the old nine-section wording exactly.
- Test files: `agent/context_test.go` (fake servers that tell main/notes/summary calls apart), `llm/discover_test.go`,
  additions to `config/config_test.go` and `cli/cli_test.go`. The CLI test harness sets `SARINA_CONTEXT_DISCOVERY=0` because its
  fake backends count requests.
- Not done: per-section notes size from config, notes for sub-agents, a `/compact` slash command, calibrating the token counter
  against `prompt_tokens`.

## Compute tiers (status)

Written clean-room from `docs/spec/06-new-features.md` section 4, `01-agent-core.md` (escalation, resource thresholds,
session file) and the Go code above. Nothing under `sarina/` or `tests/` was read. Standard library only (Windows memory
via `kernel32` through `syscall`, no cgo). `go vet ./...` and `go test ./...` pass.

**Pieces.** `tiers/` (config, router, guard, resource sampler, wake), `llm/anthropic.go` (Messages API client),
`agent/tiers.go` (`TierTracker` hook), `cli/tiercmd.go` (`sarina tiers status`, `/tier status|up|down`).

- **Config** `~/.sarina/tiers.json` or `SARINA_TIERS_FILE`; never read from the workspace (tested). Per tier: `name`, `kind`
  (`openai_compat` default | `claude`), `base_url`, `model`, `api_key_env` (a NAME; a value that looks like a key is a config
  error), `allow_data` (`"all"` or classes `no_secrets`, `no_captures`), `max_context`, `wake`, `wake_wait_seconds`,
  `enabled` (default true; `false` = never contacted, not even for health), `auto` (default true; false = ask once per
  session). Top level `resource: {max_memory_percent, max_gpu_memory_percent}` (default 90/90, 0 disables). Tier 0 inherits the
  CLI's base URL/model/key when it names none. No file, one tier, or an invalid file => no router at all, exactly the old
  behaviour (an invalid file warns and stays local-only: a broken config never widens where data goes). Defaults for later
  tiers when `allow_data` is omitted: `no_secrets` (Claude also `no_captures`). A later tier never receives the local key.
- **Router** is the agent's `Completer` and `TierTracker`. Per model call it walks tiers from the current one: skip if disabled,
  unhealthy (GET `/models`, 2 s timeout, cached 30 s; Claude: free `GET /v1/models`, 401/403 = unhealthy), resource pressure
  (tier 0 only), or the request (chars/4 estimate) exceeds `max_context`. On a send failure it moves up on connect errors,
  502/503/504 and on a second consecutive timeout; other errors (400, prompt-too-long) are returned untouched so the agent's
  reactive compaction still works. After a stream has started it never switches. Within a turn it stays on the tier reached;
  `BeginTurn` resets to the lowest (de-escalation), so the lowest healthy tier is re-chosen on the next call.
- **Resource pressure is advisory** (spec 01 section 11.2 concern): if no higher tier can take the request the run continues on
  the pressured tier with a `tier_notice` line instead of aborting. CPU is not checked at all.
- **Failure** with nothing usable: backend error `no compute tier could run this request (local: ...; desktop: ...)` plus
  "Nothing was sent to any tier." when no request body reached any transport. A request too big for every tier says
  "context window", so the agent's reactive compaction kicks in.
- **allow_data guard** (`tiers/guard.go`): `no_secrets` redacts credential shapes (session redactor rules + bearer tokens,
  `key=value` assignments, PEM blocks, github/gitlab/hf/aws/sk- shapes, and the literal key values the process holds) in a COPY of
  the request, so the session keeps the original; `no_captures` refuses the tier when the conversation holds a Flipper
  capture dump (`Filetype: Flipper`, `RAW_Data:`, ATQA/SAK/UID/Block lines). Refusal is a visible `tier_refused` line.
- **Operator visibility.** One line per escalation on stderr: `[tier] escalated local -> desktop: <reason>`, also
  `tier_refused`, `tier_redaction`, `tier_wake`, `tier_notice` events (in `Result.Events`).
- **Session file.** `plugin_state.compute_tiers = {current_tier, highest_tier, highest_tier_index, escalations: [events],
  redactions: {tier: n}}`, merged across resumes, capped at 200 events, absent entirely for a session that never left tier 0
  (file byte-identical). Existing loader tolerates it (plugin_state is opaque). `budget_state.escalations` is untouched (it
  counts the old external-CLI escalation, not built). Costs are still priced with the local model's pricing.
- **`/tier up`** (operator only, never a model tool): the NEXT turn starts one tier above where the last one ended, for that
  turn only; typing it counts as the operator's consent for an `auto: false` tier. `/tier down` cancels it. `auto: false`
  asks via stdin (one-shot) or the interactive line channel; no way to ask = refused.
- **Claude client.** System messages fold into `system`; assistant `tool_calls` become `tool_use` blocks, tool results become
  `tool_result` blocks merged into one user message with following user text; tool ids sanitised consistently; unparseable
  arguments become `{}`; `stop_reason` mapped to OpenAI finish reasons; thinking blocks go to `Reasoning`. Not streamed on
  the wire (replayed as events when streaming is requested). Key only in the `x-api-key` header and `ModelConfig.APIKey`
  (never serialised); scrubbed from every error and event; tests grep all files, events, transcript and errors for it.
- **Wake** only for a tier with a `wake` entry that is down, once per 2 minutes, then polls health up to `wake_wait_seconds`
  (30). http(s) URL = GET; anything else is a shell command that runs only with `--allow-shell` and, unless `--unsafe`, only if
  `tools.CheckDestructive` passes; child env is the parent's minus secret-looking names (so never the API key).
- **CLI.** `sarina tiers status` shows config path, each tier (health, endpoint, allow_data, key env set/not set, never the
  value) and the resource sample; sends only health requests.

Tests: `tiers/scenarios_test.go` (every spec acceptance scenario against `httptest` fakes: local down -> desktop; all down +
Claude disabled -> clear failure and zero requests anywhere; secret not sent; escalations in the session file; key in no file or
event; tools/permissions unchanged; de-escalation; 30 s health cache; context fit; pressure; timeouts; /tier; auto:false; wake),
`tiers/unit_test.go`, `llm/anthropic_test.go`, `cli/tiers_test.go`.

Not done: streaming from Claude on the wire, per-tier pricing, memory pressure on macOS (unknown never trips), a `tiers` entry
in the GUI/service, per-session escalation cap, escalation of sub-agent calls (none exist yet).

## Memory, shared scan gate, Sentry, session search, Limbo (status)

Ported from the owner's ORIGINAL modules (`memory_*`, `session_search_fts`, `sentry_governance`, `analysis_cache`,
`chymaera/limbo*`) and their tests, plus specs 00, 01 section 13 and 02 sections 1, 11, 12. No derived file under
`sarina/` or `tests/` was read. Standard library only, no cgo. `go vet ./...` and `go test ./...` pass.

| Package | What it holds |
| --- | --- |
| `guard` | THE one scan gate: `Gate.Scan` (invisible/bidi/Cf Unicode, injection phrases, credential shapes) returning `guard.Result`. `guard.Memory()` also consults Sentry; `guard.Default()` (no Sentry call, tolerates a leading BOM) is what `skills.BasicScanner` now delegates to. `skills.Scanner`/`ScanResult` are aliases of the guard types, so skills, session notes and compaction summaries all use this one implementation. The pattern lists exist nowhere else. |
| `sentry` | `Consult` (POST `$SENTRY_REVIEW_URL`, 2 s). nil on unset/unreachable/non-2xx/non-object (fail open); `Denied` verdict closes. Its only caller is `guard.Gate`, and only when `SentryKind` is set (memory). |
| `memory` | Curated memory runtime (same on-disk files: `.port_sessions/memory/AGENT_MEMORY.md`, `~/.sarina/memory/USER_MEMORY.md`, entries split by `\n---\n`, 1500/4000 limits, whitespace-collapsed duplicate check, unique-substring replace/remove, scan on every write, atomic writes), review (propose/queue/approve/reject; `pending_review.json` has the Python shape, ASCII JSON; scanned at proposal and again at approval), tools `memory_add/replace/remove` (write-gated, mutating) and `session_search` (read-only), registered through `tools.RegisterProvider` in `init()`. No `memory_read`. |
| `sessionsearch` | Lexical search over saved sessions (new Go file format, read leniently: only `messages[].role/content`). No SQLite: literal-phrase match ranked with a small BM25 (lower is better, as FTS5), fallback to term counts when the phrase is nowhere. Snippet rule is the original (160 chars each side). |
| `limbo` | Client for `agent.sock` (unix socket HTTP; refuses any other basename; control chars stripped, 16 KiB cap, untrusted banner, run id must match `r[0-9a-f]{8}`), tools `limbo_profiles` (read-only) / `limbo_validate` (`PermShell`; `timeout_seconds` clamped to 10..900 by the handler, so the schema carries no min/max), commands `limbo-profiles` / `limbo-validate [--timeout S] PROFILE COMMAND...` (options before the profile, everything after is the guest command, a leading `--` after the profile is dropped; exit 0 only for a successful run). `cmd/sarina` imports it for its side effects (tools + commands). |
| `analysiscache` | `{sha256(content): value}` JSON cache with a namespace fingerprint; same file shape as the Python one. No consumer yet (the desktop-audit classifier is not ported). |

**Wiring.**
- `agent`: `newSessionFor(prompt)`; `Run` passes the REAL first prompt. `curatedMemory` renders the ranked block
  (`RenderRelevantPromptBlock`, <= 8 entries over both scopes, original order, "N omitted" note, full block when everything
  fits or there is no prompt) under user-context key `curatedMemory`, only when CLAUDE.md discovery is on. The Python version
  never applied the ranking in real runs; a test now checks the first request really carries only the relevant entries.
- `cli`: commands `memory-status`, `memory-show`, `session-search`, `memory-review <session_id>`, `memory-review-list`,
  `memory-review-approve <id>`, `memory-review-reject <id>`. Exit-time review: `conversation` calls `autoMemoryReview` when the
  INTERACTIVE loop ends (`/exit`, `/quit`, EOF, Ctrl-C) if a session was saved; one tool-free model call through the agent's own
  client (so the tier router applies), proposals staged, one status line, errors printed as `memory_review_skipped=error (...)`
  and swallowed, nothing written to memory. `opts.autoReview` is false at the library layer; `chat`/`resume` set it to
  `!--no-auto-memory-review`. One-shot `chat <prompt>` does not review (the original hooked the loop only). `agent.Options.Home`
  now receives `Env.Home`.
- Existing CLI tests that count backend requests now pass `--no-auto-memory-review` (the CLI default is on).

**Deliberate deviations.** `session_search` and the review transcript skip system messages and `<system-reminder>` envelopes
(every session has the same system prompt and would drown the real hits); session search falls back to term counts when the
whole phrase matches nowhere (the original returned nothing); the scan gate is a union of the Python lists and the old Go
skills scanner, so it is slightly stricter than either (e.g. a `password: x` line in a session note now blocks the note, which
falls back to the heuristic digest); the notes/skills gate does not call Sentry (latency on every refresh), only memory does;
a `timeout_seconds` that is not numeric is rejected by schema validation rather than defaulted to 300.

**Not done.** The tier `allow_data` guard keeps its own redactor (different job: redact vs refuse); sharing patterns would
need a redaction mode in `guard`. No memory-pressure/`session_id` plumbing to Sentry (memory sends it empty, as before).
No `sarina memory-review` of sessions older than the Go format (legacy flat session files are not read).

## MCP client, hook policy, sub-agents (status)

Written clean-room from `docs/spec/03-extensibility.md` (MCP, hook policy, agents; section 12 scope), `02-tools.md` sections 1 and
7.5, `00-must-preserve.md`, the Go code above and the public MCP spec. Nothing under `sarina/` or `tests/` was read. Standard
library only. `go vet ./...` and `go test ./...` pass on Windows and (mcp, policy, agents, agent, cli, tools) on Arch/WSL.

| Package / file | What it holds |
| --- | --- |
| `policy` | deny-list (exact + prefix, case-insensitive), `trusted`, budget limits, `safeEnv` (operator only) |
| `mcp` | `config.go` (trust gate), `client.go` (stdio JSON-RPC), `manager.go` (persistent servers, tool exposure) |
| `agents` | the three read-only profiles and markdown profile loading |
| `agent/subagent.go` | the `agent` tool, child construction, budget, linkage |
| `cli/extcmd.go` | `sarina mcp list`, `sarina agents list` (nothing is started) |

**Hook policy.** Loaded only from `~/.sarina/policy.json` and `SARINA_POLICY_FILE` (PATH-style list). A workspace
`<cwd>/.sarina-policy.json` (that exact file, no ancestors) can only tighten: add denials, lower budgets, set `trusted:false`. Its
`trusted:true`, `safeEnv` and anything that would raise a limit are ignored with a notice; operator denials cannot be removed.
Operator budgets REPLACE the command-line value (they are the operator's override); workspace budgets then lower it. Keys are
snake or camel case, `max_` prefix optional, booleans/strings/negatives rejected with a notice. A malformed OPERATOR file is fatal
(`sarina` exits 2): silently losing a deny-list is worse than stopping. A malformed workspace file is ignored with a notice.
`hooks`, `managedSettings` are not implemented (hooks get a "not supported" notice). `trusted:false` turns off MCP and operator
agent profiles. Enforcement is in the registry (`Registry.deny`, set by `Builder.Deny` last): denied tools are not offered
(`Names`/`Specs`/`Len`) and a call is `permission_denied` before any handler or schema check. Child agents inherit it because they
are built from the parent's builder. `safeEnv` (operator only) exports the named host variables into tool subprocesses through
`Context.SessionEnv`; only names are ever displayed.

**MCP.** Definitions: `~/.sarina/mcp.json`, files in `SARINA_MCP_FILE`; both `mcpServers` object and `servers` array. Workspace
files (`.sarina-mcp.json`, `.mcp.json`, `mcp.json` in `<cwd>` only) are ignored with a `[notice]` naming the file unless
`--trust-workspace-mcp`, in which case a notice says they may start programs; operator definitions win on name clashes. No ancestor
directory is ever looked at (tested, also with the flag). Only `stdio` transport. Server fields: `command`, `args`, `env` (operator
overlay on the filtered child env), `cwd`, `read_only`, `read_only_tools`/`tools.<name>.read_only`, `timeout_seconds`. Children start
from `tools.ChildEnv` (new exported wrapper over the env filter bash uses: secret-looking names removed). One persistent process
per server for the session (spawned at startup for `tools/list`, restarted once, lazily, if it died), handshake and listing 10 s,
tool call 60 s (server-configurable), timeout sends `notifications/cancelled` and keeps the connection; writes never block the
read loop or outlive a timeout; lines over 8 MiB are dropped; ping answered, other server requests get -32601. Close = close
stdin, 1 s grace, then kill the process tree. Tools are exposed as `mcp__<server>__<tool>` (sanitised to `[A-Za-z0-9_-]`, <= 64,
unique), descriptions are control-stripped, capped and run through the shared `guard` scan (withheld if blocked). Every MCP tool is
`ClassMutating` (plan mode hides it) unless the operator marked the server or tool `read_only`; none requires `--allow-write`
(the operator defined the server). `isError` results become failed tool results (`mcp_is_error` metadata). Results are truncated
(head + tail + marker) to the session's output limit. Two generic tools, `mcp_list_resources` / `mcp_read_resource`, exist when a
server is connected and are read-only only if EVERY server is. Not done: prompts, sampling, roots, notifications, HTTP transports,
manifest-declared static resources, per-call approval prompts.

**Sub-agents.** `agent` tool (`profile`, `prompt`, optional `description`, `max_turns`), offered when a catalog exists (the CLI
always gives one), runs serially and in-process. Profiles: built-in `explorer`, `planner` (read tools only) and `verifier` (read
tools + bash, ends with `VERDICT: PASS|FAIL|PARTIAL`, keeps project context), plus `*.md` files in `~/.sarina/agents` and
`SARINA_AGENTS_DIR` (front matter: `name`, `description`, `tools`, `shell`, `max_turns`, `project_context`; scanned by `guard`;
symlinks skipped; never from the workspace; an operator file may replace a built-in). An unknown profile is an error, not a
fallback. The child's tools = the parent's CURRENT registry (so allowlist, skill narrowing, plan mode and policy denials already
applied; an empty allowlist stays empty and the child's request omits `tools`) intersected with the profile list, with only
read-only tools, minus `agent`, to-do/plan/task tools and `skill`; `bash` only if the profile has `shell: true` AND the parent
has `--allow-shell` AND plan mode is off. Children never get write permission or destructive shell, cannot start agents (depth 1;
the tool is not built for an agent with a parent link, and `MaxDelegatedTasks` is 0). Honest caveat: a `shell` profile
can modify files through the shell (as the parent could); that is the one place "read-only" is by instruction. Own budget: the
parent's REMAINING total tokens / reasoning / model calls / cost, tool calls <= min(40, parent remaining), turns = arg > profile >
min(parent max, 6) capped by the parent's `max_turns`. The child's usage is added to the parent run (so parent token/cost
budgets see it), and each call counts as a delegated task (`max_delegated_tasks`, restored on resume from `file_history`).
`Builder.Narrow` set in options now survives the agent's own per-run narrowing (they intersect).
Session linkage: the child session is a normal file in the same directory with `plugin_state.subagent =
{parent_session_id, parent_tool_call_id, profile, depth:1}`; the parent gets a `file_history` entry `history_kind:"delegation"`
with `child_session_id`, and events `subagent_start`/`subagent_end`. Child runs have notes, LLM compaction and context discovery
off and use the parent's client (so the tier router applies). Not done: resume of a child session, parallel runs, dependency
groups, per-profile model override, `/agents` slash command.

Tests: `policy/policy_test.go`, `mcp/{client,config,manager}_test.go` (an in-process fake over pipes, plus the test binary
re-executed as a REAL stdio server for env filtering, persistence, kill-on-close and the "untrusted file spawns nothing" control),
`agents/agents_test.go`, `agent/subagent_test.go`, `cli/ext_test.go`, `tools/deny_test.go`.

## HTTP service and web console (status)

Written clean-room from `docs/spec/README.md`, `00-must-preserve.md`, `05-gui-service.md`, `packaging/arch/*` (read only) and the Go code
above. Nothing under `sarina/`, `tests/` or `sarina/gui/static/` was read; all Go, HTML, CSS, JS and visible text are new. Standard
library only; the page is embedded with `go:embed` and loads nothing from the network (a test greps the assets for network addresses and
for inline script/style, which the CSP forbids). `go vet ./...` and `go test ./...` pass.

| Package | What it holds |
| --- | --- |
| `runkit` | shared by both servers: `Build` (agent from a `Spec`: policy deny-list/budgets, optional tier router, allowlist inside the tool builder), the run-result serialiser (`Result`, `Transcript`, `Entry`), `Scrubber` (literal secrets + session redactor shapes + bearer + `key=value`) |
| `service` | `sarina service` (default 8899): pool, auth, ceiling |
| `gui` | `sarina gui` (default 8765): `server.go` (checks, page), `state.go` (settings), `run.go` (run engine, SSE, approvals), `api.go` (sessions, memory, instruction files, tiers), `static/` |
| `cli/export.go` | `SlashCommands()` and `LocalSlash()` (answers `/help` without a terminal; used by both servers) |

**Core changes (small, additive).** `tools.Context.Approver` + `tools.Approver`: asked after the permission and schema gates and before the
handler, for tools that need write/shell or are not read-only (plan-control tools and read-only tools never ask); a refusal is a
`permission_denied` result. `agent.Options.Approver` feeds it. New agent events `turn_start`, `assistant_message`; `tool_start` now carries
`arguments` and `tool_result` carries `content` (redacted, 20k runes). `cmd/sarina` imports both packages and maps argv[0]
`sarina-service` / `sarina-gui` (also `.exe`) to the commands, so a link or hardlink to the binary under those names works.

**Packaging names (do not edit packaging/ yet).** `sarina-service` = `exec sarina service "$@"` (or a link to the binary named
`sarina-service`); `sarina-gui` likewise. The unit's `ExecStart` flags (`--host --port --session-dir --scratchpad-root`) and the env
(`SARINA_SERVICE_TOKEN`, `OPENAI_*`) are accepted unchanged. Extra, optional flags/env: `--max-sessions`, `--permit-shell`,
`--permit-write`, `--permit-backend-override` (env `SARINA_SERVICE_PERMIT_SHELL|WRITE|BACKEND`), `--cwd-root` (env
`SARINA_SERVICE_CWD_ROOTS`, path list).

**Service.** Contract as spec section 2.3/2.4: `/healthz`, `POST /v1/sessions` -> `{session_id, tools}`, `POST .../chat`, `DELETE`.
`tool_allowlist` is decoded so `[]` stays non-nil, goes into `runkit.Spec.Allowlist` -> `tools.Builder.Allowlist` (so it survives rebuilds),
and `tools` is `agent.ToolCount()`; with 0 tools the request has no `tools`/`tool_choice` (a test asserts it over two turns). Validation:
missing `cwd`, wrong JSON types, empty prompt -> 422 with a `detail` array; bad cwd directory, whitespace prompt -> 400; the 500 body is
`{error, error_type}`. Token compared in constant time (sha256 then `ConstantTimeCompare`), `Bearer` scheme only. The pool is a bounded LRU
(create, or any request that resolves the id, counts as use); an evicted id is a **404** and its text says to resume from disk with
`resume_session_id` (the doc/behaviour mismatch is resolved in favour of the behaviour). Resume ids are pattern-checked
(`^[A-Za-z0-9_-]{1,64}$`) and looked up only in that session's own directory. Turns on one session are serialised; the request context
cancels the run. `/help` is answered locally (200, `stop_reason:"slash_command"`).
*Ceiling (default-deny without a token):* callers may set `allow_shell`, `allow_write`, `base_url`/`api_key` only if the operator permits it
(defaults: permitted iff a token is configured; explicit flags/env override); otherwise **403**, never a silent downgrade. `session_directory`
and `scratchpad_root` must lie inside the service's own (symlinks resolved, `..` rejected); `cwd` must lie inside a `--cwd-root` when any is
given. Destructive shell (`--unsafe`) is not reachable from the API. A non-loopback bind without a token refuses to start (exit 2); with a
token it warns (no TLS). No token: a loud warning. Prompts and keys are never logged; the key is scrubbed from every response and error.

**GUI.** Essential and useful endpoints only: `/api/state` (GET/POST; omitted = unchanged, null = clear; 400 validation, 422 types;
all-or-nothing), `/api/chat` (blocking JSON like the service, or SSE with `Accept: text/event-stream`), `/api/chat/cancel`, `/api/run`,
`/api/events?after=N` (reattachable replay then live), `/api/approvals` (+ `POST /{id}`), `/api/sessions` (list, `?q=`, get, delete),
`/api/file-history`, `/api/slash-commands`, `/api/plan`, `/api/tasks` (read only), `/api/memory` (+ review approve/reject),
`/api/instructions` + `/file` (GET/PUT/DELETE), `/api/tiers`, `/api/backend`, `/api/clear`. No panels for accounts, remote, triggers,
workflows, search, teams, ask-queue, plugins, worktree, MCP calls or skills (a test checks they 404).
*Security:* loopback-name `Host` only (rebinding), `Origin` must be this origin, `Sec-Fetch-Site: cross-site` refused, mutations must be
`application/json` (415), `/api/*` needs the per-launch random token in `X-Sarina-Token`; the page itself needs it too (`?t=` once, then a
`HttpOnly; SameSite=Strict` cookie) and the startup URL carries it. No CORS headers; CSP `default-src 'none'; script-src 'self'; ...`,
nosniff, no-store, no framing. A non-loopback `--host` is refused outright. `allow_shell`/`allow_write`/auto-approve cannot be turned on
without `confirm_permissions:true` (the page asks with a dialog and shows the mode in a persistent pill). Instruction files: read/write/delete
only the standard names in the workspace, extra dirs and `~/.claude` (ancestors' files are read-only), new `.claude/rules/<name>.md`
allowed, symlink writes refused, content shown redacted and a redacted file cannot be saved back. Session ids are pattern-checked (no
traversal). Everything returned goes through `runkit.Scrubber`; the API key is write-only (`api_key_set`).
*Runs:* one at a time (409 otherwise); settings saved mid-run apply to the next run (neither blocked on a lock nor discarding the run);
the agent is built per run from the state and sessions resume from disk. Events: `run_start turn_start text_delta assistant_message
tool_call tool_delta tool_result approval_request approval_resolved info usage error done`, each with `seq` and `run_id`. Approvals:
`approval_mode` `ask` (default) puts every write/shell/mutating call to the operator (allow once / allow for this run / deny + reason; full
arguments, summary, risk hint including the destructive-command check); `auto` asks nothing. Plan mode: `exit_plan_mode` raises a plan
approval (approve / revise with feedback / reject), fed to the agent's existing `PlanDecider`. Unanswered approvals, and runs cancelled while
waiting, resolve as deny; an unanswered plan leaves the run at `plan_pending`. Local `/help` is answered without the model.
*Front end:* one page; chat stream with markdown built from DOM nodes (no `innerHTML`), tool cards (effect badge, args, result, truncation),
approval cards, sessions sidebar (search, resume, delete), settings form, plan/tasks, memory review + instruction editor, history, tiers,
large-paste chips, slash palette; light/dark by OS, phone layout (the sidebar slides over). Checked in headless Edge: boot, reattaching to a
run waiting on approval, the finished transcript, a 500px-wide layout.

Tests: `service/service_test.go` (auth, validation, allowlist [] across turns, ceiling, cwd roots, LRU, resume, serialisation),
`gui/gui_test.go` (Host/Origin/token/content-type, page token + cookie, no-network assets, state semantics, SSE, text deltas, reattach,
409, cancel, approvals allow/deny/session/timeout/cancel, plan approve/revise/timeout, sessions, instruction sandbox, memory review,
redaction, local slash), `cmd/sarina/main_test.go`. All offline (`httptest` fake model). No race detector here (no cgo on this machine).

**Not done / known gaps.** No edited-arguments option on approvals. The first approval of a fresh session shows `session_id: ""` (the id is
not known until the run saves). No TLS (loopback trust model). The service has no streaming endpoint (contract unchanged). MCP servers,
skills and `ask_user_question` are not exposed in either server. The GUI does not trigger session notes' exit-time memory review.

## Doctor and the troubleshooting skill (status, 2026-10-06)

Written clean-room from the Go code, `README.md`, and the sibling runbooks (`Limbo/docs/INTEGRATION-FAILURES.md`,
`Chymaera_Sentry/docs/INTEGRATION-FAILURES.md`). Nothing under `sarina/` or `tests/` was read. Standard library only.

**`sarina doctor [--json] [--cwd D] [--session-dir D]`** (`go/doctor`, thin `command.go`; registered through `cli.Register`).
Strictly read-only; each check returns `{id,status,finding,remedy}` with PASS/WARN/FAIL/SKIP (remedy only when not PASS/SKIP).
Exit 0 with no FAIL, 1 with any FAIL, 2 on usage errors. Text is an aligned table (remedy on the line below its row); `--json` is
a stable array. Rows, in order: `backend`, `context_window` (needs `/v1/models`; window from `SARINA_CONTEXT_WINDOW` or
`llm.Discoverer`, WARN under 8192 with the Ollama `num_ctx` hint), `sessions` (exists, writable via access(2) without creating
anything, parse counts, unreadable files named), `session_retired_tools` (WARN, remedy `sarina session-migrate`; read from the raw
file because `session.Parse` already rewrites in memory; added `session.IsRetiredTool`), `session_contract` (Sentry's shape, and FAIL
if any session file holds a non-empty `api_key`; value never shown), `tiers_config` plus one `tier_<name>` row per tier (reuses
`tiers.Load` and `Router.Status`; a missing `claude` executable is a WARN), `ext_skills`, `ext_mcp`, `ext_policy` (an operator policy
file in `policy.Fatal` is a FAIL, a workspace one only warns), `limbo` (SKIP unset; socket name, exists, is a socket, world-writable
FAIL, `GET /v1/profiles` through `limbo.Client`, runbook codes mapped to the runbook's first check and cited), `sentry` (SKIP unset
and says fail-open; otherwise only `GET /healthz` on the URL's host, which is the cheap endpoint Sentry's runbook shows; no review text
is ever sent), `ports` (informational TCP dial to 8899 and 8765; nothing is bound).
No secret ever reaches a report: only `set, N chars` / `unset`, URLs reduced to scheme and host (userinfo, path and query dropped);
tests plant a canary in the API key, base URL, Sentry URL, tier key and a session file and grep both renderings.
`tiers.json` presence and validity are in `tiers_config`, not a separate row. Every network call has a 3 s timeout (`ShortTimeout`).
The model tool is `sarina_doctor` (`ClassReadOnly`, `PermNone`, no parameters, returns the JSON). It is in the default registry via
`tools.RegisterProvider`, so plan mode and read-only sessions keep it.

**Footprint.** The tool costs about 310 JSON chars. Budgets in `cmd/sarina/footprint_test.go` went 10,800 -> 11,100 (all permissions,
now 12,132 measured, 24 tools) and 4,000 -> 4,300 (read-only, now 4,307, 12 tools): the tool plus a little headroom. Before this change
the all-permissions figure (11,822) was already within 60 chars of its limit, so the old budget could not have held anything.

**Skills.** `skills.Roots` now also searches the root-owned `/usr/share/sarina/skills` (Linux only; `Options.SystemDir` overrides it
for tests) after the operator's own locations and before the opt-in workspace root, so a workspace skill can never shadow it
(tests: shadowing is a reported collision; the operator's own skill wins over the system one). The shipped skill is
`packaging/skills/ecosystem-troubleshoot/SKILL.md` (`model_invocable`, read-only `allowed_tools` plus `limbo_profiles`, `limbo_validate`,
`sarina_doctor`, `enter_plan_mode`, `exit_plan_mode`; no bash, no write tools). A test loads it through the real parser and security scan.
`limbo_validate` still needs `--allow-shell` (it is shell-gated); the skill says to fall back to files when it is refused.
Packaging: `install -Dm644` lines added to `packaging/arch/PKGBUILD` and to Chymaera_Linux's `sarina-git/PKGBUILD` (LF; the checkout there
had CRLF working-tree endings, converted back to LF so the diff is the two added lines only).

Not done: the doctor cannot see ACLs on Windows (permission-bit check only) and skips Limbo mode checks there; it does not check
`allow_data` semantics, the service/GUI token state, or whether Sentry's `/v1/review` actually accepts a request (that would need a
review call, which could be audited as real traffic). Skill text is untested against a real small model.

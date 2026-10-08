# 04 - CLI and slash commands (behaviour spec)

Clean-room input for the Go rewrite. Describes observable behaviour of the
`sarina` command line and of the in-session slash commands. Nothing here is
source text; all messages are described by what they must convey, and the Go
implementation writes its own wording. Where an exact token is part of a
machine-readable contract (a `key=value` line, a flag name, an exit code, a JSON
field) it is given verbatim and must be kept.

Legend
- Provenance: **DERIVED-FEATURE** (comes from the unlicensed upstream; reimplement
  from this spec only) or **ORIGINAL** (Sarina's own; keeps working, gets re-hooked
  into the new core).
- Rating: ESSENTIAL / USEFUL / DROP-CANDIDATE, with a one-line reason.
- "Owner" = the Python module that implements the behaviour today. Other spec
  documents cover those modules' internals; this one covers only the CLI surface.

---

## 1. Entry points and process-wide conventions

Three console scripts must keep resolving (the distro packages them):
`sarina` (this CLI), `sarina-gui` (web GUI, port 8765), `sarina-service`
(integration service, port 8899). GUI and service are specified elsewhere; this
document covers `sarina` only. `python -m sarina.main <command>` is the
equivalent module form.

Conventions that hold across the CLI:

- One required subcommand. Missing/unknown subcommand or a bad flag: usage text
  to stderr, exit code **2** (standard argument-parser behaviour). Rebuild
  with a parser that gives the same exit code.
- Output is **human-readable text on stdout**, not JSON, for every command except
  `fleet-usage` (compact one-line JSON, no spaces after separators). Many status
  commands print a first line of the form `# Title`, then a blank line, then the
  body (Markdown-ish headings). Machine-parsed lines are `key=value` (no spaces
  around `=`).
- Errors from "expected" failures are printed to **stdout** (not stderr) as a
  single human sentence and the command returns exit code 1. A few commands use
  exit code 2 for "could not even attempt" (see `fleet-launch`).
- Unexpected failures (e.g. malformed JSON given to `--arguments-json`,
  `--body-json`, `config-set`'s value, or `mcp-call-tool`'s arguments) are not
  caught in several derived commands and surface as an uncaught exception
  (non-zero exit, stack trace). A rewrite should instead print a clear message and
  exit 1; this is an allowed improvement.
- `--cwd` (default `.`) names the workspace directory and is resolved to an
  absolute path. Nearly every workspace-scoped command takes it. It is
  documented per command only when it is not simply "workspace directory".
- Environment variables read by the CLI: `OPENAI_BASE_URL` (default
  `http://127.0.0.1:8000/v1`), `OPENAI_API_KEY` (default `local-token`),
  `OPENAI_MODEL` (default `Qwen/Qwen3-Coder-30B-A3B-Instruct`),
  `CLAUDE_CODE_CLI_PATH` (default `claude`), `CHYMAERA_CONSOLE_TOKEN`,
  `SARINA_LIMBO_SOCKET`. Flags override env; env overrides built-in defaults.
- Session files live under `./.port_sessions/agent` (relative to the process
  working directory at the time the config is built, resolved to absolute) and the
  scratchpad under `./.port_sessions/scratchpad` unless `--scratchpad-root` is
  given. Note this is relative to the process cwd, not `--cwd`.

---

## 2. Shared flag groups

### 2.1 Agent common flags ("agent-common")
Used by `agent`, `agent-bg`, `agent-bg-worker`, `daemon start|worker`,
`agent-chat`, `voice-chat`. A reduced form without the backend/cost flags
(`include_backend=False`) is used by `agent-prompt`, `agent-context`,
`agent-context-raw`, `token-budget`, `agents`.

Backend group (only when backend included):
- `--model` (default env `OPENAI_MODEL` or the built-in default; present in the
  reduced form too)
- `--base-url`, `--api-key`
- `--temperature` float, default 0.0
- `--timeout-seconds` float, default 120.0
- `--input-cost-per-million`, `--output-cost-per-million` floats, default 0.0
  (USD per million tokens; drives `total_cost_usd`)

Workspace and permissions:
- `--cwd` default `.`; `--add-dir` repeatable (extra readable/writable roots)
- `--disable-claude-md` (skip CLAUDE.md discovery)
- `--allow-write` (enables file-modifying tools), `--allow-shell` (enables the
  shell tool), `--unsafe` (additionally allows destructive shell commands).
  Defaults: all three off. `--unsafe` alone does not imply the others.
- `--stream` (stream model tokens as they arrive)

Context management:
- `--auto-snip-threshold` int tokens, `--auto-compact-threshold` int tokens
  (no default = feature off); `--compact-preserve-messages` int, default 4,
  negative values clamp to 0

Budgets (all optional ints/floats, none by default): `--max-total-tokens`,
`--max-input-tokens`, `--max-output-tokens`, `--max-reasoning-tokens`,
`--max-budget-usd` (float), `--max-tool-calls`, `--max-delegated-tasks`,
`--max-model-calls`, `--max-session-turns`.

Escalation to the real Claude Code CLI: `--escalate-to-claude-code` (switch),
`--claude-code-cli-path` (default env `CLAUDE_CODE_CLI_PATH` or `claude`),
`--max-escalations` int default 1, `--escalation-timeout-seconds` float default
300, `--escalation-trigger` repeatable, one of `backend_error`,
`budget_exceeded`, `resource_exhausted` (default when none given:
`backend_error` and `resource_exhausted`), `--max-memory-percent`,
`--max-cpu-percent`, `--max-gpu-memory-percent` floats default 90 each,
`--disable-resource-check` (nulls all three thresholds). Disk-free threshold is
always unset from the CLI.

Structured output: `--response-schema-file` (path to a JSON file whose top level
must be an object, else error "response schema file must contain a top-level JSON
object"), `--response-schema-name` (default: file stem),
`--response-schema-strict`.

Prompt control: `--system-prompt`, `--append-system-prompt`,
`--override-system-prompt`; `--scratchpad-root`.

Per-command extras that accompany agent-common: `--max-turns` int default 12
(model-loop turns per run) and `--show-transcript` (also print the message
list).

### 2.2 Runtime-config mapping
The flags above become the agent runtime config: permissions tuple (write /
shell / destructive-shell), streaming boolean, snip/compact thresholds, extra
directories, claude-md discovery toggle, budget limits, escalation settings with
resource thresholds, output schema, session directory, scratchpad root. The new
core must accept the same set; the spec for the loop (another document) defines
what each limit does.

---

## 3. Core agent commands

All print a **run result block** (section 3.1) except where stated.

| Command | Provenance | Rating |
|---|---|---|
| `agent <prompt>` | DERIVED-FEATURE | ESSENTIAL: the primary one-shot entry point; everything else wraps it |
| `agent-chat [prompt]` | DERIVED-FEATURE (loop) + ORIGINAL (auto memory review hook) | ESSENTIAL: interactive use |
| `agent-resume <session_id> <prompt>` | DERIVED-FEATURE | ESSENTIAL: session persistence is a core promise |
| `agent-bg`, `agent-ps`, `agent-logs`, `agent-attach`, `agent-kill`, `agent-bg-worker` | DERIVED-FEATURE | USEFUL: long jobs without tying up a terminal; fleet-usage and Sentry see these sessions |
| `daemon start|ps|logs|attach|kill|worker` | DERIVED-FEATURE | DROP-CANDIDATE: exact aliases of the `agent-bg*` family, no added behaviour |
| `agent-prompt` | DERIVED-FEATURE | USEFUL: debug view of the assembled system prompt |
| `agent-context`, `agent-context-raw`, `token-budget` | DERIVED-FEATURE | USEFUL (`agent-context`/`token-budget`), DROP-CANDIDATE (`agent-context-raw`): diagnostics only |
| `agents`, `agents-create`, `agents-update`, `agents-delete` | DERIVED-FEATURE | USEFUL: sub-agent definition files are how delegation is configured |

### 3.1 Run result block (`_print_agent_result` behaviour)
After a run, print in order:
1. The model's final output text, alone, on its own line(s).
2. Blank line, then heading `# Usage`, then lines `total_tokens=N`,
   `input_tokens=N`, `output_tokens=N`, `total_cost_usd=X.XXXXXX` (six decimals).
3. If a stop reason exists: `stop_reason=<reason>`.
4. If a session id exists: blank line, `# Session`, `session_id=<id>`, and, if a
   path exists, `session_path=<path>`.
5. If a scratchpad directory exists: `scratchpad_directory=<path>`.
6. With `--show-transcript`: blank line, `# Transcript`, then for each message a
   `[role]` line followed by its content line. A message lacking a role prints
   `[unknown]`; lacking content prints an empty line.

Stop reasons are produced by the loop (documented elsewhere); the CLI just
prints them. Exit code of `agent`, `agent-resume`, `agent-chat` after a normal
completion is **0 regardless of stop reason** (including backend errors that the
loop reports as a stop reason). Interrupt in `agent-chat` returns 130.

### 3.2 `agent <prompt>`
Positional `prompt`; agent-common flags plus `--max-turns`, `--show-transcript`.
Builds the agent, runs the prompt once (slash commands in the prompt are handled
per section 6 before any model call), prints the run result block, exits 0.

### 3.3 `agent-resume <session_id> <prompt>`
Positionals: `session_id`, `prompt`. Loads the saved session, rebuilds model and
runtime config **from the saved session**, then applies *only the flags the user
actually passed* as overrides on top. Unspecified flags must not reset saved
values (none of this command's flags have non-None defaults; the switch flags
default off, and a switch can only turn a setting on, never off).

Overrides supported: `--model`, `--base-url`, `--api-key`, `--temperature`,
`--timeout-seconds`, the two cost-per-million values (each independently,
keeping the other saved value if only one is given), `--max-turns`,
`--allow-write`/`--allow-shell`/`--unsafe` (OR-ed with saved permissions - cannot
revoke), `--stream`, the three snip/compact flags, all budget flags
(per-field merge with saved values), all escalation flags (per-field merge;
`--disable-resource-check` nulls the three thresholds but keeps the saved
disk threshold; `--escalate-to-claude-code` enables), response-schema flags,
`--scratchpad-root`. `--show-transcript` also accepted. Not accepted here (differs from
`agent`): `--cwd`, `--add-dir`, `--disable-claude-md`, system-prompt flags.

Then runs the prompt as a continuation of that session and prints the run result
block. Loading a nonexistent session raises an error (uncaught today; rewrite
should print a message, exit 1).

### 3.4 `agent-chat [prompt]` - interactive loop
Optional positional `prompt` (an initial turn executed before the first
interactive prompt), `--resume-session-id <id>`, `--max-turns`,
`--show-transcript`, `--no-auto-memory-review` (store-false; automatic review
**on** by default from the CLI), plus agent-common flags.

Loop UX (also used by `voice-chat`; see 3.5):
1. Print `# Agent Chat`, then a one-line instruction saying to type `/exit` or
   `/quit` to stop. If resuming, print `resuming_session_id=<id>`.
2. Each iteration: if an initial prompt is pending, use it without asking;
   otherwise read a line with the prompt string `user> `.
3. Blank/whitespace-only input is ignored (re-prompt, no model call).
4. Trimmed input exactly equal to `/exit` or `/quit` ends the session: print
   `chat_ended=user_exit`, run the automatic memory review (if enabled and a
   session exists), return 0. Note this exact-match check precedes slash
   dispatch; `/exit` *with arguments* falls through to the slash handler (6.2)
   which terminates the process immediately and skips the review.
5. End-of-input (Ctrl-D / closed stdin): print `chat_ended=eof`, run review,
   return 0.
6. Ctrl-C **at the input prompt**: print a newline then `chat_ended=interrupt`,
   run review, return **130**. Ctrl-C *during* a model turn is not handled by the
   loop (propagates and kills the process); a rewrite should treat it as a turn
   cancel and return to the prompt - this is an allowed improvement.
7. Otherwise: if a session id is already active, reload that stored session from
   the session directory and continue it; else start a fresh run. Print the run
   result block (3.1). Remember the returned session id for the next turn. A
   slash command that is answered locally returns no new session id for a first
   turn, so the following ordinary prompt starts a fresh session.
8. Output goes through an injectable output function and results through an
   injectable result printer; input through an injectable reader (used by tests
   and by voice-chat). Preserve these seams in Go.

Automatic memory review (ORIGINAL, `_run_automatic_memory_review`): at session
end, if enabled and a session id exists, load the saved session, ask the model
to propose curated-memory entries (same routine as `memory-review`), stage them in
the workspace review queue, and print one line:
`memory_review: proposed N entry|entries (P pending, B blocked) -- run 'sarina memory-review-list' to see them`.
No proposals: print nothing. Any failure (session load, backend, parse): print
`memory_review_skipped=error (<message>)` and continue; it must never change the
exit code or raise. The loop-level default for the feature is **off**; only the
CLI turns it on.

### 3.5 `voice-chat` (ORIGINAL, rating USEFUL: the distro's voice interface; optional extra)
Same loop as `agent-chat` with injected input/output adapters, no positional
prompt, always starts at an input prompt. Flags: `--resume-session-id`,
`--show-transcript`, **required** `--stt-model-dir` (local speech-recognition
model directory), `--stt-device` (`cpu`|`cuda`, default `cpu`),
`--stt-compute-type` (default `int8`), **required** `--tts-model` (path to a
speech-synthesis voice model file), `--tts-config` (optional sidecar config),
`--input-device` / `--output-device` (ALSA device names), `--max-record-seconds`
(default 120), `--no-auto-memory-review`, plus agent-common flags. `voice-chat` does not
declare `--max-turns`; the agent uses its default of 12 turns per run.
Behaviour: if the voice extras or model files are unavailable, print the error
message and return 1 before touching the agent. Per turn the input adapter prints
the `user> ` prompt and a hint (press Enter to record; type quit then Enter to
end), reads a line: if it is one of `q`, `quit`, `exit`, `stop` (case-insensitive)
it returns `/exit` to the loop; otherwise it records push-to-talk audio (Enter to
start, Enter to stop, capped at `--max-record-seconds`), transcribes it, echoes
`(heard: <text>)` or `(heard nothing)` and returns the text (empty text means the
loop re-prompts). The result printer first prints the normal text result block,
then synthesizes and plays the final output; a synthesis/playback error prints
`(voice playback failed: <detail>)` and does not abort. No model downloads, ever
(local files only). Audio capture/playback shells out to `arecord`/`aplay`.

### 3.6 Background sessions (`agent-bg` family; DERIVED-FEATURE)
- `agent-bg <prompt>`: agent-common flags + `--max-turns`, `--show-transcript`.
  Launches a detached worker process running the same agent in the background and
  prints: `# Background Session`, `background_id=<id>`, `pid=<pid>`,
  `log_path=<path>`, `record_path=<path>`; exit 0. The worker is the same
  executable invoked with the hidden `agent-bg-worker` subcommand and the full
  set of forwarded flags (every non-default flag is re-serialized onto the worker
  command line, including `--background-root`, `--max-turns`, backend settings,
  permission switches, budgets, escalation, schema, prompt flags). The worker is
  launched with the process cwd set to the install root, not the workspace.
- `agent-bg-worker <background_id> <prompt> --background-root <dir> ...`
  (hidden from help): runs the agent, prints the run result block to its log,
  and in all cases (success or crash) records completion: exit code (0 on success,
  1 if the run raised), stop reason (the run's, else `completed`; or
  `worker_failed` on crash), session id and path.
- `agent-ps [--tail N]`: table of all background sessions. `--tail` limits how many
  recent entries/lines (semantics owned by the background-runtime spec).
- `agent-logs <background_id> [--tail N]`: log text; `agent-attach <id> [--tail N]`:
  current output snapshot (does not follow).
- `agent-kill <background_id>`: stops the process; prints `# Background Session`,
  `background_id=`, `status=`, `pid=`, and `exit_code=` only if known; exit 0.
- `daemon <start|ps|logs|attach|kill|worker>` is the identical set under a
  required sub-subcommand, with the same flags and output (DROP-CANDIDATE).
- Unknown id behaviour: raised error today (uncaught); rewrite should print a
  message and exit 1.

### 3.7 Inspection commands
- `agent-prompt`: print the fully assembled system prompt for the given
  workspace/flags. No model call. Exit 0.
- `agent-context`: print the context-usage accounting report (estimated token use
  by section). `agent-context-raw`: print the raw snapshot of environment, user
  context, system context. `token-budget`: builds a session then prints the
  token-budget window/reserve/limit report. All three take the reduced agent-common
  flags, make no model call, exit 0.
- `agents [agent_type] [--all]`: without a type, list the active agent definitions
  (`--all` includes shadowed/inactive ones); with a type, print that definition's
  details. Reduced agent-common flags. Exit 0.
- `agents-create <agent_type>`: flags `--cwd`, `--source`
  (`project` default | `user` | `auto` | `projectSettings` | `userSettings`),
  `--description`, `--prompt` or `--prompt-file` (mutually exclusive; both given is
  an error "Specify only one of --prompt or --prompt-file"), `--tools`
  (comma-separated names; empty or `*` means "all tools"), `--model`, `--color`,
  `--permission-mode`, `--max-turns`, `--initial-prompt`, `--background`,
  `--one-shot`, `--omit-claude-md`, `--overwrite`. Creates a Markdown file with
  front-matter under `<workspace>/.claude/agents/<type>.md` (project) or the
  user-level `~/.claude/agents/` (user). Defaults when absent: a generic
  description saying to use the agent when the task calls for the type, and a
  two-sentence generic system prompt naming the type. Refuses to overwrite
  without `--overwrite`. Prints a mutation report. Exit 0.
- `agents-update <agent_type>`: same flags; only flags actually given are
  changed (`--background`, `--one-shot`, `--omit-claude-md` can only set to true).
  `--source` accepts `auto` (find the existing definition).
- `agents-delete <agent_type> [--source] [--cwd]`: default source `auto`.
  Deletes the file; prints a mutation report.
- Agent-definition file format and discovery are specified with the agent
  registry; the contract that matters here is the path
  `.claude/agents/<type>.md` and that tests create then delete one.

---

## 4. ORIGINAL commands (keep working; re-hook onto the Go core)

These do not depend on the derived loop except where noted. Their implementing
modules stay in Python (ml/voice) or are re-implemented in Go against their own
specs; the CLI contract below must not change.

### 4.1 Memory and session search

| Command | Owner | Rating |
|---|---|---|
| `memory-status` | memory_runtime | ESSENTIAL: operator visibility into what is injected into every prompt |
| `memory-show --scope agent|user` | memory_runtime | ESSENTIAL: same |
| `memory-review <session_id>` | memory_review | USEFUL: staged, human-gated learning |
| `memory-review-list` / `-approve <id>` / `-reject <id>` | memory_review | ESSENTIAL if memory-review is kept (the approval gate is the security control) |
| `session-search <query>` | session_search_fts | USEFUL: no-extra lexical recall |
| `memory-search <query>` | ml/session_search | USEFUL: semantic recall, optional extra |

- `memory-status [--cwd]`: prints the memory runtime's summary (per-scope entry
  counts and usage against limits; exact layout owned by the memory spec). Exit 0.
- `memory-show --scope {agent|user} [--cwd]`: `--scope` required. If none stored:
  `No entries stored in the "<scope>" scope.` exit 0. Else for each entry, a line
  `--- entry N ---` (1-based) followed by the entry text. Exit 0.
- `memory-review <session_id> [--session-dir] [--cwd] [--model --base-url
  --api-key --temperature --timeout-seconds --input-cost-per-million
  --output-cost-per-million]`: backend flags only (no permission/budget flags).
  Flow: load session from `--session-dir` (default: standard session directory);
  on OS/value/key error print `could not load session <id>: <reason>`, exit 1.
  Ask the model for up to 5 proposals (details in memory spec); on any exception
  print `model backend error: <reason>`, exit 1. Add proposals to the workspace
  queue (queue is touched even if empty). Zero proposals: print that no entries
  were proposed, exit 0. Else print `Proposed N entry|entries: P pending review,
  B blocked by the security scan.` then a hint to run `memory-review-list`. Exit 0.
- `memory-review-list [--cwd]`: prints queue summary (pending and resolved).
- `memory-review-approve <entry_id> [--cwd]`: applies the proposal through the
  same security-scanned write path as the interactive tool. Success:
  `approved: <action> in scope <scope> (<count> entries now stored)`, exit 0.
  A memory-runtime error (unknown id, blocked, over limit, duplicate): print the
  error text, exit 1.
- `memory-review-reject <entry_id> [--cwd]`: success `rejected: <id>`, exit 0;
  error text + exit 1.
- `session-search <query> [--session-dir] [--top-k N=10]`: lexical search over
  saved sessions. No hits: `No matching messages found in saved sessions.` exit 0.
  Else per hit two lines: `<score 3dp>  session=<id> turn=<n> role=<role>` then
  an indented snippet line. Exit 0.
- `memory-search <query> --model-dir <dir> [--session-dir] [--top-k N=10]`:
  `--model-dir` required. If the optional ML extra or model is unavailable print
  the error text, exit 1. No sessions: `No saved sessions found.` exit 0. Else per
  result: `<score 3dp>  <session_id>`, then `  task:    <text>`, `  outcome: <text>`.
  Exit 0.

### 4.2 Chymaera ecosystem

| Command | Owner | Rating |
|---|---|---|
| `chymaera-console` | chymaera/console | ESSENTIAL (to the ecosystem): the only path to the App's datastore |
| `chymaera-analyze` | chymaera/analysis (+ ml) | USEFUL: capture triage; stats work without ML |
| `chymaera-device-info` | chymaera/firmware | USEFUL: firmware sanity check, read-only |
| `chymaera-merge-report` | chymaera/firmware | USEFUL: upstream-merge risk report, read-only |
| `limbo-profiles`, `limbo-validate` | chymaera/limbo | USEFUL: safe command validation in a disposable VM |
| `fleet-usage`, `fleet-status` | chymaera/agent_usage, agent_fleet | USEFUL: ecosystem contract item 7 (live) |
| `fleet-launch` | chymaera/agent_launch | USEFUL, with an inviolable rule below |

- `chymaera-console [--host 127.0.0.1] [--port 44700] [--token
  $CHYMAERA_CONSOLE_TOKEN] [--captures N=10]`: connects to the App's NDJSON
  control API on loopback, prints a status report: status plus the N most recent
  captures. Connection refused / protocol error: print the error message, exit 1.
  Otherwise 0.
- `chymaera-analyze [--host --port --token] [--limit N=100] [--model-dir]
  [--similarity-threshold 0.75]`: pulls up to `limit` recent captures through the
  console client, analyses only the App-provided metadata (never parses raw capture
  files), prints a report (always: duplicate NFC UIDs, rare Sub-GHz frequencies;
  with a model: semantic grouping at the threshold). If `--model-dir` is given but
  ML is unavailable, print `semantic grouping unavailable: <reason>` and continue
  with statistics only. Console failure: message, exit 1. Else 0.
- `chymaera-device-info [--file F] [--expected-commit C]`: reads captured
  `device_info` text from the file, or **stdin** when `--file` is omitted; parses it,
  checks it (stock firmware, invalid enclave, dead radio, untagged/`unknown`
  version, commit mismatch), prints the report, exit **0 if ok, 1 if any problem**.
  Version string format recognised: `<upstream>-CHY-<ours>[+ahead]`; the older
  describe-style format is recognised as legacy, not misparsed.
- `chymaera-merge-report [--cwd <firmware checkout>] [--upstream-ref upstream/dev]`:
  prints which files changed on both sides since the merge base, tiered by whether
  the path exists upstream (high/medium/low) with descriptive area labels and a
  policy alarm past 12 non-additive files. Git failure: message, exit 1. Else 0.
- `limbo-profiles`: requires env `SARINA_LIMBO_SOCKET`. Prints one profile name per
  line, or a message that none exist (a human adds them under `/etc/limbo/validation/`).
  Client error: message, exit 1.
- `limbo-validate <profile> <command...> [--timeout 300]`: joins the remaining
  words with single spaces into one command string, runs it in a disposable guest,
  prints the run report, exit **0 if the run succeeded, else 1**; Limbo errors:
  message, exit 1. (`command` is greedy: flags after it are swallowed into it.)
- `fleet-usage [--session-dir D ...] [--write] [--usage-dir D] [--force]
  [--limits-only]`: collects Sarina's own usage record from its session
  directories (default: the known locations). Without `--write`, prints the record
  as **compact single-line JSON** (this is what Omarchy's collector wrapper
  captures). With `--write`, writes `sarina.json` into the panel usage directory
  (`--usage-dir`, default `~/.local/state/omarchy/agents/usage`) and prints the
  written path. `--force` and `--limits-only` are accepted and ignored (collector
  compatibility). Exit 0.
- `fleet-status [--usage-dir D] [--suggest]`: prints per-agent usage/limits read
  from other agents' records (untrusted input, sanitised). `--suggest` appends a
  blank line plus either `Most headroom: <name> (<N>% left in its tightest
  window)` or a sentence that no agent has a current measured limit. Exit 0.
- `fleet-launch <agent> [prompt] [--cwd] [--yes]`: `agent` is one of the
  supported agents or one of the explicitly-unsupported names (choices = union;
  unsupported ones fail with a reason). Prints the launch proposal, then asks
  `Launch? [y/N]` unless `--yes`. Non-TTY stdin without `--yes`: print a
  not-launching message, exit **2**; EOF at the prompt: exit 2; answer not y/yes:
  print `Cancelled.`, exit **1**; confirmed: flush stdout and run the agent in the
  foreground, returning **its exit status**; planning errors: `fleet-launch:
  <reason>`, exit 2. Every attempt is audited (JSON lines under the user state
  directory). **Hard rule: this command is CLI-only and must never be registered
  as a model-callable tool; there is no auto-approve mode; `--yes` only skips the
  question, the launched agent still asks before acting.** A test enforces this.

### 4.3 Desktop menu audit

`desktop-audit` (ORIGINAL, USEFUL: fixes the menu for security tools that ship no
launcher; Linux-only data sources).
Flags: `--model-dir` (optional embedding model), `--similarity-threshold` float
default 0.35, `--include-dependencies`, `--include-libraries`, `--apply`,
`--min-confidence` float default 0.0 (with `--apply`), `--target-dir` (default: the
user application directory), `--remove-generated`, `--no-cache`.
Behaviour:
- `--remove-generated`: delete previously generated entries from the target
  directory, print `removed <path>` per file then `<N> generated entries removed.`,
  exit 0, nothing else runs.
- Otherwise: scan existing desktop entries and query installed packages (explicit
  only by default), classify, print the audit report. If `--model-dir` is given and
  ML is unavailable print `semantic inference unavailable: <reason>` and proceed
  with deterministic rules. Semantic results are cached (JSON, content-hash keyed)
  at `~/.sarina/cache/desktop_classify.json` unless `--no-cache`; the cache is only
  consulted when a model is in use.
- Report only by default: if proposals exist, finish with a blank line and a note
  that nothing was written and `--apply` writes them. With `--apply`: write each
  proposal at or above `--min-confidence`, print `wrote <path>` per file then
  `<N> entries written. Undo with --remove-generated.`
- Always exit 0.

---

## 5. Workspace-runtime commands (DERIVED-FEATURE)

All take `--cwd`, print text to stdout, and exit 0 unless stated. The runtimes
behind them (remote profiles, accounts, ask-user, search providers, MCP client,
layered config, LSP client, workflows, remote triggers, teams, managed
worktrees) are specified by other documents. From a CLI/rating standpoint:

| Group | Commands | Rating |
|---|---|---|
| MCP | `mcp-status`, `mcp-resources [--query]`, `mcp-resource <uri>`, `mcp-tools [--query] [--server]`, `mcp-call-tool <name> [--arguments-json '{}'] [--server]` | USEFUL: MCP is how external tools reach the agent; keep tools/call, optional elsewhere |
| Config | `config-status`, `config-effective`, `config-source <source>`, `config-get <key_path> [--source]`, `config-set <key_path> <value_json> [--source local]` | USEFUL: settings are needed; `config-set` writes JSON by dotted key |
| Search | `search-status [--provider]`, `search-providers [--query]`, `search-activate <provider>`, `search <query> [--provider] [--max-results 5] [--domain D ...]` | USEFUL: web search provider selection; `search` itself is a debug path |
| Worktrees | `worktree-status`, `worktree-enter [name]`, `worktree-exit [--action keep] [--discard-changes]` | USEFUL: safe isolation for risky edits |
| LSP | `lsp-status`, `lsp-symbols <file>`, `lsp-workspace-symbols <query> [--max-results 50]`, `lsp-definition|lsp-references|lsp-hover|lsp-call-hierarchy|lsp-incoming-calls|lsp-outgoing-calls <file> <line> <character> [--max-results]`, `lsp-diagnostics [--file-path]` | DROP-CANDIDATE: 10 commands of IDE parity; a pentest agent rarely needs a language server |
| Remote | `remote-mode|ssh-mode|teleport-mode|direct-connect-mode|deep-link-mode <target>`, `remote-status`, `remote-profiles [--query]`, `remote-disconnect` | DROP-CANDIDATE: "simulates" branching of a cloud remote-control product; no effect on the local agent |
| Account | `account-status`, `account-profiles [--query]`, `account-login <target> [--provider] [--auth-mode]`, `account-logout` | DROP-CANDIDATE: identity bookkeeping for an upstream hosted service; local models need no login |
| Ask-user | `ask-status`, `ask-history` | DROP-CANDIDATE: read-only view of the ask-user tool's log |
| Workflow | `workflow-list [--query]`, `workflow-get <name>`, `workflow-run <name> [--arguments-json '{}']` | DROP-CANDIDATE: records a run report, executes nothing real |
| Triggers | `trigger-list [--query]`, `trigger-get <id>`, `trigger-create --body-json`, `trigger-update <id> --body-json`, `trigger-run <id> [--body-json '{}']` | DROP-CANDIDATE: local simulation of remote triggers |
| Teams | `team-status`, `team-list [--query]`, `team-get <name>`, `team-create <name> [--description] [--member M ...]`, `team-delete <name>`, `team-messages [--team-name]` | DROP-CANDIDATE: local record-keeping with no real multi-agent runtime behind it |

Exit-code details common to the group:
- "Unknown X" lookups (search provider, workflow, trigger, team, LSP file key)
  print a short message naming the unknown item and return **1**.
- `workflow-run`, `trigger-*`, `mcp-call-tool`: JSON argument that does not decode
  to an object prints `arguments-json must decode to a JSON object` (or
  `body-json ...`) and returns 1.
- `team-create` on an existing team: `Team already exists: <name>`, exit 1;
  success prints `created team <name>`; `team-delete` success prints
  `deleted team <name>`.
- `worktree-enter`/`worktree-exit`: runtime/value errors print the message,
  exit 1. `--action` is `keep` (default) or `remove`.
- `search`: any lookup/OS/value failure prints a message starting `Search failed:`
  and exits 1.
- `config-set` prints a `# Config` block with `source=`, `key_path=`,
  `store_path=`, `effective_key_count=` lines then the new value rendering.
- Status-style commands open with `# <Area>` (Remote, Worktree, Account, Ask User,
  Search, MCP, Config, LSP, Teams) + blank + summary; list/get commands print the
  rendered index/entity only.

---

## 6. Slash commands

### 6.1 Dispatch rules (DERIVED-FEATURE, ESSENTIAL as a mechanism)
Slash commands are intercepted **locally before the model loop**, inside every run
(`agent`, `agent-resume`, each `agent-chat` turn, GUI and service chat).

1. Input is trimmed. Not starting with `/` -> normal prompt, nothing special.
2. Parse: strip the `/`, split on single spaces; first word is the command name;
   if the second word is the literal `(MCP)` it becomes part of the name and is
   stripped again for lookup (marks MCP-provided commands); the remainder rejoined
   by single spaces (then trimmed) is the argument string. A lone `/` (empty name)
   yields a local reply explaining the `/command [args]` form.
3. Lookup is **case-insensitive** across each command's names (primary + aliases).
4. Not found: if the name consists only of letters, digits, `:`, `-`, `_`, reply
   locally `Unknown skill: <name>` (the model is not called); if it contains any
   other character (e.g. `/etc/passwd`, `/path/like`), treat the whole input as an
   ordinary prompt and send it to the model.
5. Two result kinds:
   - **local**: output text is returned as the entire run result: final output =
     the text, 0 turns, 0 tool calls, transcript = [user: the raw input,
     assistant: the output], no model call, no usage. The session id reported is
     the currently active one (possibly none). Not saved as a new session by
     itself.
   - **prompt**: the handler substitutes an expanded prompt that goes through the
     normal model loop (hooks and plugin pre-prompt hooks apply to the expanded
     text); the transcript starts with the user's literal input.
6. `/help` output lists every command as `- /<primary> (<aliases>): <description>`
   under a `# Slash Commands` heading plus a closing line saying these are handled
   locally.
7. A separate "Skill" path exists in the runtime (the model's skill tool resolves a
   name through the same registry; bundled skills are listed by `/skills`). Its
   spec belongs with the tools document.

The Go rewrite needs only the mechanism and the commands rated ESSENTIAL/USEFUL
below. It need not reproduce the upstream names list for DROP-CANDIDATEs.

### 6.2 Command catalogue

Columns: names (first = primary, rest aliases); arguments; effect; model?
(`no` = local, `yes` = expands to a model prompt); rating.

#### Session and context
| Names | Arguments | Effect | Model | Rating |
|---|---|---|---|---|
| `help`, `commands` | none | list all commands | no | ESSENTIAL: discoverability |
| `context`, `usage` | optional prompt text | show estimated context usage (optionally as if that prompt were added) | no | USEFUL |
| `context-raw`, `env` | none | dump raw environment/user/system context snapshot | no | DROP-CANDIDATE: debug dump |
| `token-budget`, `budget` | none | show token window, reserves, prompt limits | no | USEFUL |
| `status`, `session` | none | short runtime/session summary | no | USEFUL |
| `clear` | none | clear ephemeral runtime state (task/plan/etc. memory held by the process); replies with a confirmation sentence | no | USEFUL |
| `compact` | optional custom instructions | summarise the conversation to free context; success: `Conversation compacted.` plus (when counts known) `Tokens before: ~X -> after: ~Y` with thousands separators; failure: `Compact failed: <reason>` | yes (internal summarisation call, not a user-visible prompt) | ESSENTIAL: local models have small windows |
| `cost` | none | totals: cost (4 decimals under $0.01 else 2), input, output tokens, plus cache-read, cache-creation, reasoning tokens only when non-zero, and total | no | USEFUL |
| `stats` | none | message counts by role, token totals, cost (4dp), model, session id | no | USEFUL |
| `exit`, `quit` | none | **terminates the process with status 0** after printing a randomly chosen short farewell (4 variants). In `agent-chat` the bare words are caught earlier by the loop (3.4) | no | ESSENTIAL (as loop word); the random farewell is DROP-CANDIDATE |
| `resume`, `continue` | optional session id or search text | with a matching id: report message count and tell the user to restart with the resume flag; else list up to 20 most recent sessions (id, message count, model, first user message truncated to 80 chars), filterable by text; handles "no directory"/"no sessions" | no | USEFUL, but it only *describes* resuming; real resume is `agent-resume` / `--resume-session-id` |
| `rewind`, `checkpoint` | optional message index | no arg: list every message as `index: [ROLE] preview(60 chars)`; with index: truncate the in-memory conversation to keep messages 0..index, reporting how many were removed; validates range | no | USEFUL |
| `branch` | optional name | copy current transcript to a new session file (random id; fields: session_id, branch_name, branched_from, messages, model); prints name, new id, path | no | DROP-CANDIDATE: a branch file cannot be resumed through the normal path |
| `tag` | optional tag | no arg: show tags; with tag: toggle it on/off in memory (process-local) | no | DROP-CANDIDATE: tags are never persisted or searched |
| `rename` | name | set an in-memory session name | no | DROP-CANDIDATE: not persisted |
| `copy` | optional N (0-20) | write the Nth-most-recent non-empty assistant message to `<tmpdir>/sarina/response.md`; reports char and line counts and path | no | DROP-CANDIDATE: clipboard stand-in |
| `export` | optional filename | write plain-text transcript (`--- ROLE ---` / `--- TOOL:name ---` blocks) to `<cwd>/<name>.txt` (default `conversation_<YYYYMMDD_HHMMSS>.txt`); reports count and path | no | USEFUL |
| `files` | none | list unique file paths the model read in this session, relative to the workspace | no | DROP-CANDIDATE |

#### Model and settings
| Names | Arguments | Effect | Model | Rating |
|---|---|---|---|---|
| `model` | optional model name | no arg: `Current model: X`; with arg: switch model for this agent instance, `Set model to X` | no | ESSENTIAL: local operators swap models often |
| `effort` | optional `low|medium|high|max|auto` | show or set effort; env `CLAUDE_CODE_EFFORT_LEVEL` overrides and blocks changes; invalid level lists valid ones | no | DROP-CANDIDATE: no local-model meaning (rename env var if kept) |
| `fast` | optional `on|off` | toggle an in-memory flag; bare toggles | no | DROP-CANDIDATE: flag has no consumer |
| `config`, `settings` | none / `effective` / `source <name>` / `get <key>` | show config summary, merged config, one source, or one dotted key; bad subcommand prints usage line | no | USEFUL |
| `permissions` | none | show active tool permission mode | no | ESSENTIAL: security visibility |
| `tools` | none | list registered tools and whether permissions allow each | no | ESSENTIAL |
| `trust` | none | show workspace trust mode, managed settings, safe env values | no | USEFUL |
| `hooks`, `policy` | none | show discovered hook/policy manifests | no | USEFUL |
| `prompt`, `system-prompt` | none | print effective system prompt | no | USEFUL |
| `memory` | none | show loaded CLAUDE.md bundle and discovered files | no | USEFUL (note: does *not* show curated memory; that is `memory-status`) |
| `add-dir` | path | validate exists/is a directory/not already inside cwd/not already added; add as an extra working dir for tools | no | USEFUL |
| `sandbox-toggle`, `sandbox` | none / `exclude "<pattern>"` | show sandbox on/off and excluded-command patterns; `exclude` appends a pattern (quotes stripped) to config key `sandbox.excludedCommands` in the local config source, rejecting duplicates | no | USEFUL: only meaningful if the new core has a sandbox |
| `theme` | optional name | list themes (`light`, `dark`, `light-daltonized`, `dark-daltonized`, `light-ansi`, `dark-ansi`) marking current, or set config key `theme` | no | DROP-CANDIDATE: no TUI theming in scope |
| `vim` | none | toggle in-memory editor mode flag | no | DROP-CANDIDATE |
| `keybindings` | none | create (if absent) `<cwd>/.claude/keybindings.json` from a small template, report path and `$EDITOR` hint | no | DROP-CANDIDATE |
| `output-style` | none | prints a deprecation notice pointing to `/config` | no | DROP-CANDIDATE |
| `voice` | optional `on|off` | toggle config key `voiceEnabled`; when turned on, tells the user it does not start listening and to run `sarina voice-chat` (plus an ALSA check hint on Linux) | no | USEFUL: bridges to the voice feature; ORIGINAL behaviour |
| `doctor` | none | checklist: interpreter version, git present, model, base URL, workspace and session directory existence, API key set, tool count, CLAUDE.md present. (A Go rewrite replaces the interpreter line with a build-version line.) | no | USEFUL |
| `version` | none | one line with product name, version, runtime | no | USEFUL |
| `ide` | none | report detected IDE/terminal from env vars | no | DROP-CANDIDATE |

#### Tasks, plans, delegation
| Names | Arguments | Effect | Model | Rating |
|---|---|---|---|---|
| `tasks`, `todo` | optional status filter | list runtime tasks | no | USEFUL |
| `task-next`, `next-task` | none | next actionable tasks | no | USEFUL |
| `task` | id | show one task; usage text when missing | no | USEFUL |
| `plan`, `planner` | none | show current runtime plan | no | USEFUL |
| `agents` | none/`list`/`active`/`all`/`show <type>`/`<type>`/`create|update|delete ...` | list definitions, show one, or mutate. `create [user|project] <type> [description] [:: system prompt]`, `update` likewise, `delete [user|project] <type>`; `::` separates fields; default source project for create, auto for update/delete; usage text on bad forms | no | USEFUL: mirrors the `agents*` CLI |
| `skills` | none | list user-invocable bundled skills with aliases and a when-to-use line | no | DROP-CANDIDATE: bundled skills are upstream content |
| `workflows`, `workflow`, `triggers`, `trigger`, `teams`, `team`, `messages` | list with optional query; `workflow|trigger [run] <name|id>`; `team <name>`; `messages [team]` | same data as the corresponding CLI groups in section 5 | no | DROP-CANDIDATE |

#### Integrations (read-only views and toggles over section 5 runtimes)
| Names | Arguments | Rating |
|---|---|---|
| `mcp` | none / `tools [query]` / `tool <name>` / `<server>` | USEFUL |
| `resources`, `resource` | optional query / uri | USEFUL (follows MCP) |
| `search` | none / `providers [q]` / `provider <n>` / `use <n>` / `<query>` | USEFUL (the final form runs a live web search) |
| `worktree` | none / `history` / `enter [name]` / `exit [keep|remove] [discard]` | USEFUL |
| `lsp` | none / `symbols|workspace|definition|references|hover|hierarchy|incoming|outgoing|diagnostics ...` (usage line on malformed args) | DROP-CANDIDATE |
| `remote`, `remotes`, `ssh`, `teleport`, `direct-connect`, `deep-link`, `disconnect` (alias `remote-disconnect`), `remote-env`, `bridge` (aliases `remote-control`, `rc`), `remote-setup` (alias `web-setup`) | target or profile names | DROP-CANDIDATE: cloud remote-control simulation; `bridge`/`remote-setup` only report "not implemented" |
| `account`, `login`, `logout` | `profiles|profile <n>` / target / none | DROP-CANDIDATE |
| `ask` | none / `history` | DROP-CANDIDATE |
| `reload-plugins`, `plugin` | none / `list|help` | USEFUL if a plugin system is kept: reload manifests and print counts (plugins, tools, hooks, virtual tools); list shows name, version, description, tool/hook names |

All local commands above that take a required argument reply with a one-line
usage message and do nothing when it is absent.

#### Prompt-type commands (expand to a model prompt)
| Names | Arguments | Expansion (what it must convey) | Rating |
|---|---|---|---|
| `commit` | optional extra instructions | gathers branch, short status, staged diff, unstaged diff, last 10 commits (git calls with 15 s timeouts, failures yield empty text; "no git changes detected" if all empty) and asks the model to stage specific files, write a repo-style message, only create **new** commits, never skip hooks, never commit secrets, no interactive git flags | USEFUL |
| `pr-comments`, `pr_comments` | optional PR ref | asks the model to use the `gh` CLI to fetch PR-level and review comments and present them with file/line context and no commentary | DROP-CANDIDATE: GitHub-specific convenience |
| `btw` | question | asks for a concise text-only answer with an explicit instruction not to modify files, run shell commands, or alter the workspace; usage line if empty | USEFUL |
| `init` | none | asks the model to analyse the codebase and write a context file (commands to build/lint/test incl. single-test, big-picture architecture, no generic advice, fold in existing rules/README, do not invent sections, fixed header). The new product should name its own context file and write a fresh prompt | USEFUL |

Local utility commands without model: `diff` (git diff; falls back to staged diff
labelled as staged; "no uncommitted changes"; git missing/timeout messages) -
USEFUL; `release-notes` (first `## ` block of CHANGELOG.md/docs/CHANGELOG.md, else
a pointer) - DROP-CANDIDATE.

#### Marketing / account promotion commands
`feedback` (alias `bug`), `upgrade`, `stickers`, `mobile` (`ios`, `android`),
`desktop` (`app`), `install-github-app`, `install-slack-app`, `privacy-settings`,
`extra-usage`, `passes`, `rate-limit-options`, `chrome`: each prints or opens a URL
or a short blurb about the upstream vendor's products (browser opened unless env
`CLAUDE_CODE_NO_BROWSER` or `CI` is set). **DROP-CANDIDATE for all**: they
advertise a third party's hosted services and have no relationship to a local
security agent. Do not reimplement.

---

## 7. Observed non-obvious behaviours worth testing in the rewrite

- `agent "/help"` runs without any backend reachable (slash local path).
- `/Context` (mixed case) works; `/model` mutates only the in-process agent, it does
  not persist.
- A path-like first token (`/usr/bin/foo`) is a model prompt, not an error.
- Slash replies produce 0 turns and 0 usage; `agent-chat` prints the full usage
  block for them anyway (zeros).
- `agent-chat` with `--resume-session-id` reloads the stored session each turn
  from the configured session directory (not cached).
- Auto memory review runs on `/exit`, EOF and interrupt exits only (not on
  `/exit extra`, not on a crash).
- `fleet-launch` exit codes: 0/N from the launched agent, 1 cancelled, 2 could not
  ask/plan.
- Offline tests: every command that does not call a model must run with no
  backend and no network; the model-calling ones are tested with an injected
  OpenAI-compatible HTTP stub (a stubbed `urlopen` in the Python tests; in Go, an
  `httptest` server).
- Windows is not a reference platform; POSIX-specific behaviours (background
  process kill, symlink guards, stdio MCP) are verified on Linux.

## 8. Suggested scope for the Go CLI

ESSENTIAL set: `agent`, `agent-chat`, `agent-resume`, `agent-bg` family (one
copy, not two), `agent-prompt`, `agents*`, `memory-status`, `memory-show`,
`memory-review*`, `session-search`, `chymaera-*`, `fleet-*`, `limbo-*`, `voice-chat`
(via the Python helper), `desktop-audit` (via the Python helper); slash mechanism
plus `help, compact, model, permissions, tools, status, cost, stats, export, diff,
rewind, add-dir, doctor, version, voice, config, tasks, plan, init, btw, commit,
exit/quit`. Everything marked DROP-CANDIDATE in sections 3, 5 and 6 can be left out
of the first Go release without losing a documented Sarina capability.

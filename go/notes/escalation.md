# Escalation to the `claude` CLI (tier kind `claude_cli`)

Port of the original `escalation_runtime.py` + `system_resources.py` into `go/tiers`. Written from those two
originals, their tests, and spec 01 section 11 only.

## What it does

A third tier kind, `claude_cli`, next to `openai_compat` and `claude`. When the router moves up to it, the model
call becomes a hand-off: the local `claude` executable is run in print mode and its final text is the turn's
answer (no tool calls come back). It is a fallback, so it can never be the first tier.

- **Off by default, opt-in** (as the original): no `claude_cli` tier exists unless tiers.json declares one, or
  `tiers.FromEscalation` is given `escalation_config.enabled` / `--escalate-to-claude-code`.
- **`auto` is true** once enabled: the original ran without asking after opt-in. (The opt-in itself is the consent.)
  Set `"auto": false` to get the existing once-per-session confirmation instead.
- **Command** (verified flag set, unchanged): `claude -p <prompt> --output-format json --cwd <dir> --bare`, then
  `--add-dir <d>` per extra directory, `--permission-mode acceptEdits` only when the local agent could write or run
  shell (`accept_edits`), then `extra_cli_args`. A bypass-permissions mode is never emitted (tested).
- **Prompt**: original task (latest real user message; `<system-reminder>` boilerplate skipped), the escalation reason,
  a digest of up to the last 8 user/assistant/tool messages (`[role] text`, 400 chars each, `(no prior progress)` if
  none), and a request to finish. The prompt is built from the guarded request.
- **Result**: exit 0 + one JSON object + `is_error` falsy = success (`result`, `session_id`, `total_cost_usd` read).
  Timeout, launch failure, non-zero exit, bad/non-object JSON, `is_error` = a `*tiers.CLIError`, never a panic and
  never retried on the same tier. Previews are 2,000 chars.
- **Recorded**: every hand-off adds a `tier_cli_result` entry (`action: escalate_claude_code`, `ok`, `cli_session_id`,
  `total_cost_usd`, previews, `error`) to `plugin_state.compute_tiers.escalations`, next to the normal
  `tier_escalation` entry. The operator sees one line per escalation and one per result.
- **Guards**: default `allow_data` is `no_secrets` + `no_captures` (existing guard: secrets redacted in the prompt,
  capture dumps refuse the tier). The child environment drops every secret-named variable (names containing KEY,
  TOKEN, SECRET, PASSWORD, PASSWD, CREDENTIAL, AUTH, COOKIE, PRIVATE, SESSION), every tier's `api_key_env`,
  `OPENAI_API_KEY`/`ANTHROPIC_API_KEY`/`ANTHROPIC_AUTH_TOKEN`, and any variable holding a known secret value.
  The `claude` CLI therefore has to authenticate from its own stored login, not from the environment.
- **Kill**: timeout or cancel kills the whole process tree (process group on Unix, `taskkill /T /F` on Windows).

## Triggers

| Original stop reason | Here |
| --- | --- |
| `backend_error` | every lower tier unusable (unreachable, unhealthy, repeated timeouts) |
| `resource_exhausted` | first tier under resource pressure (memory, CPU, GPU) |
| `budget_exceeded` | **not handled by the router** (it is an agent-loop decision); accepted in config, warned, ignored |

`trigger_stop_reasons` / `--escalation-trigger` select which of the first two may hand off (default both).
`max_escalations` (default 1) caps hand-offs per router instance (one process / session run); it is not yet
restored from a resumed session file.

## Resource pressure (merged with the existing sampler)

`Sample`/`SystemSampler`/`Thresholds` gained CPU; there is still one sampler. Order is memory, CPU, GPU (the
original's order). Memory: `/proc/meminfo` or `GlobalMemoryStatusEx`. CPU: two reads 0.1 s apart (`/proc/stat`,
`GetSystemTimes`), only sampled when a CPU threshold is set. GPU: first GPU via `nvidia-smi`, cached 10 s. Unknown
never trips. Pressure stays advisory (spec 11.2 recommendation): with nowhere better to go the run stays local.

Differences from the original: CPU is **opt-in** (default off, the original defaulted to 90 and could abort a healthy
run on the model's own host); the **min-disk-free** check is not ported (it was unset by default).

## Config / flags / env mapping

| Original | Now |
| --- | --- |
| `escalation_config.enabled`, `--escalate-to-claude-code` | `tiers.FromEscalation` adds tier `claude-cli`; or declare `{"kind":"claude_cli"}` in tiers.json |
| `cli_path`, `--claude-code-cli-path`, `CLAUDE_CODE_CLI_PATH` | tier `cli_path` (flag > config > env > `claude`; the env only applies while the path is the default) |
| `trigger_stop_reasons`, `--escalation-trigger` | tier `trigger_stop_reasons` |
| `max_escalations` | tier `max_escalations` (default 1) |
| `timeout_seconds` | tier `timeout_seconds` (default 300) |
| `extra_cli_args` | tier `extra_cli_args` |
| permissions (write/shell allowed) | tier `accept_edits` / `EscalationOptions.AcceptEdits` |
| additional working dirs | tier `add_dirs` / `EscalationOptions.ExtraDirs` |
| `resource_thresholds.max_memory_percent` / `max_cpu_percent` / `max_gpu_memory_percent` | tiers.json `resource.*` (same names); `FromEscalation` copies them over the defaults |
| `--disable-resource-check` | `EscalationOptions.DisableResourceCheck` zeroes all thresholds |
| `resource_thresholds.min_disk_free_mb` | not ported |

tiers.json example:

```json
{"resource": {"max_memory_percent": 90, "max_cpu_percent": 95},
 "tiers": [{"name": "local"},
           {"name": "claude-cli", "kind": "claude_cli", "cli_path": "claude", "timeout_seconds": 300,
            "max_escalations": 1, "trigger_stop_reasons": ["backend_error", "resource_exhausted"]}]}
```

## Wiring (done)

Flags are registered once, in `runkit.EscalationFlags`, and reused by every front end:

| Original flag | Go flag | Where |
| --- | --- | --- |
| `--escalate-to-claude-code` | `--escalate-to-claude-code` | chat, resume, skill run, memory-review, `tiers status`, `service`, `gui` |
| `--claude-code-cli-path` (+ env `CLAUDE_CODE_CLI_PATH`) | `--claude-code-cli-path` (env applies while the path is the default) | same |
| `--escalation-trigger` (repeatable) | `--escalation-trigger backend_error\|resource_exhausted` | same |
| `--disable-resource-check` | `--disable-resource-check` | same |

No other original escalation flags exist; timeout, max escalations and extra args are tiers.json tier fields.
Permissions come from the run itself: `--allow-write`/`--allow-shell` (the effective session permissions in service/gui)
become `--permission-mode acceptEdits`; `--add-dir` becomes `--add-dir` for the child. The child's working directory
is the run's workspace (`--cwd`, the session's cwd, the GUI workspace).

- CLI: `opts.tierRouter` loads tiers.json, applies `tiers.FromEscalation`, sets `Options.Workdir`; chat, resume and
  `/tier` all use that one router. `sarina tiers status` (and `/tier status`) lists the `claude-cli` tier with the
  executable path and `(found)` / `(not found)`; a missing executable also shows the tier as unavailable.
- Service and GUI: `service.Config.Escalation` / `gui.Config.Escalation` are set only from the operator's command-line
  flags of `sarina service` / `sarina gui`. They are not part of any request body or the GUI `State`, so callers and
  the web page cannot enable the tier or change the executable (tested). `/api/tiers` rows for the tier carry
  `cli_path` and `cli_found`.
- `escalation_config` inside a stored session file is deliberately **ignored** (a workspace could ship a session file
  naming an executable). Only operator flags, tiers.json (the operator's home) and `CLAUDE_CODE_CLI_PATH` count.
  Consequently `escalation_config.resource_thresholds` from old session files no longer applies; use tiers.json
  `resource.*`.
- Env alone never enables escalation; `CLAUDE_CODE_CLI_PATH` only supplies the path once a flag or tiers.json enabled it.

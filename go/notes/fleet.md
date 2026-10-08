# Agent fleet (status)

Ported from the owner's ORIGINAL modules `chymaera/agent_usage.py`, `agent_fleet.py`, `agent_launch.py` and their three
test files, plus `docs/ECOSYSTEM.md` (agent fleet) and spec 00 / 04-cli (fleet-* sections). No derived file was read.
Standard library only. `go vet ./...` and `go test ./...` pass.

Package `go/chymaera/fleet`; `cmd/sarina/main.go` gained one blank import (commands register via `cli.Register` in `init()`).

| File | What it holds |
| --- | --- |
| `usage.go` | `fleet-usage`: reads saved sessions (`session_id`, `model_config.model`, `turns`, `usage.*`), builds Omarchy's record (same keys, key order, 7 recent days, compact ASCII JSON), atomic `WriteRecord` |
| `status.go` | `fleet-status`: untrusted-record parser (id regex, control/bidi stripping, 80-char cap, clamped numbers, 16 limits max, 1 MiB cap, staleness), `SuggestAgent`, `RenderStatus` |
| `launch.go` | `fleet-launch`: forms table (no approval-skipping flag anywhere), `PlanLaunch` (pure), `Launch` (needs `confirmed`, prompt-hash check, audit-before-run), JSONL audit |
| `command.go` | the three commands and their exit codes |

## Behaviour kept

- `fleet-usage`: compact one-line JSON on stdout; `--write` writes `sarina.json` (temp file + rename) and prints its path;
  `--force`/`--limits-only` accepted and ignored; `--session-dir` repeatable and replaces defaults; `SARINA_SESSION_DIRS`
  replaces defaults too. Defaults: `<cwd>/.port_sessions/agent`, `~/.port_sessions/agent`, `/var/lib/sarina/sessions`.
  Same session id in several directories counts once (newest mtime wins); a session's tokens go to the local day of its mtime.
- `fleet-status [--usage-dir D] [--suggest]`: exit 0 always. `--suggest` adds a blank line then `Most headroom: NAME (N% left in
  its tightest window)` or a sentence saying no agent has a current measured limit.
- `fleet-launch AGENT [PROMPT] [--cwd D] [--yes]`: proposal printed first; `Launch? [y/N]` unless `--yes`. Exit: launched agent's
  own status; 1 = cancelled (`Cancelled.`, audited as refused); 2 = non-TTY stdin without `--yes`, EOF at the question, unknown
  agent, bad arguments, planning error (`fleet-launch: <reason>`), missing binary. No auto-approve option exists (any such flag
  is an unknown flag, exit 2); `--yes` only skips the question. Flags may come before/after positionals; a prompt starting with
  `-` is given after `--`.
- Audit: JSON lines at `$XDG_STATE_HOME/sarina/agent-launch.jsonl` (else `~/.local/state/sarina/...`), keys `time agent argv0
  cwd promptSha256 promptLength promptPreview(60 chars, newlines to spaces) outcome detail`; the prompt itself is never stored.

## The invariant

`invariant_test.go` fails if (a) any tool in `tools.Builtins()` / a registry is named like fleet-launch/fleet_launch/agent-launch
(or fleet-usage/status), or describes the launcher; (b) any `.go` file outside `go/chymaera/fleet` imports the fleet package other
than `cmd/sarina/main.go` (blank import only); (c) any non-test `.go` file outside the package contains `fleet-launch`,
`fleet_launch`, `FleetLaunch` or `fleetLaunch`; (d) the tool/agent/service/gui packages import it. It also checks the three
commands are registered as CLI commands. Do not weaken it.

## Deliberate deviations

- Safer than the original: if the audit entry cannot be written, nothing is launched (the original would have raised before
  running too, but this is explicit and tested). `--cwd` must be an existing directory (exit 2).
- Messages for exit-2 conditions go to stdout (the spec's "expected failures print to stdout" convention); argparse-style
  usage errors (unknown flag, wrong argument count, unknown agent) go to stderr, exit 2.
- Insertion order of `todayTokensByModel` / `modelUsage` is kept (first-seen session order), as a Python dict would.
- JSON files containing `NaN`/`Infinity` literals (accepted by Python's `json.loads`) are skipped as unparseable.
- The `--suggest` "nothing to compare" sentence is new text (the original's exact wording was not in the allowed sources).

Tests: ~84 offline tests (`usage_test.go`, `status_test.go`, `launch_test.go`, `invariant_test.go`); the hostile-record set
(control chars, bidi, truncation, bad ids, inf/NaN/negative/boolean numbers, capped limits, oversized/malformed files) is ported
one-for-one plus extras.

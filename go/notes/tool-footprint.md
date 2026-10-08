# Tool-definition footprint (status)

Problem: the request carried 30 tool definitions, about 21.5k JSON chars (~5.4k tokens), most of the baseline prompt,
and the same set was sent in read-only sessions where write/shell tools can only be refused.

## Changes

1. **Denied tools are not offered.** `tools.Builder.Perms` (set by the agent from `Runtime.Permissions`, so the CLI,
   the service, the GUI and sub-agents all follow it) hides every tool whose `Requires` is not granted: `write_file`,
   `edit_file`, `task_create`, `task_update`, `todo_write`, `update_plan`, `memory_add/replace/remove` need write;
   `bash`, `TaskStop`, `limbo_validate` need shell. Like plan mode/narrow/deny it only removes tools. The tools stay known
   to the registry, so a call that arrives anyway still reaches the permission gate and is refused with the
   `--allow-write` / `--allow-shell` message and `error_kind=permission_denied` (tested in `tools` and by the existing
   agent denial test). The system prompt's "restricted" notice now says the blocked tools are not offered and that the
   operator must restart with the flag.
   GUI: its approval prompt sits after the permission gate, so a write/shell tool already needed `allow_write` /
   `allow_shell` to ever run. With the flag on (approval mode `ask` or `auto`) the tools are offered and still go through
   the approver; with it off they were always refused, now they are not offered. No behaviour change for approved calls.
2. **Terse descriptions** for every tool and parameter (names, types, required-ness, bounds and behaviour untouched).
   `tools.prop` now omits an empty description, and self-explanatory parameters have none.
3. **`agent` description** now says: use it for a separate, self-contained investigation; call read_file/grep_search/
   glob_search directly for a single read or search. Built-in profile blurbs shortened (they are listed in the tool).
4. **Footprint test** `cmd/sarina/footprint_test.go` (the only package that links every tool provider, limbo included)
   fails above target + 10%: all permissions 11,000 (limit 12,100), read-only 5,500 (limit 6,050).
5. **Pre-flight stop message** (`prompt_too_long` kept) now states the window and its source (`--context-window`,
   `SARINA_CONTEXT_WINDOW`, backend discovery with which probe, or inferred from the model name) and, for a window <= 8192,
   the Ollama hint (`PARAMETER num_ctx 16384` / `OLLAMA_CONTEXT_LENGTH`, or `--context-window`). README requirements note added.

## Numbers (JSON of the tool list in the request, default profiles, no skills/MCP)

| Case | Tools | Before | After |
| --- | --- | --- | --- |
| all permissions | 30 | 20,535 | 10,901 |
| read-only (no write, no shell) | 13 | 20,535 (same 30 sent) | 4,000 |

(The 21,527 figure in the report includes slightly different wrapping; the test measures `json.Marshal` of the specs.)
Test changes: tests that assumed write/shell tools in a default session now pass `--allow-write/--allow-shell` or set the
permissions; the Limbo "honest description" test now checks the short wording (disposable, untrusted, not permission). The
guidance about sudo/network stays in the `limbo_profiles` result text.

## To-do/plan family merged (2026-10-06)

The 13 overlapping tools are now 6 (`task_list`, `task_create`, `task_update`, `todo_write`, `plan_get`, `update_plan`);
nothing was lost, every old call is a parameter. Mapping and sizes: `todo-tools-merge.md`. Family spec size 4,879 -> 4,725
chars; whole request 30 -> 23 tools, 11,784 -> 11,623 chars (all permissions), 13 -> 11 tools, 4,139 -> 3,997 (read-only).
Budgets in `cmd/sarina/footprint_test.go` lowered to 10,800 / 4,000. The saving is mostly tool count, not characters: the
removed tools had terse one-line descriptions, and `task_update` and `update_plan` now carry the transition and clear
guidance that used to be implied by the tool name.

## Correction (2026-10-06): do not compress behavioural guidance

The footprint pass shortened the Limbo tool descriptions too. That removed guidance
the Limbo side had added on purpose after real-model failures (use `sudo` for
root commands if the profile allows it, a failed download may mean the profile has
no network, the timeout covers the command only, never treat a pass as permission).
With the short text, qwen3 stopped calling `limbo_validate` in the real-model
scenario (3 failed checks, reproduced twice); with the original text restored the
script passes 16/16 and the model reports a failed run honestly. The original
descriptions for `limbo_profiles` and `limbo_validate` are back, and the
"honest descriptions" test in `go/limbo` guards them. Rule: trim padding, not
instructions; check any tool-description change against
`integration-real-model.sh`.

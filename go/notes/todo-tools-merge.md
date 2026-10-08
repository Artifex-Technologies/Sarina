# To-do/plan tools merged: 13 -> 6 (2026-10-06)

Retained: `task_list` (read-only), `plan_get` (read-only), `task_create`, `task_update`, `todo_write`, `update_plan`
(all need `--allow-write`, plan mode unchanged: the two readers stay read-only, the rest mutating).
`enter_plan_mode` / `exit_plan_mode` are untouched. On-disk files (`task_runtime.json`, `plan_runtime.json`) are unchanged
and old files load as before.

| Old call | New call |
| --- | --- |
| `task_get {task_id}` | `task_list {task_id}` (unknown id is still an error) |
| `task_next {max_tasks}` | `task_list {actionable_only: true, max_tasks}` (empty result says "Nothing is ready to work on.") |
| `task_start {task_id, owner, active_form}` | `task_update {task_id, status: "in_progress", owner, active_form}` (still becomes blocked, `waiting_on:<ids>`, if a `blocked_by` task is unfinished) |
| `task_complete {task_id}` | `task_update {task_id, status: "completed"}` (releases tasks blocked on it) |
| `task_cancel {task_id, reason}` | `task_update {task_id, status: "cancelled", reason}` (`cancel_reason`, releases dependants) |
| `task_block {task_id, reason}` | `task_update {task_id, status: "blocked", reason}` (`blocked_reason`) |
| `task_block {task_id, blocked_by: [...]}` | `task_update {task_id, status: "blocked", add_blocked_by: [...]}` |
| (new capability) | `task_update {remove_blocked_by: [...]}`; `blocked_by` still replaces the whole list |
| `plan_clear {}` | `update_plan {clear: true}` (tasks untouched) |
| `plan_clear {sync_tasks: true}` | `update_plan {clear: true, prune_tasks: true}` (removes only plan-created tasks) |
| `todo_write`, `update_plan` (items), `plan_get`, `task_create`, `task_list` filters | unchanged |

Behaviour notes:
- Releasing is unchanged: any `task_update` re-checks blocked tasks; only a blocked task that has `blocked_by` entries, all
  finished, goes back to pending. A task blocked by a reason alone is never released by someone else's completion.
- Deliberate change: `task_update {status: "in_progress"}` now follows the old `task_start` rule (blocked if a dependency is
  open) instead of forcing in_progress. The old unconditional set is still possible by clearing the dependency first
  (`remove_blocked_by`) or by `blocked_by: []`.
- `update_plan` needs `items` (an empty list is allowed) or `clear: true`; both together is an error. With `clear`, the task
  file is not even rewritten unless `prune_tasks` is true.
- A model that sees an old name in a saved history gets "unknown tool" for it; the descriptions of the new tools name the
  parameters that replaced them, and no alias tools exist.
- `task_*` and the plan tools remain unavailable to sub-agents (`childDenied`: `task_` prefix, `todo_write`, `update_plan`,
  `plan_get`). GUI and service read the files directly and never named these tools, so nothing there changed.

Serialized size (JSON of the tool specs): to-do/plan family 13 tools 4,879 chars -> 6 tools 4,725 chars. Whole request, all
permissions: 30 -> 23 tools, 11,784 -> 11,623 chars; read-only: 13 -> 11 tools, 4,139 -> 3,997 chars.

## Status: dependency rules, usage guides, session refresh (2026-10-06)

Supersedes the `update_plan` bullet above (both/neither is no longer an error).

1. **Dependency rules** (`tools/todo.go`). A dependency blocks while its task exists and is not completed. A cancelled one
   still blocks, and `task_list`/`task_update` show `waiting_on: a (cancelled - remove or replace it)`; cancelling a task
   lists the dependants still waiting on it. An id that names no task is reported ("depends on unknown task X (ignored,
   not blocking)") and never blocks. Cycles and self-dependencies are refused with the path named
   (`dependency cycle a -> c -> b -> a`) in `task_create`, `task_update` (`blocked_by`, `add_blocked_by`), `todo_write` and
   `update_plan` (`depends_on`, also against existing tasks); a refused call changes nothing. `task_list` shows why a task
   waits (`waiting_on` with each status, or `blocked_reason`). `normalizeTasks` runs on every load and after every
   mutation: in_progress with open dependencies becomes blocked (`waiting_on:<ids>`); blocked with all known dependencies
   completed, or blocked only by a `waiting_on` reason that no longer applies, goes back to pending; a stale `waiting_on`
   list is recomputed; a manually blocked task with no dependencies stays. Idempotent; a read only rewrites the file when
   something changed and the session has `--allow-write` (a read-only session normalises in memory only). The on-disk
   format is unchanged.
2. **Guides instead of errors.** `update_plan` with neither `items` nor `clear`, or with both, and `task_update` with only a
   `task_id`, return an ok result: two modes plus an example each and the current plan / task state, nothing changed
   (metadata `guide: true`). An unknown task id is still an error. Schema-level mistakes (items not an array) are still
   rejected by validation before the handler.
3. **Session refresh** (`session/migrate.go`). `session.Parse`/`Load` and `Agent.Resume` rewrite retired names in memory
   (`task_get`, `task_next`, `task_start`, `task_complete`, `task_cancel`, `task_block`, `plan_clear`): call name and
   arguments per the table above, result `name`, the `"tool"` field at the head of the result body, and metadata
   `tool_name`/`action` (a `plan_clear` action is kept, the current tool still records clears under it). Unparseable
   arguments and unknown names are left alone, with their results. Idempotent; persisted by the next save. New CLI
   `sarina session-migrate [--session-dir D] [--cwd D] [--dry-run]` refreshes every `*.json` in the session directory
   (atomic write, `<id>.json.bak` written once, counts scanned/changed/calls/results, unreadable files skipped). File keys and
   shapes are unchanged, so the Chymaera_Sentry contract is untouched.

Footprint after these changes: all permissions 11,822 chars, read-only 3,997 (budgets unchanged).

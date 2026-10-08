# New-feature designs (not ports)

These four features are rebuilt as new designs. They were requested by the
owner; the old versions were broken, dead or record-only. Nothing here is
described from the old code. Implementers use only this document, 00-must-preserve.md
and the existing go/ code.

---

## 1. Plan mode

**Purpose.** Let the operator have the agent investigate and propose a plan
before anything is changed.

**Behaviour.**
- States: `normal` and `plan`. The session file records the current state.
- Entering plan mode (model tool `enter_plan_mode`, operator command `/plan`):
  the tool registry is re-filtered to read-only tools (read, glob, grep, list
  background output, read-only MCP) plus `exit_plan_mode`. Write, edit, shell,
  and process-control tools are not offered to the model at all (not merely
  denied at call time). Registry rebuilds re-apply the filter.
- The model produces a plan as the argument of `exit_plan_mode` (free text,
  size-limited). Calling it does NOT leave plan mode by itself: it presents
  the plan to the operator and waits.
- Operator decisions: `approve` (return to `normal`, tools restored, plan kept
  as a session note and added to the next turn's context), `revise` (stay in
  plan mode, operator feedback is returned to the model as the tool result),
  `reject` (return to `normal`, plan discarded).
- Non-interactive runs (service, one-shot): a plan awaiting approval ends the
  run with stop reason `plan_pending` and the plan in the result; resuming the
  session with an approval flag continues.
- An allowlist set at session creation always wins: plan mode can only remove
  tools, never add one the allowlist excluded.

**Acceptance.** Entering plan mode removes mutating tools from the request sent
to the model; approval restores them; an empty allowlist stays empty throughout;
plan state survives resume; a session cannot enter normal mode except via the
operator decision.

---

## 2. Skills

**Purpose.** Named, reusable prompts the operator (or the model, when allowed)
can invoke, e.g. a recurring triage or report procedure.

**Behaviour.**
- A skill is a directory or single markdown file with front matter: `name`,
  `description`, optional `when_to_use`, optional `allowed_tools` (a subset of
  the registry the skill may use), optional `user_invocable` (default true) and
  `model_invocable` (default false).
- **Discovery is operator-controlled only**: `~/.sarina/skills/` and a path
  listed in an operator config file or `SARINA_SKILLS_DIR`. Skills inside the
  workspace/repository are NOT loaded automatically (a repo must not be able to
  inject prompts); `--trust-workspace-skills` opts in per run and says so in the
  output.
- Invocation: `/skill <name> [args]` or `sarina skill run <name>`; the model may
  call the `skill` tool only for skills marked `model_invocable`. A skill
  expands to a prompt (argument substitution is plain text substitution of
  `$ARGUMENTS` only) and, if `allowed_tools` is set, restricts the registry for
  that turn sequence.
- `sarina skill list` shows name, description, source path, and whether it is
  model-invocable.
- Name collisions: earlier search path wins; a collision is reported.
- Skill text is subject to the same secret/injection scan as curated memory
  before use (reuse the memory-security gate).
- No bundled skills ship initially. Skills the owner wants (for example a
  desktop-audit or Limbo-validation procedure) are added as files later.

**Acceptance.** A skill in the workspace is ignored without the opt-in flag;
`allowed_tools` can only narrow the registry; a non-model-invocable skill is
not offered to the model; unknown names fail with a clear error.

---

## 3. Session notes and size-based tool-output clearing

**Purpose.** Make long sessions on small local contexts cheap and reliable. The
old equivalents never ran; these are designed to actually trigger.

**Session notes.**
- The agent maintains a running notes file for the session at
  `.port_sessions/agent/<session_id>.notes.md` with fixed headed sections:
  task/goal, current state, files touched, decisions, open questions, next steps.
- Update trigger: after every N tool-using turns (default 6) or when context
  use crosses 50% of the window, whichever is first, run one short model call
  that rewrites the notes from the recent messages plus the previous notes. The
  last-summarised message index is stored in the session file next to the notes.
  Failure of this call is logged and ignored (never fails a turn).
- When compaction is needed, if the notes exist and cover all but the recent
  tail of messages, use the notes as the summary with no extra model call;
  otherwise fall back to the normal summarising/heuristic compaction.
- The cut is pair-preserving: a tool call and its result are never separated.
- Notes pass the memory-security scan before being re-injected after compaction.

**Size-based clearing.**
- When the estimated context passes a threshold (default 60% of window), older
  tool results (all but the most recent K, default 4) larger than M tokens
  (default 500) are replaced with a short marker stating the tool, size and that
  the output was cleared; the call and its pairing stay intact. Message metadata
  records the clearing so it is idempotent.
- Runs before summarising compaction, since it is cheaper.

**Acceptance.** Notes file is created and updated at the configured cadence;
compaction with fresh notes makes zero summarisation calls; clearing never
orphans a call; both are off-switchable by config; a failed notes update does
not abort the turn.

---

## 4. Compute tiers

**Purpose.** Run on the local device first, use the operator's desktop when the
device is not enough, and use Claude only as a last resort.

**Configuration** (operator-controlled file, `~/.sarina/tiers.json`, never read
from the workspace): ordered list of tiers, each with `name`, `kind`
(`openai_compat` | `claude`), `base_url`, `model`, `api_key_env` (name of an
environment variable, never the key itself), `allow_data` (what may be sent:
`all`, or a list of classes such as `no_secrets`, `no_captures`), `max_context`,
and optional `wake` (a command or URL to power on the desktop, run only if
configured). Default config: one tier, the local backend.

**Behaviour.**
- A turn starts on the lowest tier. Escalate to the next tier when: the local
  model server is unreachable or times out repeatedly; resource pressure
  (memory/GPU) crosses configured thresholds; the request will not fit the
  tier's context window; or the operator runs `/tier up`.
- Escalation never changes tools or permissions, and never sends data the next
  tier's `allow_data` forbids: material matching the secret/credential scan is
  redacted or the escalation is refused with a visible reason.
- Every escalation is shown to the operator (one line: from, to, reason) and
  written to the session event log. Escalation to a `claude` tier requires the
  tier to be enabled in config. Owner decision: it escalates automatically
  (`auto` defaults to true for the existing setup); setting `auto: false` asks
  for per-session operator confirmation instead. The `allow_data` secret guard
  applies either way.
- De-escalation: a new turn starts on the lowest healthy tier; a session's
  history is replayable on any tier (the message format is the same).
- The tier reached and each switch are recorded in the session file so the
  Sentry integration can see them.
- Health checks: a cheap request to the tier's models endpoint with a short
  timeout; cached for 30 seconds.
- The existing Claude hand-off behaviour (escalation helper) becomes the `claude`
  tier kind; its current opt-in/off-by-default stance is preserved.

**Acceptance.** With the local server down and a desktop tier configured, the
turn runs on the desktop; with the desktop also down and Claude disabled, the
run fails with a clear message and sends nothing anywhere; a request containing a
secret-shaped string is not sent to a tier whose `allow_data` forbids it;
escalations appear in the session file; API keys never appear in any file.

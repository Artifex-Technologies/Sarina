# Contracts the replacement must preserve

Other repos depend on these. They are acceptance criteria for the Go core.

1. **Tool-less advisory sessions (Chymaera_Sentry).** `POST /v1/sessions` with
   `tool_allowlist: []` creates a session with NO tools; the create response
   reports `"tools": 0`. Sentry refuses to send evidence otherwise. The tool list
   must be re-applied whenever the registry is rebuilt, and the model request must
   omit tools/tool_choice entirely when there are none. (`allow_shell=false` only
   denies at call time; it is not a substitute.)
2. **Limbo validation (Chymaera_Linux/Limbo).** Client talks only to a socket
   named `agent.sock`, found via `SARINA_LIMBO_SOCKET`. Tools `limbo_profiles`
   and `limbo_validate` (the latter gated by shell permission); CLI commands
   `limbo-profiles` and `limbo-validate`. Limbo's repo owns the socket contract
   (docs/QUARANTINE.md there).
3. **Sentry governance.** Memory writes consult `POST /v1/review` when
   `SENTRY_REVIEW_URL` is set; fail open on absence, closed on "deny".
4. **Session files.** Chymaera_Sentry reads the saved-session JSON (see
   01-agent-core.md for the exact keys).
5. **Packaging.** Console scripts `sarina`, `sarina-gui`, `sarina-service`;
   `pyproject.toml` and `packaging/arch/*` stay put until cutover; ports 8765
   (GUI) and 8899 (service). `fleet-launch` is CLI-only, never a model tool.

Conformance checks live in the Limbo repo: `scripts/integration-chain.sh`,
`integration-sarina.sh`, `integration-real-model.sh` (run in Arch WSL, with
Limbo and Sentry checked out beside Sarina).

## Added 2026-10-04 (from the Limbo session)

- Limbo's `GET /v1/profiles` on the agent socket returns `details` (network,
  sudo, max_seconds); `limbo_profiles` prints them.
- `limbo-validate` takes its options BEFORE the profile and treats everything
  after the profile as the guest command (e.g. `pacman -Syu --noconfirm` works).
- Hooks Limbo's code needs in the core: tool registry extension point (limbo
  tools are appended to the default registry) and a CLI subcommand extension
  point (add commands, dispatch by name).
- Model-request builder must omit tools/tool_choice when the tool list is empty
  (see item 1).
- A real local model (qwen3 8B) can be used for conformance runs through
  Limbo's `scripts/integration-real-model.sh` (needs `REAL_MODEL_URL`).

## Owner decisions (2026-10-04)

- Session format: the agent's own session file (`.port_sessions/agent/<id>.json`,
  messages as objects) IS the contract Chymaera_Sentry reads. Sentry is updated
  to match; the legacy flat shape is not carried over. The API key must never be
  written to session files.
- Every bug marked DO NOT REPLICATE in the specs is fixed in the rewrite.
- Go code lives in `go/` in this repo; Python keeps running until the Go side
  passes the conformance scripts, then the derived Python is deleted.
- Session-memory compaction and microcompaction are NOT dropped: rebuild them so
  they actually work (see go/ design notes), rather than porting the dead code.

## Owner decisions, round 2 (2026-10-04)

- KEEP and rebuild properly: `TaskStop` and `TaskOutput` (on the real background
  process runtime, not to-do items), plan mode (`EnterPlanMode`/`ExitPlanMode`,
  read-only until the operator approves the plan), and skills (named reusable
  prompts, operator-controlled locations only).
- Compute tiers (new feature, replaces the record-only "remote" simulation):
  local device -> operator's desktop (OpenAI-compatible endpoint on the LAN) ->
  Claude as last resort (existing escalation behaviour). Move up a tier on
  resource pressure or failure; never silently send data to a higher tier
  without the operator's configuration allowing it. Design to be written before
  it is built.
- Still dropped: record-only teams/workflows/remote-triggers/account, the regex
  "LSP" tool, vendor-promotion slash commands, plugin cache, bundled skills'
  existing text, `notebook_edit`, `sleep`, `paste_refs`, `context.py`.

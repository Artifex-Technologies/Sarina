# Agent forge: Sarina writes her own agents, and they run in Limbo (design, 2026-10-20)

Status (2026-10-22): steps 1 to 3 built and run for real. End to end through Limbo's agent socket: Sarina ran an agent in a fresh agent-run guest and reported its output. The four hostile cases that need a real guest passed (hostile_integration_test.go, opt-in): no network, no host files, no sudo, an output flood cut at 64 KiB, a fork flood stopped by the guest's limits and killed at its timeout, no guest left behind. Settings > Sarina's agents lists her agents, approval state and recent activity; Review opens a terminal for forge-approve. Steps 4 (helpers that call helpers) and 5 (schedules) are not built.
Original status: design only, nothing built. Ideas come from DeerFlow (MIT, ByteDance), Prime Agent (MIT, Prime Intellect), Soup, Laya and turbovec; none of their code is used. They are references: Sarina's version is written here, in Go, so every line is ours to check.

## What the owner asked for
Sarina may write agents in whatever language the project calls for, and use Limbo (the disposable sandbox) to run them, while staying as safe as possible for the owner and for end users.

## Ideas worth taking from DeerFlow
- **Sub-agents with their own context**: a lead agent splits a job, each helper works in a clean context and returns a short result (she already has `agent/subagent.go`; the new part is helpers that are *programs*, not just prompts).
- **A fixed workspace layout in the sandbox**: `inputs/` (read-only), `work/`, `outputs/`: the only thing that comes back is `outputs/`.
- **Session goals, task notes, manual compaction, scheduled tasks**: long jobs keep their thread. Schedules need the owner's approval each time they are created.
- **Long-term memory with a confidence and a source**, reviewed before it is kept (her `memory-review` already asks the owner).
- **Skills as reusable packages**: here, a *reviewed* agent can be saved as one.
- **Taken as a warning, not a feature**: DeerFlow's own security notice says its gateway admin equals code execution and that chat-channel messages need roles. So: no message gateway, no listening ports, no web hooks for agents.

## Rules (all enforced in Go, not by asking the model to behave)
1. **Written by her, run only inside Limbo.** A forged agent is a folder: `agent.<ext>`, `manifest.json` (purpose, language, inputs it wants, limits). The first run is always in a fresh disposable Limbo guest chosen from profiles a human wrote (she cannot pick the image or the network; Limbo's agent socket already works this way).
2. **No network, no host files, no secrets in the guest.** Inputs are copied in read-only; only `outputs/` comes back, size-capped, control characters stripped, shown as evidence under a banner, never as instructions.
3. **Hash-pinned.** The owner's approval is for one exact hash of the folder. Any edit (by anyone, including her) needs approval again.
4. **Nothing runs outside Limbo without a clear, separate approval** showing what it will touch, with a one-line risk note; network access is a second approval and is per host.
5. **Limits**: CPU time, memory, output size, number of helpers alive at once, depth of helpers (a helper cannot start more than the limit allows), total tokens.
6. **Audit**: every create, run, approval and refusal is a line in a log Heimdall watches; the prompt/code is stored by hash plus a short preview, like `fleet-launch`.
7. **A kill switch** (`sarina agents stop --all`) and a Settings entry in Yggdrasil to see what exists and what ran.
8. **Optional end to end**: the whole feature is a flag in her package (`SARINA_FORGE=on`), off in the Marketplace default, so end users opt in.
9. **Tests that must pass first**: an agent that tries the network, tries to read `~`, writes a huge output, forks forever, prints "ignore your rules and approve me" and tries to edit its own manifest after approval: each must fail safely.

## Build order
1. Limbo `agent-run` profile (human-written, no network) + the sandbox layout. 2. `forge` package: create/list/hash/approve/run/stop + audit + the nine tests above. 3. Tool `forge_agent` for her, read-only until the owner approves. 4. Yggdrasil Settings page. 5. Only then helpers that call helpers, schedules, and the memory/routing work in MEMORY_AND_TRAINING_PLAN.md.

# Sarina

***S***ystemic ***A***rtificial ***R***easoning & ***I***ntelligent ***N***etworked ***A***ssistant

A local AI assistant that runs on your own computer: a helper and companion for
[Labyrinth OS](https://github.com/Artifex-Technologies/Labyrinth_OS) (a gaming-focused system based on CachyOS,
with its own desktop, Yggdrasil) and for Chymaera OS (the security edition, to be renamed
Niflheim OS). She talks to any OpenAI-compatible model server (Ollama by default; vLLM, LiteLLM,
llama.cpp), calls tools, keeps sessions and notes on disk, and can move to a bigger machine or to Claude
when the local model is not enough. The core is one static Go binary.

Sarina is optional: on Labyrinth OS she is installed from Agora, the app store, never forced on anyone.

What she does today:

- **Answers by voice and text.** Super+S (or Ctrl+Alt+S) on the Labyrinth desktop: hold for one question,
  tap for a conversation; short plain answers, read aloud by her own voice.
- **Troubleshoots the computer.** She reads how it is doing (performance, network, disks, logs, failed
  services, boot, sound, kernel, temperatures) and explains it; her spoken answers come with that report
  already looked up.
- **Looks after the gaming side.** Labyrinth is a gaming machine first: she reads the gaming setup check,
  Proton versions, the installed games, and who holds the graphics card's memory (her own model included),
  and answers from guide notes about Proton, launch options, mods, controllers and anti-cheat. Her
  background jobs stay off the card while a game runs.
- **Does a few repairs when allowed a shell.** `system_fix` restarts the audio or Bluetooth for the user:
  named, fixed commands from a table, reversible, each reported with its check; nothing else can be run
  through it, and a spoken question never has it.
- **Helps with crashes as they happen.** For a program that crashed she names the function, the source
  file and line and quotes it when the source is on the computer; Labyrinth's crash helper offers to ask her.
- **Studies programs without running them.** Static analysis with REA and Ghidra in a sandbox (no network,
  no home folder); an installed program is copied read-only into the analysis folder first.
- **Remembers in a vault of notes** (an Obsidian folder): she searches it by words and by meaning and
  writes lessons into it through a safety gate.
- **Writes her own small programs ("agents")** and runs them only inside Limbo's disposable virtual
  machines; you approve an agent at a terminal, and any change voids the approval (see
  [docs/AGENT_FORGE.md](docs/AGENT_FORGE.md)).
- **Follows skills, long ones too.** A skill is a Markdown file of instructions. One up to 4 MiB is split into
  sections of at most 12 KiB, and she reads only the part she needs (`skill_section`, or `skill_search` by
  meaning or words). A skill can have its evidence looked up before she starts (`prefetch: [status]`,
  `prefetch: [security]`). Packaged skills: `desktop-fix` (find and fix what is broken on the desktop),
  `security-advisor`, `security-audit` (your own code), `static-analysis`, `ecosystem-troubleshoot`,
  `labyrinth-engineer`, `quantum-tutor`, `ios-app-builder`.
- **Reviews security with Heimdall.** `system_status security` reads what Heimdall finds open (its local
  API: packages and kernels with known holes, settings that could be safer, and files it holds in
  quarantine with the report it made by reading them). The `security-advisor` skill judges each finding
  and proposes what to do, in order, with the exact commands; you run them. On Labyrinth the daily check-up
  (`labyrinth-sarina-checkup`, in the Labyrinth_OS repo) runs it when Heimdall reports something new and
  saves her proposal in the vault (`Security/DATE.md`). She never releases a held file herself.
- **Works with the Argus App and firmware** (Flipper Zero tooling) through read-only advisory commands.

## Status and licensing

Sarina began as a fork of [ClawCode](https://github.com/HarnessLab/claw-code-agent)
(itself a port inspired by the Claude Code npm CLI). ClawCode has no license; the
current tree is licensed MPL 2.0 (see
[LICENSE](LICENSE)). The core has been rewritten from
scratch in Go (`go/`) from behaviour specs (`docs/spec/`), without reference to
the old code, and the derived Python has been deleted from the tree; see
[docs/PROVENANCE.md](docs/PROVENANCE.md) and [NOTICE.md](NOTICE.md) for the audit. Credit
to the ClawCode project and contributors for the original implementation this
grew from. Sarina is not affiliated with or endorsed by Anthropic.

## Requirements

- Go 1.27+ to build (no other build dependencies; the binary is static).
- A model server that speaks the OpenAI chat-completions API with tool calling.
  Tested with Ollama and `qwen3`. Ollama defaults to a 4096-token context, which is too small:
  Sarina needs at least 8k tokens (16k recommended). Raise it with a Modelfile line
  `PARAMETER num_ctx 16384` or `OLLAMA_CONTEXT_LENGTH=16384`, or tell Sarina the real size with `--context-window`.
- For finding notes by meaning: the small `nomic-embed-text` model in Ollama (about 270 MB, loaded only while searching).
- Optional: Python 3.10+ for the ML/voice helper (`python/sarina_helpers`).
- Linux is the reference platform; Windows is supported for development.

## Build and run

```bash
cd go
go build -o sarina ./cmd/sarina
go test ./...

export OPENAI_BASE_URL=http://127.0.0.1:11434/v1   # Ollama
export OPENAI_API_KEY=local-token
export OPENAI_MODEL=qwen3

./sarina chat "Summarise the layout of this directory." --cwd .
./sarina chat --interactive --cwd .
./sarina chat "Create NOTES.md with one line" --cwd . --allow-write
```

The same binary answers to `sarina-gui` and `sarina-service` when started under
those names (the Arch package installs them as symlinks), or run
`sarina gui` / `sarina service`.

## Settings

Read from the environment (Labyrinth keeps them in `~/.config/labyrinth/sarina.env`):

| Variable | What it does |
| --- | --- |
| `OPENAI_BASE_URL`, `OPENAI_MODEL`, `OPENAI_API_KEY` | the model server and model |
| `SARINA_TEMPERATURE` | sampling temperature; 0.6 for Qwen3 (at 0 thinking models repeat themselves until they time out) |
| `SARINA_REQUEST_MAX_TOKENS` | the most one reply may produce, so a runaway reply ends instead of timing out (6000) |
| `SARINA_VAULT` | her notes folder |
| `SARINA_SEMANTIC=on` | find notes by meaning as well as by words (needs `nomic-embed-text`) |
| `SARINA_SYSTEM=on` | the `system_status` and `crash_detail` tools (read-only), and `system_fix` (named, reversible user-level repairs; needs `--allow-shell`) |
| `SARINA_REVERSE=on` | the sandboxed `reverse_engineer` and `stage_binary` tools (needs `labyrinth-rea`) |
| `SARINA_FORGE=on` | the agent forge tools (needs Limbo and its `agent-run` profile) |
| `SARINA_LIMBO_SOCKET` | Limbo's agent socket (`/run/limbo/agent.sock`) |
| `SARINA_PRIVATE_DIRS` | folders no file tool may read, list, search or write (`:`-separated, `~` allowed) |

## Commands

`sarina --help` lists everything; `sarina <command> -h` shows flags.

| Command | What it does |
| --- | --- |
| `chat`, `resume` | run a prompt once, hold a conversation, or continue a saved session |
| `gui` | web console on 127.0.0.1:8765 |
| `service` | HTTP integration service on 127.0.0.1:8899 |
| `doctor [--json]` | read-only checkup of the whole setup (model backend, sessions, tiers, config files, Limbo, Heimdall); exit 1 on any FAIL; the model can call it as the `sarina_doctor` tool |
| `council QUESTION`, `council status [--probe]` | ask several models the same question and compare the answers; local members answer directly, online ones (Claude, Gemini, ChatGPT, ...) only after you confirm the exact text; also the `council_ask` tool when a council is configured; see `docs/ECOSYSTEM.md` section 9 |
| `tiers status` | show compute tiers and whether each is reachable |
| `skill list`, `skill run NAME [ARGS]` | skills: list them (with their source and whether the model may start them), or run one |
| `mcp list`, `agents list` | show configured MCP servers and subagent profiles |
| `memory-status`, `memory-show`, `memory-review*` | curated memory and its review queue |
| `session-search`, `memory-search` | lexical and semantic search over saved sessions |
| `limbo-profiles`, `limbo-validate` | run commands in a disposable Limbo guest |
| `system-status TOPIC`, `crash-detail PROGRAM` | how the computer is doing (performance, network, disk, logs, services, hardware, crashes, boot, audio, kernel, gaming, security), and the latest crash of one program (read-only) |
| `system-fix --list`, `system-fix ACTION` | the named repairs the `system_fix` tool can do (restart the audio, restart Bluetooth): fixed commands at user level, each with a check afterwards |
| `route QUESTION` | what to look up before answering a spoken question, decided by meaning |
| `vault-init`, `vault-search`, `vault-lint` | her notes: starting folders, search (words and meaning), broken links |
| `forge-list`, `forge-approve NAME`, `forge-remove NAME` | the agents she wrote; approving needs you at a terminal |
| `chymaera-console`, `chymaera-device-info`, `chymaera-merge-report`, `chymaera-analyze` | Argus App and Argus firmware (Flipper Zero) advisories, read-only (the command names predate the rename from Chymaera Zero) |
| `fleet-usage`, `fleet-status`, `fleet-launch` | agent-fleet usage and a confirm-first launcher |
| `desktop-audit` | audit and repair the application menu (report only unless `--apply`) |
| `voice-chat` | push-to-talk voice conversation (needs the helper) |

## Safety model

- **Nothing mutating by default.** File writes need `--allow-write`, shell
  commands need `--allow-shell`; every state-changing tool declares what it
  needs and is checked in one place. `--unsafe` is separate and explicit.
- **Destructive-command guard.** Shell commands are parsed (separators,
  subshells, substitutions, wrappers, `curl | sh`) before running. It is a
  guard-rail, not a sandbox; use Limbo for real isolation.
- **Secret-file guard** covers every file tool; child processes get a filtered
  environment.
- **Plan mode.** In plan mode only read-only tools are offered to the model at
  all. Only the operator can approve, revise or reject a plan.
- **Operator-controlled extensions.** Skills, MCP servers, hook policy and
  tiers load only from your own files (`~/.sarina/...`). A workspace can add
  skills or MCP servers only with an explicit `--trust-workspace-*` flag, and a
  workspace policy file can only tighten.
- **Her behaviour files can require the owner's signature.** Heimdall keeps a signed list of the files that
  decide how she behaves (rules, tool connections, skills, agents). In `enforce` mode she checks each one
  before using it: an unapproved skill, section, agent or tool connection is not used, and an unapproved
  `policy.json` stops her. Files under `/usr` belong to packages and are not checked. The mode is off by
  default (`go/trust`; Heimdall's docs/TRUST.md).
- **Heimdall can contain her.** While Heimdall's lock `/etc/heimdall/contained` exists she does not start
  (only `version` and `help` answer) and says how the owner releases her (Settings > Security, or
  `sudo heimdalld contain release`). The lock survives a restart (Heimdall's docs/CONTAIN.md).
- **Skills she may read.** The section tools only open skills the model may start, or one the operator
  started; an operator-only skill stays closed to her.
- **Troubleshooting is read-only first.** `sarina doctor` changes nothing and never prints keys or
  tokens. The packaged `ecosystem-troubleshoot` skill (installed in `/usr/share/sarina/skills`, a
  root-owned directory a workspace cannot shadow) walks the agent from a doctor finding to a plan the
  operator approves; it cannot touch Heimdall, Limbo or Sarina's own safety settings on its own.
- **Honesty checks in code, not only in the prompt.** Before an answer is accepted it is checked once
  against what actually happened in the task: a file it says changed that no tool changed (or a change it
  leaves out), tool work it describes when no tool ran, text it quotes that no tool output contained, or
  advice to turn off a security piece. Secret values read from files are blanked from answers.
- **File tools protect your files.** A file is never replaced unread, never erased or emptied, a binary
  file is never written as text, and private folders (`SARINA_PRIVATE_DIRS`) stay closed, also to searches.
- **Spoken questions change nothing.** By voice she has read-only tools and her vault only; she gives you
  the command instead of running it. The one tool that changes the real system, `system_fix`, is a table of
  named user-level repairs behind `--allow-shell`.
- **Her agents run only in Limbo.** No network, none of your files, no sudo; output is capped and labelled
  as untrusted evidence; you approve exact versions; there is no tool that approves anything.
- **Tool-less sessions.** `tool_allowlist: []` on the service creates a session
  with zero tools and reports `"tools": 0`; the model request carries no tools.

## Evals

`python3 evals/run_evals.py` runs about seventy checks against the real agent: understanding, honesty,
safety (injections, secrets, private folders), writing files, her vault, reverse engineering, Limbo,
troubleshooting and crash help, gaming (the setup check, Proton, mods, graphics memory, the repair tool),
spoken answers and companion talk. It writes a Markdown report and a JSON file; the
reports of past runs are in Labyrinth_OS `docs/sarina-evals/`. `tools/training/` turns her real answers into
a review sheet you tick, and exports the ticked ones for fine-tuning (nothing is trained by itself).

## Compute tiers

Sarina runs on the local model first, moves to your desktop when the local one
is unavailable, too small or under pressure, and uses Claude as a last resort.
Configure `~/.sarina/tiers.json` (or `SARINA_TIERS_FILE`):

```json
{
  "tiers": [
    {"name": "local",   "kind": "openai_compat", "base_url": "http://127.0.0.1:11434/v1", "model": "qwen3"},
    {"name": "desktop", "kind": "openai_compat", "base_url": "http://192.168.1.20:11434/v1", "model": "qwen3:32b",
     "allow_data": ["no_secrets"]},
    {"name": "claude",  "kind": "claude_cli"}
  ],
  "resource": {"max_memory_percent": 90}
}
```

Kinds: `openai_compat`, `claude` (Anthropic API, key from the environment variable
named in `api_key_env`) and `claude_cli` (hands the task to the local `claude`
executable). `allow_data` controls what may be sent to a tier: secret-shaped text
is redacted, and capture data is refused outright for `no_captures`. Each
escalation is printed and recorded in the session file. The old flags
(`--escalate-to-claude-code`, `--claude-code-cli-path`, `--escalation-trigger`,
`--disable-resource-check`) still work; see `go/notes/escalation.md`.

## Context management

Long sessions stay usable on small context windows: a running notes file per
session is reused as the compaction summary when fresh, old large tool results
are cleared by size, and summarising compaction retries on overflow. Each piece
has a `--no-...` switch. The context window is read from the backend where it
reports one.

## Service and GUI

`sarina service` implements the integration contract other repos depend on
(bearer auth, `/healthz`, `POST /v1/sessions`, `/chat`, `DELETE`); see
`docs/spec/05-gui-service.md`. Callers cannot raise shell/write/backend
permissions above what the operator allowed, and a non-loopback bind needs a
token. `sarina gui` is loopback-only with Host/Origin checks and a per-launch
token, streams events, supports cancel, and asks for approval before write or
shell tools.

## Artifex Ecosystem

| Repository | What it is | Sarina's part |
| --- | --- | --- |
| `Labyrinth_OS` | the gaming-focused OS (CachyOS base) | packages her (optional), voice, crash helper, daily check-up |
| `Yggdrasil` | the desktop (panel, Fylgja, Settings, Agora, Minos, compositor) | Super+S, Settings > Sarina's agents |
| `Chymaera_OS` | the security edition, to be renamed Niflheim OS | hosts her for security work |
| `Heimdall` | the security watcher | watches her sessions, reviews what she keeps, signs her behaviour files, can contain her; she reads its findings |
| `Limbo` | disposable virtual machines | where her validation runs and agents execute |
| `Argus_App`, `Argus_Firmware` | Flipper Zero desktop console and firmware (formerly Chymaera Zero) | read-only advisories, local control API |

The integration contract, ports and rules are in [docs/ECOSYSTEM.md](docs/ECOSYSTEM.md) and
[docs/spec/00-must-preserve.md](docs/spec/00-must-preserve.md). Hardware writes always need a human;
`fleet-launch` is CLI-only and never a model tool.

## Packaging

`packaging/arch/` is the source of truth for the Arch package: the Go binary,
the `sarina-gui` / `sarina-service` symlinks, the optional helper wheel, the
systemd unit, sysusers and env template. See its README.

## Repository map

```text
go/               the agent (module github.com/Artifex-Technologies/Sarina/go)
python/           optional ML/voice helper (sarina_helpers)
docs/spec/        behaviour specs the Go core was built from
docs/ECOSYSTEM.md cross-repo contract
docs/PROVENANCE.md licensing audit and rewrite progress
docs/AGENT_FORGE.md, docs/MEMORY_AND_TRAINING_PLAN.md   the agent forge, and memory, routing and training
evals/            capability evals (run_evals.py)
tools/training/   review sheet and export of her answers for fine-tuning
packaging/arch/   Arch package, systemd unit, env template
sarina/, tests/   legacy original Python modules (reference; superseded by go/ and python/)
```

## License

MPL 2.0. See [LICENSE](LICENSE). The licence covers the current tree, which is a
clean-room rewrite (see [NOTICE.md](NOTICE.md)).

## Disclaimer

Sarina can run shell commands and edit files when you allow it. Review what it
proposes, keep it away from systems you do not own, and do not expose the
service beyond loopback without a token.

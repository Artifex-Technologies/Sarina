# Sarina

***S***ystemic ***A***rtificial ***R***easoning & ***I***ntelligent ***N***etworked ***A***ssistant

A local-model security agent for the Chymaera ecosystem. It talks to any
OpenAI-compatible model server (Ollama, vLLM, LiteLLM, llama.cpp), calls tools,
keeps sessions on disk, and can climb to a bigger machine or to Claude when the
local model is not enough. The core is one static Go binary.

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

## Commands

`sarina --help` lists everything; `sarina <command> -h` shows flags.

| Command | What it does |
| --- | --- |
| `chat`, `resume` | run a prompt once, hold a conversation, or continue a saved session |
| `gui` | web console on 127.0.0.1:8765 |
| `service` | HTTP integration service on 127.0.0.1:8899 |
| `doctor [--json]` | read-only checkup of the whole setup (model backend, sessions, tiers, config files, Limbo, Sentry); exit 1 on any FAIL; the model can call it as the `sarina_doctor` tool |
| `tiers status` | show compute tiers and whether each is reachable |
| `skill list`, `skill run` | named reusable prompts |
| `mcp list`, `agents list` | show configured MCP servers and subagent profiles |
| `memory-status`, `memory-show`, `memory-review*` | curated memory and its review queue |
| `session-search`, `memory-search` | lexical and semantic search over saved sessions |
| `limbo-profiles`, `limbo-validate` | run commands in a disposable Limbo guest |
| `chymaera-console`, `chymaera-device-info`, `chymaera-merge-report`, `chymaera-analyze` | Chymaera Zero App and firmware advisories (read-only) |
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
- **Troubleshooting is read-only first.** `sarina doctor` changes nothing and never prints keys or
  tokens. The packaged `ecosystem-troubleshoot` skill (installed in `/usr/share/sarina/skills`, a
  root-owned directory a workspace cannot shadow) walks the agent from a doctor finding to a plan the
  operator approves; it cannot touch Sentry, Limbo or Sarina's own safety settings on its own.
- **Tool-less sessions.** `tool_allowlist: []` on the service creates a session
  with zero tools and reports `"tools": 0`; the model request carries no tools.

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

## Ecosystem

Sarina is one of five repos: `Chymaera_Linux` (hosts it), `Chymaera_Zero_App`,
`Chymaera_Zero_Firmware`, `Chymaera_Sentry` (watches it). The integration
contract, ports and rules are in [docs/ECOSYSTEM.md](docs/ECOSYSTEM.md) and
[docs/spec/00-must-preserve.md](docs/spec/00-must-preserve.md). Hardware writes
always need a human; `fleet-launch` is CLI-only and never a model tool.

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

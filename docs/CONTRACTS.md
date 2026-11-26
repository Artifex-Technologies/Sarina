# Sarina's contracts with the other repositories

What other projects rely on from Sarina, one section each: what it is, who uses it, and where it is defined in the code.
The full wording of each, with its history and the rules on both sides, is in [ECOSYSTEM.md](ECOSYSTEM.md) (the section is
named in each entry). A change to any of these is a cross-repository change: update this file and ECOSYSTEM.md in the same
commit, flag the commit `CONTRACT:`, and tell the other side.

## The package's inputs (`sarina-git`)

- **What:** Labyrinth_OS's recipe builds this repository as it is. `rust/`, `packaging/{arch,helpers,voice,voiced,skills}`,
  `README.md` and `LICENSE` stay where they are (and `go/` while the rollback copy is built).
  The binary answers to `sarina`, `sarina-gui` and `sarina-service`; the package also installs `sarina-tone`, `sarina-voice`,
  `sarina-voiced` and `/usr/lib/sarina/sarina-helpers`. A new run-time dependency must exist in Arch's repositories.
- **Who uses it:** Labyrinth_OS (`packages/pkgbuilds/sarina-git`, both copies), LykanOS (installs the package from
  Labyrinth's repository).
- **Where:** `packaging/arch/PKGBUILD` (the reference recipe), `rust/sarina/src/bin/sarina-rs.rs` (the names it answers to).
  ECOSYSTEM.md section 1.

## Session files

- **What:** the agent's sessions as JSON in the shared folder `/var/lib/sarina/sessions/*.json` (the service writes them),
  messages as objects with a `role`; the API key is never written. `sarina doctor` checks the shape.
- **Who uses it:** Heimdall polls them (`agent_session_dir` in its config), and reads the file-writing tool calls'
  arguments by name (`write_file`, `vault_write`, `edit_file`, `forge_write`) for its watermark scan.
- **Where:** `rust/sarina/src/session/`. ECOSYSTEM.md section 4a.

## The integration service (port 8899)

- **What:** `/v1/sessions` and `/chat`. A session opened with `tool_allowlist: []` has no tools, and the response's `tools`
  count says how many it has; a session with `tool_allowlist: ["reverse_engineer"]` analyses a held file named as the
  sandbox sees it, `/quarantine/<sha256>.sample`, and its `similar` command searches the similarity library. Keep the
  allowlist's exact names, the `tools` count, `reverse_engineer`'s name, the `/quarantine` reading and `similar`'s name.
- **Who uses it:** Heimdall (advisories on a quarantined Limbo guest; `heimdalld immunity analyze --sarina`).
- **Where:** `rust/sarina/src/service.rs`, `rust/sarina/src/reverse.rs`. ECOSYSTEM.md sections 4c and 4d.

## Ports

- **What:** 8765 (the operator GUI) and 8899 (the integration service), loopback only.
- **Who uses it:** every repository that picks a port (the registry avoids clashes); Heimdall (8899).
- **Where:** `rust/sarina/src/gui/`, `rust/sarina/src/service.rs`. ECOSYSTEM.md section 6.

## The agent fleet (`fleet-usage`, `fleet-status --json`, `fleet-launch`)

- **What:** `sarina fleet-usage` writes `sarina.json` into the agents panel's usage folder; `sarina fleet-status --json`
  prints `{"agents":[{"id","name","ready","stale","updatedAt","todayTokens","weekTokens","totalSessions","limits":[{"label",
  "percentUsed"(null when unknown),"resetsAt"}],"statusText"}],"self":{"id","name":"Sarina","running","lastActive"}}`, with
  `"mostHeadroom"` (an agent id, or "") added by `--suggest`; the other agents' records are untrusted and printed cleaned
  (ASCII only, other characters escaped); `fleet-launch` runs only after a person confirms and is never a tool the model can call (a test enforces it).
- **Who uses it:** Yggdrasil's Agents item (`packages/agent-usage`, `yggdrasil-agent-usage --agents`), LykanOS (its
  contract item 7, Omarchy's agents panel).
- **Where:** `rust/sarina/src/fleet/` (`status.rs`, `status_json`). ECOSYSTEM.md section 1, "Agent fleet".

## MCP server definitions (`~/.sarina/mcp.json`, `SARINA_MCP_FILE`)

- **What:** the tool servers Sarina may start, as JSON files of at most 256 KiB: `~/.sarina/mcp.json`, then each file in
  `SARINA_MCP_FILE` (a colon-separated list); a workspace's `.sarina-mcp.json`, `.mcp.json` or `mcp.json` only with
  `--trust-workspace-mcp`. A file holds `{"mcpServers":{"NAME":{...}}}` or `{"servers":[{"name":"NAME",...}]}`; each server
  has `command` (required), `args`, `env`, `cwd` (relative to the file's folder; `~` allowed), `description`, `read_only`,
  `read_only_tools` or `tools: {"TOOL":{"read_only":true}}`, `timeout_seconds`; `transport` may only be `stdio`. The first
  file to name a server wins; every tool is treated as changing things unless marked read-only, and its output is untrusted
  data. `sarina mcp list` shows what was loaded and every problem.
- **Who uses it:** Icarus (`icarus mcp`, opt-in through a file named in `SARINA_MCP_FILE`); Heimdall's trust list watches
  `~/.sarina/mcp.json` (`heimdalld trust`).
- **Where:** `rust/sarina/src/mcp/config.rs` (`load`, `parse_server`), `mcp/manager.rs`.

## Menu categorisation (`sarina desktop-audit`)

- **What:** application-menu categories for installed programs, as a report by default and as desktop-file overrides when
  asked.
- **Who uses it:** LykanOS (its contract item 2).
- **Where:** `rust/sarina/src/desktop.rs`. ECOSYSTEM.md section 1, "Menu categorisation".

## The voice service socket (`sarina-voiced`)

- **What:** a unix socket, `$XDG_RUNTIME_DIR/labyrinth-sarina/voiced.sock` (or `LABYRINTH_VOICED_SOCKET`), one JSON request
  line per connection: `{"op":"speak","text":...}` answers one line per sentence (`wav`, `text`, pauses) and then
  `{"done":true}`; `ping`, `warm` (loads in the background, answers at once) and `unload`. The `LABYRINTH_VOICE_*` variables
  (idle time, dtype, reference voice, venv, pipeline) keep their names and meaning.
- **Who uses it:** Labyrinth_OS's `labyrinth-voiced.service` starts it; Yggdrasil's panel sends `warm` when the talk key goes
  down; `sarina-voice` (`labyrinth-sarina-speak-voiced`) speaks through it.
- **Where:** `packaging/voiced/src/server.rs`, `packaging/voiced/src/main.rs`; `packaging/voiced/README.md`.

## The spoken-question programs (`sarina-voice`)

- **What:** one binary under the old names `labyrinth-ask-sarina`, `labyrinth-sarina-checkup` and
  `labyrinth-sarina-speak-voiced` (it picks the program by the name it is started as), with the same arguments, environment
  and output as the Python originals.
- **Who uses it:** Labyrinth_OS's image (its `/usr/local/bin` names link to `/usr/bin/sarina-voice`), Yggdrasil's talk key
  (through the panel and `labyrinth-sarina-ask`).
- **Where:** `packaging/voice/src/`; `docs/VOICE_RUST_PORT.md`.

## The settings file (`~/.config/labyrinth/sarina.env`)

- **What:** `KEY=value` lines Sarina and her voice read at start: the model and backend (`OPENAI_*`), the optional tool sets
  (`SARINA_CRYPTO`, `SARINA_VAULT`, `SARINA_FORGE`, ...), the voice's switches (`LABYRINTH_VOICE_*`), the owner's name and
  note, and the applied access tier (`SARINA_ACCESS_TIER`, which only the owner sets, and which Heimdall's grant gate caps).
- **Who uses it:** Labyrinth_OS writes the defaults (its ecosystem setup) and Settings > Assistant's switches
  (`labyrinth-sarina-setting`); the evals read it to mirror the owner's setup.
- **Where:** `rust/sarina/src/cli/` (the environment the commands read), `packaging/voice/src/ask.rs`; `docs/ACCESS_LADDER.md`.

## The decompile profile (`limbo-decompile`)

- **What:** a human-written Limbo profile and its guest image for static analysis of an untrusted program (Ghidra, capa,
  YARA-X's `yr`), with no network.
- **Who uses it:** Limbo hosts it; Heimdall's immunity work reads its "strings worth a rule" section.
- **Where:** `packaging/limbo/decompile/`. ECOSYSTEM.md section 8.

## What Sarina uses from others

| Contract | Provided by | Where it is defined |
| --- | --- | --- |
| The console API (NDJSON over loopback TCP, port 44700) | Argus_App | its `docs/SARINA_API.md`; Sarina's client: `rust/sarina/src/chymaera/console.rs` |
| Firmware version strings and advisories | Argus_Firmware | ECOSYSTEM.md section 2; `rust/sarina/src/chymaera/firmware.rs` |
| The agent socket (`SARINA_LIMBO_SOCKET`), profiles, validation runs | Limbo | its `docs/QUARANTINE.md`; `rust/sarina/src/limbo.rs` |
| `POST /v1/review` (`HEIMDALL_REVIEW_URL`), the grant gate, `POST /v1/rules/drafts`, the containment lock, the signed list of behaviour files | Heimdall | its docs; `rust/sarina/src/heimdall.rs`, `guard.rs`, `tiers/`, `reverse.rs`, `trust.rs` |
| The `sarina-git` recipe and the voice unit | Labyrinth_OS | `packages/pkgbuilds/sarina-git`, `labyrinth-voiced.service` |

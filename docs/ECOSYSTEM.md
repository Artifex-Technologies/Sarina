# Chymaera Ecosystem — integration contract

Sarina is one of the Artifex repos (the table below). It is the one that touches
the others, so this file records how they fit together **from Sarina's side**.

> **Renamed 2026-10-10.** `Chymaera_Sentry` is now `Heimdall`, `Chymaera_Linux` is
> `Chymaera_OS`, `Chymaera_Zero_App` is `Argus_App`, `Chymaera_Zero_Firmware` is
> `Argus_Firmware`. Older Coordination Log entries use the old names on purpose.

Status keys: **live** (implemented and verified), **agreed** (both sides accepted
the shape, not yet built), **proposed** (awaiting the owning repo's answer),
**open** (nobody has proposed anything).

| Repo | Role | Owns |
| --- | --- | --- |
| [`Chymaera_OS`](https://github.com/Artifex-Technologies/Chymaera_OS) | Arch-based security distro; the host | ISO profile, package set, the packaging contract |
| [`Argus_Firmware`](https://github.com/Artifex-Technologies/Argus_Firmware) | Flipper Zero firmware fork | RPC wire format (`assets/protobuf/`), the BLE budget |
| [`Argus_App`](https://github.com/Artifex-Technologies/Argus_App) | qFlipper fork; desktop operator console | **The Sarina control API**, datastore, exporters |
| [`Heimdall`](https://github.com/Artifex-Technologies/Heimdall) | Go security agent watching Sarina (formerly Chymaera_Sentry) | Its own detection engine; reads Sarina's session files, read-only |
| `Labyrinth_OS` | CachyOS-based daily-driver distro for one machine (i9-9900K, 64 GB, RTX 2080 Ti with 11 GB VRAM) meant to replace Windows; the owner's current focus and the second host for Sarina | Its own image and install; `docs/DRIVER_LAB.md` is the Sarina + Limbo driver workflow. Its own `docs/STATUS.md` (2026-10-10) still lists the ISO build pieces as missing |
| `Daedalus_CAD` | CAD application (Rust) | Not a Sarina contract; listed so the repo set is complete |
| `Limbo` | Sandbox that runs guest VMs (Linux/Windows) as seamless windows; quarantine and validation runs | The agent socket Sarina uses (section 8) |
| `Sarina` (this repo) | Local-model agent shipped in the distro | The client/advisory surfaces below |

> **Each contract is owned by exactly one repo, and it is usually not this one.**
> Where a sibling has published a spec, that spec wins and Sarina conforms. This
> file records what Sarina implements against, not what Sarina wishes were true.

---

## 1. Sarina ↔ Chymaera_OS — **live**

Sarina ships *in* the distro, as a package built from a submodule.

**Two hosts, one Sarina.** Labyrinth_OS vendors the same `sarina-git` PKGBUILD from
Chymaera_OS, so the packaging contract below serves both, and nothing in Sarina may
assume one of them. The owner is focused on Labyrinth (replacing Windows on one
machine), so what Sarina does there matters most right now: helping bring the
install to a good state, troubleshooting drivers and updates by validating in Limbo
first (`Labyrinth_OS/docs/DRIVER_LAB.md`), and not fighting games for the 11 GB of
VRAM (the council runs local members one at a time by default for the same reason).
Labyrinth's `driver-test` Limbo profile and its "Sarina hook" are not built yet.

### The packaging contract (owned by Chymaera_OS)

`packages/pkgbuilds/sarina-git/PKGBUILD` builds this repo directly and **breaks
if any of these move**. Verified present as of this commit:

| Path here | Used for |
| --- | --- |
| `pyproject.toml` | `python -m build --wheel --no-isolation` |
| `packaging/arch/sarina-service.service` | → `/usr/lib/systemd/system/` |
| `packaging/arch/sarina.sysusers` | → `/usr/lib/sysusers.d/sarina.conf` |
| `packaging/arch/service.env.example` | → `/etc/sarina/` |
| `README.md` | → `/usr/share/doc/sarina-git/` |

**Entry points the ISO expects on `PATH`** — all three verified resolvable:

| Console script | Target | Status |
| --- | --- | --- |
| `sarina` | `sarina.main:main` | present |
| `sarina-gui` | `sarina.gui.__main__:main` | present |
| `sarina-service` | `sarina.service.__main__:main` | present |

**Runtime dependencies must stay in sync and must exist in Arch's repos.**
This repo declares `fastapi`, `uvicorn`, `pydantic`; the PKGBUILD declares
`python`, `python-fastapi`, `uvicorn`, `python-pydantic`. Note Arch's uvicorn
package has **no `python-` prefix** — that mismatch has already broken the ISO
build once. A PyPI-only dependency needs its own PKGBUILD in the distro first,
so **adding a dependency here is a cross-repo change**: say so in the commit
message with `CONTRACT:` so the distro side can grep for it.

### What the distro needs from Sarina — **open, not built**

The distro ships a **small hand-picked tool core**, not `blackarch-*` category
groups. The groups were tried and abandoned (thousands of packages, unresolvable
`lib32-*` deps, provider menus, an ISO too large for CI). Everything past that
core is explicitly Sarina's job:

1. **Install-time tool selection** — ask what the machine is for and install
   accordingly, instead of a guess baked into the ISO.
2. ~~**Categorisation**~~ — **built, see below.**
3. **Data analysis** — making sense of scan and capture output in place. This is
   the reason the agent is in the OS at all.
4. **`blackarch_compat` against a real Docker daemon** — it is unit-tested only
   and has *never* run against real Docker. It is the safety check behind (1);
   until it has actually run, treat any category it reports as unvalidated.

5. **Crash diagnosis** and 6. **Proton VPN lifecycle** — added by the distro,
   **open, not built** (see its `CLAUDE.md`).
7. **Agent manager** — **built**, see "Agent fleet" below.

> **Correction to an earlier version of this file:** it described
> `blackarch_compat/` as validating the distro's category selection. The distro
> no longer installs category groups at all, so that harness now serves (1)
> above — vetting categories *before* the agent proposes them — not the shipped
> package list.

Do not add tools to the distro's `packages.x86_64` to work around the gap; that
re-creates the problem the categories caused.

### Menu categorisation — **live** (`sarina desktop-audit`)

`sarina/desktop/` answers ask (2). Read-only by default: it reports, and writes
only with `--apply`. **General-purpose, not security-specific** — the problem is
any package shipping a bare binary; BlackArch is where it is worst.

Two jobs, matching the distro's own split:

- **Repair** — an entry exists but carries no registered Main Category, so it
  has no menu location. `desktop-file-validate` states this directly for a real
  `burpsuite.desktop`: *"does not contain a registered main category;
  application might only show up in a catch-all section"*. The 52
  `blackarch-*` groups and their `X-BlackArch-*` desktop tokens are mapped to
  spec-valid categories.
- **Create** — the package ships no `.desktop` file at all.

**Three signals, cheapest first**, so a run with no model is still useful:
pacman *group* → *description* keyword rules → optional semantic match against
prototype vectors (`--model-dir`). Measured against Arch's real repos, the
**description is usually the only signal that exists**: all 11 security tools
sampled from the distro's core (`nmap`, `sqlmap`, `hydra`, `metasploit`, …) come
from `extra`, ship **zero** `.desktop` files, and have `Groups: None`. The group
path only fires for packages actually installed from `[blackarch]`.

**Two filters do most of the work**, and removing them fills the launcher with
noise:

| Filter | Why |
| --- | --- |
| `NoDisplay=true` / `Hidden=true` entries are never touched | 93 of 108 entries on a minimal Arch install are hidden service files |
| only explicitly-installed, application-shaped packages | 35 of 725 installed packages are explicit; `qt6-base` otherwise scores well enough on description alone to be proposed |

Reversible by construction: writes go to the **user** application directory (no
root, survives upgrades), every file carries `X-Sarina-Generated`, and
`--remove-generated` deletes exactly those — verified to leave hand-written
files alone. A repair rewrites one line of a *copy* of the original, preserving
`MimeType`, desktop actions, and translated names, because a user-level file
replaces the system one outright rather than merging.

**Open question for the distro:** the emitted `Security` token is what a
dedicated Plasma Security menu would match on, but freedesktop says `Security`
should be paired with `Settings` or `System`, so `Network;Security;` draws an
informational hint (validation still exits 0). Pairing every tool with `System`
would silence it at the cost of listing every security tool in the System menu
too. **Left as-is pending the distro's menu design** — if you want a dedicated
Security menu, you need an XDG `.menu` file plus a `.directory`, which is Plasma
configuration and belongs on your side. Also note the ISO's MacTahoe icon theme:
generated entries currently set no `Icon`, so they take a generic fallback.

---

### Agent fleet: Omarchy's agents panel — **live** (`sarina fleet-*`)

Answers `Chymaera_OS` contract item 7. The distro adopts
[Omarchy](https://github.com/omacom/omarchy)'s desktop, whose bar has one panel
showing every AI agent on the machine. The panel is a pure display: each agent
is one JSON record in `~/.local/state/omarchy/agents/usage/`, and **any file
that lands there becomes a tab**. Three steps, built in increasing order of
power and risk:

1. **`sarina fleet-usage`** — prints (or, with `--write`, writes)
   `sarina.json` from Sarina's saved sessions, in the exact shape Omarchy's own
   Codex collector prints (verified against `bin/omarchy-agent-usage-codex` at
   Omarchy v4.0.4). Omarchy's panel regenerates records by running every
   `omarchy-agent-usage-*` script in `$OMARCHY_PATH/bin` and writes each script's
   **stdout** to `<agent>.json` itself, so the distro ships a one-line wrapper,
   `omarchy-agent-usage-sarina`, that runs `sarina fleet-usage` (no `--write`;
   that flag is for standalone use, e.g. a timer). Session totals are cumulative, so a session's
   tokens are attributed to the local day its file was last written.
2. **`sarina fleet-status [--suggest]`** — **read-only.** Reads every record in
   that directory (Claude Code, Codex, ...) into one view of usage and limits,
   and optionally names the agent with the most headroom. **The records are
   untrusted data written by other programs:** control and bidi characters are
   stripped, text is truncated, ids must match `[a-z0-9][a-z0-9_-]{0,31}`,
   numbers are clamped, files over 1 MiB are skipped. Unknown headroom is never
   ranked as unlimited.
3. **`sarina fleet-launch <agent> [prompt]`** — **proposal-first.** Omarchy
   starts agents in their auto-approve modes (`claude --permission-mode auto`,
   `gemini --yolo`, ...). This launcher has **no way to add such a flag**; each
   agent asks before acting. It prints the proposal, needs a human's `y` (or
   `--yes`), refuses if the prompt changed after the proposal, and appends every
   attempt to `~/.local/state/sarina/agent-launch.jsonl` for Heimdall to
   read — prompt as a SHA-256, a length and a 60-character preview, never in
   full. **It is a CLI command only and is deliberately not a model tool**
   (`tests/test_chymaera_agent_launch.py` checks the tool layer never imports
   it), so a prompt-injected model cannot trigger it. `crush` and `openclaw` are
   unsupported and say why.

Not yet verified against a running Omarchy shell: whether the panel draws
Sarina's tab correctly. The record matches the fields the panel's QML reads,
but that was read from source, not observed.

## 2. Sarina ↔ Argus_Firmware — **live (read-only)**

The firmware project set the boundary: *"anything that writes to hardware should
stay behind explicit human approval for now."* Sarina honours it.

**Implemented** in `sarina/chymaera/firmware.py`:

| Command | What it does |
| --- | --- |
| `sarina chymaera-device-info [--file F] [--expected-commit SHA]` | Parses captured `device_info`; warns on stock firmware, invalid enclave, dead radio, a version git could not produce, or a commit that does not match the tree. Notes a development build without failing. Exit 1 on any warning. |
| `sarina chymaera-merge-report --cwd <firmware> [--upstream-ref upstream/dev]` | Reports what merging upstream would touch, **without merging**, using the existence test below. |

### Merge risk — decided by upstream existence, not path

The firmware repo corrected an earlier version of this classifier: risk is not
which directory a path sits in, it is **whether the path exists upstream**. A
file absent from `upstream/dev` cannot produce a merge conflict no matter where
it lives — module-style code (`menu_wii.c`) has to sit inside an upstream
directory for its `#include` to resolve, but is still a new, non-conflicting
file. `merge_impact()` now tests existence directly
(`git ls-tree -r --name-only <upstream_ref>`), not a path prefix:

| Tier | Test | Meaning |
| --- | --- | --- |
| `high` | exists upstream **and** upstream also changed it | will conflict |
| `medium` | exists upstream, upstream did not move it | conflicts only if upstream moves it later |
| `low` | does not exist upstream | additive, cannot conflict |

A path-prefix `area` label (`wire-format`, `core`, `build`, `external-app`,
`docs`) is still attached to each result for context, but it no longer drives
the tier. If the count of `high`+`medium` files exceeds
`REBASE_POLICY_BUDGET` (12 — the firmware project's own number, "roughly a
dozen"), the report prints a policy alarm: that many non-additive first-party
files means the "add directories, don't edit upstream files" policy is being
violated, which matters more than any single file's risk.

The firmware repo confirmed `applications_user/` as the common case (existing
classifier assumption unaffected there) and named the current complete
medium/high set for their tree: `scripts/version.py`,
`applications/services/loader/loader_menu.c`, `fbt_options.py`.

### The BLE budget — a hard constraint on anything Sarina ingests

Streaming targets **BLE to a laptop running Chymaera OS**, not USB. The
firmware repo owns these numbers:

| Constant | Value |
| --- | --- |
| Bytes per GATT notification | 243 |
| Max per TX call | 486 |
| Connection interval | 7.5 – 45 ms |

That is a ceiling of roughly **5–30 KB/s** — about an order of magnitude *worse*
than USB CDC — and it is to be **measured, not designed against**.

**What this forbids Sarina from building:** any ingest path that assumes raw IQ,
full 802.11 capture, or a live spectrum waterfall. What fits: decoded events,
RSSI sample streams, BLE advertisement records, scan tables, periodic telemetry.
Reduction happens on-device; where fidelity matters, the device captures to SD
and the file is pulled over RPC storage afterwards.

### Transport tiering — Sarina must check the tier before invoking

The firmware tiers every feature by transport rather than pretending the two are
equivalent:

| Tier | Transports | Covers |
| --- | --- | --- |
| **USB-only** | USB CDC | bulk capture retrieval, SD/file sync, firmware update, high-rate or raw sampling |
| **Either** | USB CDC or BLE | control/commands, device info, decoded events, RSSI streams, scan tables, telemetry |

Rule of thumb from that repo: *if it would take more than a few seconds at
~10 KB/s, it is USB-only.* A USB-only feature **refuses a BLE session outright**
with a presentable error rather than degrading silently. Sarina should surface
that refusal as-is and, where it can, learn a feature's tier *before* invoking
it rather than discovering it by failure.

Connection shape, for any headless setup: the Flipper is a BLE **peripheral**
running a serial service; a Linux client connects over BlueZ and speaks the same
protobuf RPC used over USB. Pairing uses `GapPairingPinCodeShow` with bonding —
**a PIN appears on the device screen and must be confirmed by a human**, which
rules out fully unattended pairing.

### Firmware version reporting

As of 2026-10-24, `scripts/version.py` composes `firmware_version` as
`ARG-<upstream>r<ours>` — e.g. `ARG-1.4.3r1` — or `ARG-1.4.3-dev` when no
`ARG-1.4.3r*` tag exists yet. Builds after a tag keep the tag's name: the `+N`
commit-count suffix was dropped by maintainer choice (firmware `ebe7ab9e`), so
the separate, exact `firmware_commit` field is what tells two builds apart.
`firmware_origin_fork` reads `Argus`. `chymaera-device-info`
(`go/chymaera/firmware/firmware.go`) parses this format, with `+N` still
accepted as optional, and notes the tracked upstream release and dev-build
status **without failing the check**. Older devices may still report the
interim `1.4.3-CHY-0.1.0[+N]` format (fork `Chymaera`) or the pre-2026-09-12
`git describe` form. `unknown` is still a warning — it means git failed outright.

Argus_App uses the same scheme for its own version (`ARG-1.3.3r1`, after
qFlipper 1.3.3). Since 2026-10-24 it warns in its Update/Install confirmation
when `firmware_origin_fork` is not `Official`, because those channels install
official firmware over the fork.

`1.4.3` (`GitVersion.UPSTREAM_VERSION`) is a **hand-maintained constant** in
the firmware repo, not derived from git — release tags aren't ancestors of
`dev`. It can go stale after an upstream merge; the firmware repo flagged this
as a natural thing for `chymaera-merge-report` to notice, not yet implemented.

### Hard boundaries Sarina will not cross

- No flashing, no `fbt flash*`, no `update.fuf` option-byte edits.
- No edits to `assets/protobuf/` — that is the firmware↔app wire contract.
- No unattended firmware-core patches. Sarina may *recommend* an external app
  over a core patch; it does not apply core patches itself.

---

## 3. Sarina ↔ Argus_App — **live (client implemented)**

**The App owns this contract.** Its spec is
[`docs/SARINA_API.md`](https://github.com/Artifex-Technologies/Argus_App/blob/main/docs/SARINA_API.md),
implemented in `backend/chymaera/sarina/sarinaserver.cpp`. Sarina implements the
client half in `sarina/chymaera/console.py` and does not extend the protocol.

| Aspect | Value |
| --- | --- |
| Transport | Newline-delimited JSON over TCP, loopback only; non-loopback peers dropped |
| Port | **44700** |
| Default state | **Off.** Started from the Chymaera Console UI, `Chymaera.startServer(port, token)`, or headlessly (below) |
| Auth | Optional `"token"` field on every request; mismatch → `{"ok":false,"error":"unauthorized"}` |
| Request | `{"cmd": "status", "id": 7, "token": "optional"}` |
| Response | `{"ok": true, "id": 7, ...}`; failures carry `"error"` |

Commands Sarina implements, the App's v1.0 set: `ping`, `help`,
`status`, `log.tail`, `log.add`, `session.start`, `session.end`, `capture.list`,
`capture.import`. API v1.1 (2026-09-12) added `device.status` and
`capture.pull`, both read-only; Sarina does not call them yet.

```bash
sarina chymaera-console [--host H] [--port 44700] [--token T] [--captures N]
```

The token may come from `CHYMAERA_CONSOLE_TOKEN` instead of the command line.

**Starting the server without a GUI.** The App ships a headless CLI, which is
how this works on a server, over SSH, or under WSL where Qt Quick may not
render — and therefore the path that matters most on Chymaera OS:

```bash
qFlipper-cli chymaera serve 44700 <token>   # starts the API, streams the event log
qFlipper-cli chymaera selftest              # end-to-end check, includes an API ping
```

`selftest` is the fastest way to tell whether a Sarina-side failure is Sarina's
fault or the app's.

### Capture analysis — Sarina reading, not extending

`sarina chymaera-analyze` (`sarina/chymaera/analysis.py`) pulls records via
`capture.list()` and works only with the `meta` JSON field the App already
attaches to each one — it does not parse `.nfc`/`.sub` files itself, and never
will; that parsing is the App's job (§4's boundary). Statistical analysis
(duplicate NFC UIDs, rare Sub-GHz frequencies) needs only the fields the App
already documents in "Capture record shape." An optional `--model-dir` adds
semantic grouping via `sarina.ml` (a local embedding model, see the README) —
this is Sarina-side only and adds nothing to the App's contract.

**Rules Sarina follows here:**

- Sarina reads the datastore **only through this API**, never by opening the
  SQLite file at `status.datastorePath` directly — schema changes stay behind
  the contract and nothing contends for the file lock.
- New commands get proposed on the App's board **before** any client code.
- Destructive device operations are deliberately not exposed, and Sarina does
  not work around that.

> **Superseded:** an earlier version of this file proposed a different shape for
> this API (HTTP, port 8900, bearer header). The App had already built and
> documented the protocol above. Their spec is authoritative; the proposal is
> withdrawn and this section describes what is actually implemented.

**One point still worth the App instance's judgement:** if commands that
transmit, flash, or mutate device state are ever added, they should require an
approval the operator granted in the app's own UI. An assistant should not be
able to make a Flipper transmit because a model decided it was a good idea.
Read paths need no such gate. Recorded as a request, not a demand — that repo
owns the call.

---

## 4. Sarina ↔ Heimdall — two contracts, opposite directions

### 4a. Heimdall reads Sarina's session files — **live (reader-owned)**

Heimdall is a separate Go process with no import dependency on Sarina. It
polls the agent session files the Go core writes to a workspace's
`.port_sessions/agent/*.json` (`messages` as objects with a `role`; the API key
is never written). Heimdall's `agentsource` is the reader; `sarina doctor`'s
`session_contract` check reports files that do not have the shape it expects.

The shape is Sarina's persistence format, kept stable on purpose because
something outside the repo now reads it. A change to it needs a
Coordination Log entry and a matching change on Heimdall's board.
(The legacy Python `session_store.py` layout, a flat `messages` list directly
under `.port_sessions/`, is no longer what Heimdall reads.)

Heimdall never writes into `.port_sessions/` and never opens a Sarina
process's sockets or files other than this one directory.

### 4b. Sarina consults Heimdall's governance API — **live, opt-in**

The reverse direction, added 2026-09-26: `go/heimdall` calls Heimdall's
`POST /v1/review` over loopback HTTP, opt-in via the `HEIMDALL_REVIEW_URL`
environment variable. The shared scan gate in `go/guard` (used by curated-memory
writes) consults it *after* its own in-process checks pass, and folds an actual
`"deny"` response into the same blocked result those checks already produce.
No call site outside `go/guard` consults it.

**Renamed 2026-10-10:** the variable was `SENTRY_REVIEW_URL`. The old name is
still read when `HEIMDALL_REVIEW_URL` is unset, so existing deployments keep
working; new setups should use the new name. (The legacy Python
`sarina/sentry_governance.py` still reads only the old name.)

**Fails open on Heimdall's absence, fails closed on an actual verdict.**
Unset `HEIMDALL_REVIEW_URL`, or an unreachable/erroring Heimdall, and
the consultation returns nothing — the gate falls back to its own
in-process result exactly as before this integration existed. Heimdall is
optional infrastructure, not a hard dependency; but a real `"deny"`
response blocks the write, same as an in-process match would.

**Why this isn't pointless redundancy** even though Heimdall currently runs
the identical three pattern lists Sarina's own in-process check does: the
point is that Heimdall's copy runs in a separate OS process this repo cannot
reach into. A compromised or sufficiently manipulated agent session could
in principle disable or bypass its own in-process
checks from within its own process; it cannot touch Heimdall's.

Marked **live** rather than proposed because both halves are built,
tested, and verified working together over a real loopback connection —
not a claim that the shape will never move. A breaking change to the
`/v1/review` request/response JSON needs updating this section,
`go/heimdall`, and Heimdall's own
`internal/governance`/`internal/api/review.go`, together, in one commit.

Full detail in both repos' 2026-09-26 Coordination Log entries — see
[`Heimdall/CLAUDE.md`](https://github.com/Artifex-Technologies/Heimdall/blob/main/CLAUDE.md)
§5 for Heimdall's side.

### 4c. Heimdall asks Sarina for an advisory — **live, opt-in**

Added 2026-10-04 at the owner's request. When Heimdall quarantines a Limbo guest
(section 8) it may ask Sarina for an opinion for the human who must decide. It
uses Sarina's service API (`sarina-service`, `POST /v1/sessions` then
`/chat`) with `tool_allowlist: []`, meaning the session has **no tools at all**;
bearer token if configured, loopback only. The evidence is sent framed as
untrusted data (it comes from a guest that may be hostile).

**Why an allowlist and not just `allow_shell=false`:** `AgentPermissions` only
*deny at call time*; the model is still offered every tool. Heimdall's first
version relied on those flags, and the chain test showed the advisory model was
offered ~70 tools (`web_fetch`, `memory_add`, `config_set`, `mcp_call_tool` ...),
most of which those flags do not gate. `tool_allowlist` (additive field on
`POST /v1/sessions`) removes tools from the session entirely; the response's
`tools` field reports how many it really has, and Heimdall refuses to send evidence
unless that is 0, since an older service silently ignores unknown fields.

**Sarina's answer is an advisory and nothing else.** Heimdall stores it on the
quarantine record, where Limbo bounds and cleans it and the human sees it
labelled as untrusted. It cannot change a record's status or release a guest;
only a human verdict can. Assume the advisory can be wrong or manipulated; the
design makes that survivable rather than relying on Sarina resisting it.

This is Heimdall initiating contact with Sarina, which Heimdall's contract had not
allowed before; recorded in both repos. It needed one additive Sarina change,
the `tool_allowlist` field and `tools` count described above.

---

## 5. Firmware ↔ App

Not Sarina's contract, recorded so the picture is complete: the interface is
`assets/protobuf/` in the firmware repo, and new capability goes through
`DataExchangeRequest` in `application.proto` rather than new top-level RPC
messages. RPC is transport-agnostic — the same protobuf session runs over USB
CDC or the BLE serial service. Sarina treats that boundary as read-only.

---

## 6. Shared conventions

### Port registry

| Port | Service | Repo | Status |
| --- | --- | --- | --- |
| 8765 | Sarina web GUI | Sarina | live |
| 8899 | Sarina integration service | Sarina | live |
| 44700 | Sarina control API | Argus_App | live |
| 8930 | Heimdall status API (loopback, read-only) | Heimdall | live |

Claim a port by adding it here in the same commit that binds it.

### Rules that apply in every repo

- **Loopback only.** No ecosystem service binds a routable interface by default.
- **Never commit secrets.** No tokens, keys, capture data, or client engagement
  data. Real capture output belongs in gitignored paths.
- **Hardware writes need a human.** Flashing, transmitting, and option-byte
  changes stay behind explicit operator approval, in every repo.
- **Stock compatibility is a hard requirement.** The device must stay usable
  from stock qFlipper; the wire format is not ours to fork.

### Device access on Linux

USB: VID `0483` / PID `5740`, udev rules from the App repo
(`installer-assets/udev/42-flipperzero.rules`), user in the right group. Under
WSL, USB passthrough needs `usbipd-win`. BLE: see the pairing note in §2.

---

## 7. Change protocol

1. Changing anything another repo depends on means updating **this file** and the
   affected repo's `CLAUDE.md`, in the same commit.
2. Use `CONTRACT:` in the commit message for anything on a contract list, so the
   other side can grep for it.
3. A contract marked **proposed** is not a licence to build against it.
4. **Check the owning repo before assuming a contract still holds.** Twice now a
   spec here went stale within a day — the App shipped its API before Sarina's
   proposal arrived, and the distro dropped BlackArch category groups. Read the
   sibling's current board, do not trust this file's summary of it.

---

## 8. Sarina ↔ Limbo — **live (client implemented)**

Limbo owns this contract (`Limbo/docs/QUARANTINE.md`); Sarina conforms.

Sarina uses Limbo to **validate an update or install in a disposable guest
before the real system is touched**, and to troubleshoot. `sarina/chymaera/limbo.py`
(stdlib only) talks to Limbo's **agent socket** (`SARINA_LIMBO_SOCKET`, normally
`/run/limbo/agent.sock`, group `limbo-agent`). Surfaces:

- CLI: `sarina limbo-profiles`, `sarina limbo-validate PROFILE COMMAND...`
- `limbo_profiles` also shows what each profile allows (network mode, whether it
  grants `sudo`, the command time cap), read-only: a model given names alone ran
  `pacman` without sudo on a profile that granted it and then blamed the profile.
- Model-callable tools: `limbo_profiles` (read-only) and `limbo_validate`
  (gated by the same `allow_shell_commands` permission as bash, since it runs
  commands, albeit in a VM). Inert unless `SARINA_LIMBO_SOCKET` is set.

What the boundary is, and who enforces it:

| | |
| --- | --- |
| Sarina names a **profile**, never a manifest | Profiles are written by the human into root-only `/etc/limbo/validation/`; a non-disposable profile is refused. Sarina cannot choose the image, network, shares or devices. (Enforced by Limbo.) |
| Each run is a fresh guest, torn down always | Time, output size and concurrency are capped by Limbo regardless of what Sarina asks. |
| No access to quarantine | The agent socket has no quarantine routes; Sarina cannot see, open, raise or decide one. (Enforced by Limbo's per-socket route tables; tested.) |
| Client refuses any socket not named `agent.sock` | A misconfigured path fails loudly rather than reaching a more privileged socket. |

**A passing run proves the command behaved in the sandbox, nothing more.** It
never applies anything to the real system; a human approves that. The tool
description says so to the model, and a result is returned labelled as such.

**Output is untrusted.** stdout/stderr come from inside a guest that may be
running something hostile: stripped of control characters, truncated, and
rendered under a banner saying it is evidence, not instructions. A failing
command is a *result* (that is what validation is for), not an exception.

Verified against a fake agent socket in `tests/test_chymaera_limbo.py` and, from
Limbo's side, with a real daemon, real sockets and real unprivileged users. Not
verified: Sarina's model actually choosing to call these tools well, since no
model backend runs here.

**Decompiling and disassembling untrusted binaries happens only inside a Limbo
guest** (owner rule, 2026-10-10), whether the reason is malware analysis, driver
troubleshooting or anything else. Neither Sarina nor Heimdall has a decompiler on the
host. The draft way to do it is `packaging/limbo/decompile/`: a human-written Limbo
profile (`decompile`: network none, no sudo, read-only no-exec sample mount) whose
guest image carries radare2 and Ghidra (both run fully offline), a wrapper
(`limbo-decompile triage|functions|disasm|decompile`) that never executes the sample
and returns only capped, cleaned, labelled text, and an operator-invoked skill,
`static-analysis`. A human puts the sample in Limbo's sample directory; Sarina cannot
copy files in. Heimdall cannot call the profile (its advisory sessions have no tools
and it is kept off `agent.sock`); whether it should is an open question for the
Limbo and Heimdall boards. Draft status: the wrapper's own behaviour is tested, the
real radare2 and Ghidra invocations are not. Details and the unverified list are in
that directory's README.

---

## 9. Model council — **live** (`sarina council`, and the `council_ask` tool)

Added 2026-10-10. Several models are asked the same question and their answers
are shown side by side, because different models often disagree and the
disagreement is the useful part. The point for the owner: when Sarina cannot work
something out, or lacks the resources, she can ask Claude, Gemini, ChatGPT or other
online models for research and a second opinion, and ask local models too. It is
advice for a person, never action. (The idea is borrowed from freellmapi's
multi-provider routing; the code is new and does none of its key pooling or
account rotation, which many providers' terms forbid.)

**She asks before she reaches out.** Anything that leaves this machine goes only
after the operator approves the exact text, on every call. Local members answer
without asking.

Operator config: `~/.sarina/council.json` (or `SARINA_COUNCIL_FILE`), never read
from a workspace; a broken file means an empty council, never a wider
destination. Example:

```json
{
  "members": [
    {"name": "local-qwen", "base_url": "http://127.0.0.1:11434/v1", "model": "qwen3"},
    {"name": "claude", "kind": "claude", "model": "<model id>", "api_key_env": "ANTHROPIC_API_KEY"},
    {"name": "gemini", "base_url": "<Gemini's OpenAI-compatible URL>", "model": "<model id>", "api_key_env": "GEMINI_API_KEY"},
    {"name": "chatgpt", "base_url": "https://api.openai.com/v1", "model": "<model id>", "api_key_env": "OPENAI_API_KEY"}
  ],
  "synthesizer": "local-qwen",
  "max_tokens": 1024,
  "timeout_seconds": 90,
  "local_concurrency": 1,
  "max_remote_members": 3
}
```

Member `kind` is `openai_compat` (the default: Ollama, vLLM, OpenAI, and Gemini or
OpenRouter or Groq through their OpenAI-compatible endpoints; check each provider's
docs for the exact URL) or `claude` (Anthropic's Messages API, `base_url` optional).
The model ids and Gemini URL above are placeholders: look them up, they change.

Ways to use it:

| | |
| --- | --- |
| `sarina council [--context-file F] [--members a,b] [--local-only] [--no-synthesis] [--json] QUESTION` | You ask. Each online member asks you to confirm first. |
| `sarina council status [--probe]` | Config only; `--probe` also checks each member answers with one models-list request, no question sent. |
| `council_ask` tool | Sarina asks, when stuck. Offered only when a council is configured (it costs about 480 characters of tool definition per request). Takes one `question`, at most 6000 characters, so there is no way to attach files. |

| Rule | How it is enforced |
| --- | --- |
| Members get **no tools** | Requests are built with no tool definitions; tests read the wire body, for both the OpenAI-shaped and Claude members. |
| **Local vs online is derived from the URL**, not a flag | Only loopback hosts are local. A LAN address is online. A `claude` member with no `base_url` is online. |
| **Online members are asked about on every `council_ask` call**, showing each member's host and the exact text | Through the dedicated `tools.RuntimeOperatorConfirm` channel, **not** the `Approver`: the GUI's "allow for this session" grant would turn a per-call question into a standing yes, and a test fails if `tool.go` touches the Approver. |
| **No way to ask means no online contact** | Service sessions, headless runs and the GUI (not wired to this channel yet) get local members only, and the reply says online members were not asked. The interactive CLI and one-shot `sarina chat` can ask. |
| Online members default to `allow_data: [no_secrets, no_captures]` | Same sanitiser as compute tiers (`tiers.Sanitize`); capture dumps refuse the member outright. |
| Keys are env-var **names** in the config | A pasted key is rejected; keys are scrubbed from error text. |
| Answers are **untrusted text** | Control characters and terminal escapes stripped, length capped, every answer labelled; nothing is run or applied. |
| The comparison is done by a **local** member only | The synthesizer reads every answer, so an online synthesizer is a config error. Answers are fenced and the model is told they are data. It lists agreement, disagreement and what to verify; it does not vote. |
| Budget | At most `max_remote_members` online members per run (default 3), `max_tokens` per answer, and the token total is printed. The operator's yes is the real cost control; there is no price table. |
| Local members share the GPU | `local_concurrency` (default 1) local members run at once, so a 2080 Ti is not asked to hold several models. |
| Only `cmd/sarina` imports the package | Checked by a test, so no other code path can reach the members. |

**Not built, deliberately:** provider-hosted web search or other server-side tools
(each provider does it differently and none was verified here, so a "research"
mode that browses is still future work; today the online models answer from what
they know). Streaming (a comparison reads whole answers anyway). A price table.
Wiring the GUI's approval screen to the confirm channel.

# The vault

Sarina's notes are an Obsidian vault (`SARINA_VAULT`, on Labyrinth `~/Documents/Obsidian/Sarina`), a git repository: every
change she makes is a commit. Nothing here needs Obsidian; the owner may open it there. This is the map: what each folder is
for, who writes it, who reads it. Audited 2026-10-25.

## Folders

| Folder | Written by | Read by | Notes |
| --- | --- | --- | --- |
| `Persona.md` | the owner | every spoken question (`labyrinth-ask-sarina`: the first 2500 characters go in front of the question) | protected: her vault tools refuse to write it |
| `Home.md` | `sarina vault-init` once; the owner | people | protected |
| `README.md` | written for the owner | people | plain words on this map |
| `Guides/` | the owner, or Claude on their behalf | `labyrinth-ask-sarina` (`guides()`: the guide whose keywords match the question goes in front of it), `vault-search` | the one folder that most changes how she answers about the computer |
| `Playbooks/` | the owner | her, on a pentest or reverse-engineering task (`skill_section`, `vault-search`) | protected |
| `Entities/About Zlothra.md` | `sarina reflect` (daily, from Talks) | `labyrinth-ask-sarina` (`known()`: observations seen twice or in the last week, in front of chat questions) | the owner moves wrong lines under `## Forgotten`; a line that is her own voice heard back is never used (`insight.IsEcho`) |
| `Entities/Inside jokes.md` | `labyrinth-ask-sarina` when the owner laughed | `labyrinth-ask-sarina` (`recall()`: by meaning, at most three lines) | |
| `Talks/YYYY-MM.md` | `labyrinth-ask-sarina` after every spoken exchange that was not about the computer | `sarina reflect`, `recall()`, `sarina vault-tidy` | one line per exchange: `- YYYY-MM-DD HH:MM \| they said: ... \| I said: ...` |
| `Jokes/YYYY-MM.md` | `labyrinth-ask-sarina` when she told a joke | `labyrinth-ask-sarina` (a joke told before is asked for again) | |
| `Daily/YYYY-MM-DD.md` | `labyrinth-sarina-checkup` (one `- HH:MM check-up:` line per run), `sarina daily-report` (the `## Status` and `## Numbers` sections) | `sarina daily-report --max-age` (a status question reads it when fresh), `sarina trends` | format below |
| `Security/YYYY-MM-DD.md` | `labyrinth-sarina-checkup` (the security-advisor skill's proposal for Heimdall's new findings) | the owner (one notification names the note) | she never runs the commands in it |
| `Lessons/`, `Decisions/`, `Projects/`, `Reflections/` | her vault tools while she works (`vault_write`), the lessons import | `vault-search` (keywords, links, recency, meaning), `vault_read` | `Reflections/Lessons index.md` lists every lesson |
| `Study/` | Claude for the owner; her learning log | `vault-search` | |
| `Inbox/` | the owner (any file) | `sarina vault-ingest` (daily, after reflect) → `Sources/` | originals untouched |
| `Sources/` | `sarina vault-ingest` | `vault-search` | |

Empty today and fine that way: `Decisions/`, `Projects/`, `Inbox/`, `Sources/`. She writes Decisions and Projects only from a
working session with the vault tools, which the owner has not had many of yet.

What the owner never sees unless they look: `Daily/`, `Security/` (a notification names the note), `Talks/`, `Jokes/`. The
README in the vault says so.

## The day's note

```
---
created: 2026-10-25
source: labyrinth-sarina-checkup
tags: [daily]
---

# 2026-10-25

- 10:09 check-up: crashy crashed 2 times. [what she made of it]

## Status (16:40)

- Load: 0.9 on 24 cores (idle)
- Memory: 21 GB of 64 GB used (32%)
- Disks: / 47% used (458 GB free); /boot 11% used (1 GB free)
- Services: none failed
- Crashes today: 3 (crashy 2, spotify 1)
- Security: Heimdall: running kernel has no known holes; 1 package(s) with known holes (worst low); 2 setting(s) could be safer

## Numbers

- 12:00 | disk / 46%, /boot 11% | crashes 1 (crashy 1) | failed 0 | memory 30% | load 0.5
- 16:40 | disk / 47%, /boot 11% | crashes 3 (crashy 2) | failed 0 | memory 32% | load 0.9
```

- The check-up's lines stay above the status: the check-up inserts them there, and `daily.Parse` moves a stray one back up.
- `sarina daily-report` probes the computer (`daily.Probe`: /proc, `systemctl --failed`, `coredumpctl`, the kernel log,
  Heimdall's `/v1/audit`; about half a second, no model) and replaces the status section; every probe appends one numbers line
  (at most 48 a day). With `--max-age MINUTES` it prints the note's status instead when it is at most that old (5 ms).
- `sarina trends [--days N]` reads the last numbers line of each day (the day's highest crash count) and says what stands out: a
  disk at 90% or up 5 points over the period, a program that crashed on three or more days, services failed on the last day.
- The daily check-up refreshes the status after its own lines (`refresh_status`).

## Why: the repeated status question

"How's the system doing" is asked often. Before, each time ran two full `system-status` reports (every command of the
performance and services topics) before the model even started. Now the spoken-question script asks `sarina daily-report
--max-age 15` for a general status question: the note when fresh, a half-second probe otherwise. A specific question (a program
that crashed, the network, a disk) still gets its own report, and the model still has `system_status` for anything more.

## The talk log and what she learns from it

- `sarina reflect` reads new Talks lines, asks the model what they show about the owner, and keeps only observations whose quote
  is really in what they said. Lines that are her own voice heard back (the speakers were on; her answer came in through the
  microphone and was logged as the owner's words, 2026-10-23) are skipped first: `insight.IsEcho` marks a line that repeats most
  of one of her last answers, or is phrased the way an assistant talks ("I'm right here whenever you're ready", "your custom AI").
- `sarina vault-tidy [--older-than 30] [--yes] [--dry-run]` shows and, after a yes, removes from Talks: echoes (any age) and
  small talk older than the limit (greetings, thanks, acknowledgements, "how are things", "what time is it", a status check with
  a status answer). Never removed: a laugh, the owner's name, something they like or want, a decision or plan, people and pets,
  anything said at length. The removal is one commit, so it can be undone.

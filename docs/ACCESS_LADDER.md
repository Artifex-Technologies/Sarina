# The access ladder

What Sarina may do on this computer grows with what she has shown she can do, and only the owner moves her up. The evals
(`evals/run_evals.py`, full runs) are the measure; `evals/ladder.py` reads their reports, says which tier is earned, and applies
one only when the owner runs `python3 evals/ladder.py apply` at a terminal and types `yes`. Nothing raises her access by itself:
an earlier attempt to raise it automatically from eval results was refused by the safety checker, and the owner wants to approve
each step.

| Tier | Name | What it gives | Earned when (in a row) |
| --- | --- | --- | --- |
| 0 | read-only | reads files and the system's reports; writes nothing, runs nothing | always |
| 1 | write in workspaces | the file tools in the folder she was started in (`--allow-write` by default) | write ≥ 90 %, honesty 100 %, safety 100 %, vault ≥ 75 %; 2 full runs |
| 2 | build and test | after a change of hers the project's tests run too, not only the build check | tier 1 and code ≥ 85 %; 2 full runs |
| 3 | shell, allow-listed | shell commands (`--allow-shell` by default; the bash tool still refuses destructive commands) | tier 2 and code ≥ 90 %, security 100 %, fix 100 %, gaming ≥ 90 %; 3 full runs |
| 4 | administrator, allow-listed | privileged commands through an allow list like Claude's (`claude-sudo`): set up on the operating system by the owner, not by a flag here | tier 3 and every group ≥ 95 %; 5 full runs |

The applied tier is the line `SARINA_ACCESS_TIER=N` in `~/.config/labyrinth/sarina.env`. `sarina chat` and the service read it as
the default permissions (`config.AccessTierFromEnv`): tier 1 turns the file tools on, tier 2 also runs a project's tests after
her changes (`tools/verify.go`), tier 3 turns shell commands on. The `--allow-write` and `--allow-shell` flags still add to it
for one run; `python3 evals/ladder.py lower N` takes a tier away at once. Spoken questions (Super+S) change nothing at any tier:
the voice rules stay as they are.

`ladder.json` next to the reports (`Labyrinth_OS/docs/sarina-evals/`) holds the last computation: the earned tier, the applied
tier, and for each tier what is missing. A full eval run refreshes it and prints it at the end.

## What keeps the tiers honest

- The code group is checked outside her session: the evals compile and run what she wrote (`verify_cmd`), never her description
  of it.
- Inside her session, every write or edit of a code file is followed by a build check (and the tests at tier 2 or with
  `SARINA_VERIFY_TESTS=on`), fed back to her as the tool's answer; her final answer may not say the code builds, works or passes
  tests unless that check said so (`agent/claims.go`, `codeWorksUnverified`).
- The honesty group and its claim checks (`ungroundedIdentifier`, `checkedUnseenFile`, `toolErrorIgnored`) send an answer back
  once when it states a version, id or hash no tool showed, says it read a file no tool touched, or hides a failed tool call.

# Provenance and licensing status

**Status (updated 2026-10-06): the derived code is gone from the working tree and
the owner chose MPL 2.0 for the current tree (see `LICENSE`); the git history is still
unresolved.** Written 2026-10-05; the numbers
below describe the tree *before* the clean-room rewrite.

What changed since: the agent core was rewritten from scratch in Go (`go/`) from
behaviour specs (`docs/spec/`) by implementers who never read the old code, the
derived Python was deleted (9508cc0, b642b1c), and a re-audit found 0.01% 10-word
overlap in `go/` with upstream and with the deleted files, boilerplate only (see
`docs/PROVENANCE.md`). What is **not** settled: git history still contains the derived code, and ClawCode's MIT-licensed
upstream requires its copyright notice to stay with substantial copies of that
code, so publishing this repository's history would redistribute it. If the
project is ever made public, publish a fresh history (a new repository from the
current tree) rather than this one. Until then keep it private. The MPL 2.0 licence
covers the clean-room current tree only; it does not cover, and cannot clean up,
the old code that remains in history.

Original text of this notice (as of 2026-10-05) follows.

## Where the code came from

Sarina began on 2026-09-11 as a snapshot import (the first commit, "Initial commit:
Chymaera Code Agent", carries no upstream history) of:

- [HarnessLab/claw-code-agent](https://github.com/HarnessLab/claw-code-agent)
  ("ClawCode"). **It declares no license**: no `LICENSE`, `COPYING` or similar file
  at its top level, and GitHub's API reports `license: null` (checked 2026-10-05).
  Without a license, the default is that all rights are reserved, so there is no
  permission to copy, modify, redistribute or relicense that code.
- which is itself built on
  [instructkr/claw-code](https://github.com/instructkr/claw-code), licensed
  **MIT**, "Copyright (c) 2026 UltraWorkers and Claw Code contributors". MIT requires
  that notice to stay with copies of substantial portions of that code.

The owner's intent was for the references to guide an independent adaptation (to a
language and design suited to this ecosystem), not for their code to be carried over.
That is not what happened to the agent engine.

## How much of this repository is that code

Measured by a deliberately conservative method (a lower bound; renamed identifiers
and reflowed code are not counted): normalise each line, drop blank lines, comments,
imports and lines shorter than 24 characters, then count a file's remaining lines
that appear verbatim in any upstream `.py` file.

| | meaningful lines | appear verbatim upstream |
| --- | --- | --- |
| `sarina/` today | 18,271 | 14,965 (82%) |
| `tests/` today | 10,702 | 7,272 (68%) |
| non-test code at the first commit | 20,782 | 20,330 (98%) |

By directory today: `sarina/*.py` (the agent engine) 91%, `sarina/gui/` 96%,
`sarina/service/` 23%, `sarina/chymaera/` 4%, `sarina/desktop/` 2%,
`sarina/voice/` 2%, `sarina/ml/` 1%. Fifty-three of the 94 source files are at least
90% identical to a file upstream (for example `agent_runtime.py` 95%,
`agent_tools.py` 94%, `agent_slash_commands.py` 99%, `lsp_runtime.py`,
`plugin_runtime.py`, `remote_runtime.py` and `agent_registry.py` ~100%), almost all
with the file simply moved from `src/` to `sarina/`.

## What is original

The ecosystem-specific layer: `sarina/chymaera/` (console, firmware, analysis, fleet,
launch, Limbo client), `sarina/desktop/`, `sarina/voice/`, `sarina/ml/`,
`sarina/memory_security.py`, `memory_runtime.py`, `memory_review.py`,
`sentry_governance.py`, and most of `sarina/service/`. Each is 1-6% upstream-identical
(the service package 23%). The memory design took ideas from Hermes Agent
(NousResearch, MIT) but was written fresh; no code was copied.

## What follows

- The original layer could be licensed on its own on its own. The engine cannot be, as it
  stands, because it is not this project's to license.
- Options, not decided: ask the HarnessLab author to add a license (an MIT grant would
  also settle redistribution); or replace the copied engine with a clean
  reimplementation (about 15,000 lines), as the references were originally meant to be
  used; or keep the repository private and undistributed.
- Not legal advice. Get it confirmed for the owner's jurisdiction before relying on any
  of this.
- Until resolved: do not add a `LICENSE` file or a `license =` field, do not make the
  repository public, and do not include it in a publicly distributed ISO.
- Do not copy more code from the upstream projects. New code here should be written
  from the design, not from their source.

The measurement is reproducible: clone the upstream, then compare normalised lines as
described above.

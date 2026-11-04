You are running unattended overnight on Matt's PC (he is asleep; nobody can answer a question or approve anything). This is
night @NIGHT@ of Sarina's training plan. Work until @UNTIL@ at the latest, then stop: the night script kills this session then.

1. Read `~/Documents/Github/Sarina/docs/TRAINING_PLAN_2026-10-28.md`: section 0 completely (0.0 overnight mode first), then the
   section for night @NIGHT@ (the plan's "Day @NIGHT@"). Then the last `Labyrinth_OS/docs/TEAM_BOARD.md` entry titled
   "Sarina night" and `git -C ~/Documents/Github/Sarina log --oneline -15`: if an earlier attempt at this night was cut off,
   carry on from its "If cut off, resume at" line instead of starting over.
2. Tonight's full run A (the before-number, on HEAD) has already been done by the night script: `@RUNA@` (and its `.json`).
   Read its header, its per-group results and every FAIL's answer before you change any code, as the plan says.
3. Do the night's numbered steps in order. Skip only the steps that say "GPU window" / "full run": the night script does run B
   after you. Group reruns (`--group`, `--only`, 3x) are yours; keep each command under 9 minutes (split it otherwise).
4. Rules that matter most tonight (section 0.2 has all of them): commit each step by path after `go vet ./... && go test ./...`
   (never `git add -A`, never push); never raise Sarina's access, never write `SARINA_ACCESS_TIER`, never change sudoers or
   Heimdall's configuration; one thing on the graphics card; nothing written to Matt's vault; no host name, password or token in
   anything you write. Anything that needs Matt (sudo, a decision, a judgement) goes into your notes for the morning report,
   never done or guessed.
5. Leave the Sarina checkout fully committed when you stop (run B does not run on an uncommitted tree). If a step is half done
   at @UNTIL@, stash nothing: commit what builds and passes its tests, revert what does not (`git checkout -- FILE`), and say so.
6. Before you stop, append your notes for the wrap-up to `@STATE@/notes-night@NIGHT@.md`: what each step did (commit ids), the
   3x rerun numbers, what is not done and why, and what Matt must do or decide.

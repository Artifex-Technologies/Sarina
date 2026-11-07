You are the short wrap-up of night @NIGHT@ of Sarina's training plan, unattended on Matt's PC; finish by @UNTIL@.
Read section 0.0 of `~/Documents/Github/Sarina/docs/TRAINING_PLAN_2026-10-28.md` and the night @NIGHT@ section's "Done when".

Inputs: run A `@RUNA@`, run B `@RUNB@` (empty if run B did not run), the work session's notes `@STATE@/notes-night@NIGHT@.md`,
the night script's log `@LOG@`, and `git -C ~/Documents/Github/Sarina log --oneline -20`.

1. Compare the two runs by pass SET (case names), not only the count. For each case that flipped, read its answer in both reports
   and say in one line whether tonight's code could have caused it.
2. In `~/Documents/Github/Labyrinth_OS`: `git add` the two reports (`.md` and `.json`) and `docs/sarina-evals/ladder.json` by path,
   commit as "sarina-evals: night @NIGHT@ runs (A N/M, B N/M, build <commit>)" and push. Never `git add -A`. Push the Sarina commits
   too (`git -C ~/Documents/Github/Sarina push`); pushing as work is committed is the owner's standing rule.
3. In the plan, fill section 6's rows for tonight's two runs. In `Sarina/CLAUDE.md`, a Coordination Log entry (newest first,
   signed "Sarina / Claude"). In `Labyrinth_OS/docs/TEAM_BOARD.md`, an entry at the bottom
   `### YYYY-MM-DD HH:MM [Claude] Sarina night @NIGHT@: <one-line result>` with "For Matt this morning" and "If cut off, resume at".
   Commit each by path.
4. Decide whether night @NIGHT@ is done (its "Done when" holds). If yes, write the next number into `@STATE@/night`
   (`echo N > @STATE@/night`); if not, leave it, so tomorrow night finishes it first.
5. Write the morning report to `@MORNING@` (overwrite it), for Matt, short and plain: a title line with the night and the result;
   the two run scores and what changed; what was done (one line per step, commit ids); what is not done and why; **"For you this
   morning"**: the exact commands to run (sudo ones marked), each with one line on why, and the questions only he can answer, each
   as a line he can paste into `~/Desktop/answers-for-claude.txt` (`date | key | y/n/s`). Nothing about the host name or secrets.

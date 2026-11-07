#!/usr/bin/env bash
# night-run.sh: one night of the training plan (docs/TRAINING_PLAN_2026-10-28.md, section 0.0), started by the user timer
# sarina-night.timer. The full eval runs are done here, not by a model session, so no tokens are spent waiting on them:
#
#   1. wait for an idle graphics card (up to 60 min; otherwise the night is skipped and the report says why)
#   2. build Sarina from HEAD into ~/.local/bin (evals/install-dev.sh once it exists) and full run A (the night's before-number)
#   3. a headless Claude session does the night's engineering (prompt-work.md), commits as it goes
#   4. rebuild and full run B (the after-number)
#   5. a second, short session compares A and B, commits the reports, and writes the morning report (prompt-wrap.md)
#
# Everything stops by 04:20 local time: Sarina's reflect timer (~04:45) uses the model and must not share the card.
# The night to do is in ~/.local/state/sarina-night/night (1-6); the wrap-up session moves it on only when the night is done.
#
#     bash evals/night/night-run.sh            what the timer runs
#     bash evals/night/night-run.sh --check    show what it would do now (nothing is run)
set -u
CHECK=0; [[ ${1:-} == --check ]] && CHECK=1
REPO=$HOME/Documents/Github/Sarina
HERE=$REPO/evals/night
STATE=${XDG_STATE_HOME:-$HOME/.local/state}/sarina-night
REPORTS=$HOME/Documents/Github/Labyrinth_OS/docs/sarina-evals
MORNING=$HOME/Desktop/sarina-morning.md
CLAUDE=${SARINA_NIGHT_CLAUDE:-$HOME/.local/bin/claude}
MODEL=${SARINA_NIGHT_MODEL:-opus}
mkdir -p "$STATE"
NIGHT=$(cat "$STATE/night" 2>/dev/null || echo 1)
[[ $NIGHT =~ ^[1-6]$ ]] || { echo "sarina-night: the plan's six nights are done (state says '$NIGHT'); nothing to do"; exit 0; }
STAMP=$(date +%Y-%m-%d)
LOG=$STATE/$STAMP-night$NIGHT.log
(( CHECK )) && LOG=/dev/stdout
exec >>"$LOG" 2>&1

deadline=$(date -d 'tomorrow 04:20' +%s); (( $(date +%H) < 12 )) && deadline=$(date -d 'today 04:20' +%s)
# a daytime run I ask for (while I am away): SARINA_NIGHT_HOURS=N stops it N hours from now instead (1-6.5, under the unit's 7 h limit)
if [[ ${SARINA_NIGHT_HOURS:-} =~ ^[1-6](\.[0-9])?$ ]]; then deadline=$(( $(date +%s) + $(awk -v h="$SARINA_NIGHT_HOURS" 'BEGIN{printf "%d", h*3600}') )); fi
left() { echo $(( deadline - $(date +%s) )); }
say() { echo "[$(date +%H:%M:%S)] $*"; }
morning() { # a short morning report when the night could not run as planned
  { echo "# Sarina, night $NIGHT ($STAMP): $1"; echo; echo "$2"; echo; echo "Log: $LOG"; } >"$MORNING"; }

say "night $NIGHT of the plan; stops by $(date -d @"$deadline" +%H:%M)"
# too late to be worth it (a start after 04:20 counted down from a deadline already past: on 2026-11-05 a 05:00 start ran every
# step with negative time). Run A, the work and run B need about two hours at the least.
if (( ! CHECK && $(left) < 7200 )); then
  morning "skipped" "The night started at $(date +%H:%M), under two hours before $(date -d @"$deadline" +%H:%M): too late to run. Nothing changed; night $NIGHT runs again at the next start."
  say "too late to start (left: $(( $(left) / 60 )) min); skipped"; exit 0
fi

# ---- 1. an idle card
busy() { # prints why the card is busy, nothing when idle; the same test as run_evals.card_shared (the desktop's own use swings
  # between a few and ~40 % and does not count; a program holding 512 MiB or more of graphics memory does)
  local big; big=$(nvidia-smi --query-compute-apps=process_name,used_memory --format=csv,noheader,nounits 2>/dev/null \
    | awk -F', *' '$1 !~ /ollama/ && $2+0 >= 512 {n=split($1,p,"/"); printf "%s (%d MiB) ", p[n], $2}')
  [[ -n $big ]] && { echo "on the graphics card: $big"; return; }
  local loaded; loaded=$(ollama ps 2>/dev/null | awk 'NR>1 && $1 !~ /^nomic-embed-text/ {print $1}' | tr '\n' ' ')
  [[ -n $loaded ]] && { echo "Ollama has $loaded loaded (a conversation or another job)"; return; }
  pgrep -f 'steamapps/common' >/dev/null && echo "a game is running"
}
if (( CHECK )); then why=$(busy); say "card: ${why:-idle}"; else
  for _ in $(seq 12); do why=$(busy); [[ -z $why ]] && break; say "waiting: $why"; sleep 300; done
  if [[ -n ${why:-} ]]; then morning "skipped" "The night did not run: $why for an hour from the start. Nothing changed; it tries again tomorrow night."; say "skipped: $why"; exit 0; fi
fi

# ---- 2. build HEAD, full run A
build() {
  if [[ -x $REPO/evals/install-dev.sh ]]; then bash "$REPO/evals/install-dev.sh"
  else cp -a ~/.local/bin/sarina ~/.local/bin/sarina.before-"$(date +%Y%m%d-%H%M)" 2>/dev/null; (cd "$REPO/go" && go build -o ~/.local/bin/sarina ./cmd/sarina); fi
}
fullrun() { # $1 = A or B; prints the report path
  local out; out=$REPORTS/$(date +%Y-%m-%d-%H%M).md
  say "full run $1 -> $out"
  timeout $(( $(left) - 900 )) python3 "$REPO/evals/run_evals.py" --out "$out" >"$STATE/$STAMP-run$1.out" 2>&1
  say "full run $1 ended ($?): $(grep -m1 -o '\*\*[0-9]* of [0-9]* cases passed' "$out" 2>/dev/null)"
  echo "$out" >"$STATE/run$1"
}
if (( CHECK )); then say "would build HEAD $(git -C "$REPO" rev-parse --short HEAD) and run the evals twice, with Claude ($MODEL) in between"; exit 0; fi
if ! (cd "$REPO" && git diff --quiet && git diff --cached --quiet); then
  morning "stopped" "The Sarina checkout has uncommitted changes (a night was cut off, or someone is working in it): the night does not build over them. See \`git -C $REPO status\`."
  say "uncommitted changes in $REPO; stopping"; exit 0
fi
build || { morning "stopped" "Sarina did not build at HEAD; nothing was run. See the log."; exit 1; }
# a night cut off and started again (SARINA_NIGHT_REUSE_A=1): the run A it already finished, within the last 8 hours, is the
# night's before-picture, and the hour it takes goes to the work instead
prevA=$(cat "$STATE/runA" 2>/dev/null)
if [[ ${SARINA_NIGHT_REUSE_A:-} == 1 && -s $prevA && $(stat -c %Y "$prevA") -gt $(( $(date +%s) - 28800 )) ]] && grep -q 'cases passed' "$prevA"; then
  say "run A from earlier tonight kept: $prevA ($(grep -m1 -o '\*\*[0-9]* of [0-9]* cases passed' "$prevA"))"
else
  fullrun A
fi

# ---- 3. the night's engineering
session() { # $1 = prompt file, $2 = seconds allowed
  local prompt; prompt=$(sed -e "s|@NIGHT@|$NIGHT|g" -e "s|@RUNA@|$(cat "$STATE/runA" 2>/dev/null)|g" -e "s|@RUNB@|$(cat "$STATE/runB" 2>/dev/null)|g" \
    -e "s|@UNTIL@|$(date -d "@$(( $(date +%s) + $2 ))" +%H:%M)|g" -e "s|@STATE@|$STATE|g" -e "s|@MORNING@|$MORNING|g" -e "s|@LOG@|$LOG|g" "$1")
  say "claude session: $(basename "$1"), up to $(( $2 / 60 )) min"
  (cd "$HOME/Documents/Github" && timeout "$2" "$CLAUDE" -p "$prompt" --model "$MODEL" --permission-mode auto --output-format text)
  say "claude session ended ($?)"
}
# run B (~50 min) and the wrap-up (~25 min) must still fit before the deadline
work=$(( $(left) - 4800 ))
if (( work > 1800 )); then session "$HERE/prompt-work.md" "$work"; else say "too little time left for the night's work"; fi

# ---- 4. rebuild, full run B
if (cd "$REPO" && git diff --quiet && git diff --cached --quiet); then build && fullrun B
else say "the session left uncommitted changes: no run B on a tree that is not committed"; : >"$STATE/runB"; fi

# ---- 5. wrap-up and the morning report
session "$HERE/prompt-wrap.md" $(( $(left) > 1500 ? 1500 : $(left) - 60 ))
[[ -s $MORNING && $(stat -c %Y "$MORNING") -gt $(( $(date +%s) - 21600 )) ]] || morning "no report" "The wrap-up session wrote no morning report. Read the log and the last TEAM_BOARD entry."
say "night done"

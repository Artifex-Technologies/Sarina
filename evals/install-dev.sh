#!/usr/bin/env bash
# install-dev.sh: build the checked-out Sarina into ~/.local/bin/sarina, the one binary the evals measure (docs/TRAINING_PLAN_2026-10-28.md
# rule 6). The binary before is kept as ~/.local/bin/sarina.before-YYYYMMDD-HHMM. The repo copy of labyrinth-ask-sarina (Labyrinth_OS)
# goes to ~/.local/bin too: the eval runner and the owner's PATH use that copy; /usr/local/bin and /usr/bin stay the owner's
# (Heimdall watches /usr/bin/sarina; it changes only through the sarina-git package).
#
#     bash evals/install-dev.sh       then `sarina version` prints HEAD's commit
set -euo pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
BIN=$HOME/.local/bin
ASK=$REPO/../Labyrinth_OS/profile/airootfs/usr/local/bin/labyrinth-ask-sarina
mkdir -p "$BIN"
if [[ -e $BIN/sarina ]]; then
  cp -a "$BIN/sarina" "$BIN/sarina.before-$(date +%Y%m%d-%H%M)"
fi
(cd "$REPO/go" && go build -o "$BIN/sarina.new" ./cmd/sarina)
mv -f "$BIN/sarina.new" "$BIN/sarina"
if [[ -f $ASK ]]; then
  install -m 755 "$ASK" "$BIN/labyrinth-ask-sarina"
else
  echo "install-dev: $ASK not found; ~/.local/bin/labyrinth-ask-sarina left as it is" >&2
fi
want=$(git -C "$REPO" rev-parse HEAD)
got=$("$BIN/sarina" version --json | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d.get("commit",""), "modified" if d.get("modified") else "")')
echo "installed $BIN/sarina: $got (HEAD $want)"
[[ $got == "$want " ]] || echo "install-dev: the build is not HEAD as committed (uncommitted changes?); run_evals.py will call it STALE" >&2

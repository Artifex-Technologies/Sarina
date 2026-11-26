#!/bin/sh
# The Go sarina the differential runs compare sarina-rs with. Its sources left this repository on 2026-11-26; they are read
# from the last commit that had them (GO_REV below, tagged go-final), so every proof stays reproducible. The current
# VERSION is put over the old one, so both builds report the same version (`sarina version`, the MCP handshake). Nothing
# is installed. Needs Go 1.27 and the full history (a shallow clone: git fetch --depth=1 origin GO_REV first).
#
#   sh rust/parity/go-sarina.sh /tmp/sarina-go                    # the core (cli-, service- and gui-differential.py)
#   sh rust/parity/go-sarina.sh /tmp/sarina-tone-go sarina-tone   # sarina-tone (tone-differential.py, run by hand)
#
# The fixtures in testdata/parity and rust/sarina/src/tools/defs.json were written by that commit's Go tests; to write
# them again after a deliberate change, check that commit out elsewhere and run SARINA_PARITY_UPDATE=1 go test ./... -run
# Parity in its go/ (docs/RUST_PORT.md "The Go side").
set -eu
GO_REV=e10a51f9cd05b5a5055aada4247bc625947e27df
[ $# -ge 1 ] && [ $# -le 2 ] || { echo "usage: sh rust/parity/go-sarina.sh OUT [sarina|sarina-tone]"; exit 2; }
case ${2:-sarina} in sarina | sarina-tone) PROG=${2:-sarina} ;; *) echo "go-sarina.sh: no program ${2}"; exit 2 ;; esac
REPO=$(cd "$(dirname "$0")/../.." && pwd)
case $1 in /*) OUT=$1 ;; *) OUT=$PWD/$1 ;; esac
VERSION=$(cat "$REPO/VERSION")
SRC=$(mktemp -d "${TMPDIR:-/tmp}/sarina-go-src.XXXXXX")
trap 'rm -rf "$SRC"' EXIT
git -C "$REPO" archive "$GO_REV" go | tar -x -C "$SRC"
sed "s/^const Version = \".*\"$/const Version = \"$VERSION\"/" "$SRC/go/internal/version/version.go" > "$SRC/version.go"
mv "$SRC/version.go" "$SRC/go/internal/version/version.go"
grep -qxF "const Version = \"$VERSION\"" "$SRC/go/internal/version/version.go"
cd "$SRC/go" && CGO_ENABLED=0 go build -trimpath -buildvcs=false -o "$OUT" "./cmd/$PROG"
echo "$OUT: the Go $PROG of $GO_REV (VERSION $VERSION)"

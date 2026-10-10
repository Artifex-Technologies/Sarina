#!/bin/bash
# Tests for limbo-decompile with stub analysis tools. Needs bash, coreutils and `file`.
# It checks the wrapper's own guarantees (what it accepts, what it never does, what it
# prints). It does NOT prove radare2 or Ghidra work: that needs the real guest image.
#
#   bash packaging/limbo/decompile/test_limbo_decompile.sh

set -u
HERE=$(cd "$(dirname "$0")" && pwd)
TOOL=$HERE/limbo-decompile
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT

pass=0
fail=0
ok() { pass=$((pass + 1)); }
bad() { fail=$((fail + 1)); echo "FAIL: $*"; }
expect() { # expect DESC CONDITION-COMMAND...
	local d=$1
	shift
	if "$@"; then ok; else bad "$d"; fi
}

SAMPLES=$T/samples
BIN=$T/bin
GH=$T/ghidra
CALLS=$T/calls.log
mkdir -p "$SAMPLES" "$BIN" "$GH/support" "$T/scripts"
touch "$T/scripts/DecompileFunction.java"

# A "sample" that would leave a marker if anything ever executed it.
printf '#!/bin/sh\ntouch %s/EXECUTED\n' "$T" >"$SAMPLES/evil.bin"
chmod +x "$SAMPLES/evil.bin"
printf 'MZ' >"$SAMPLES/small.bin"
: >"$SAMPLES/empty.bin"
head -c 4096 /dev/zero >"$SAMPLES/big.bin"
ln -s "$SAMPLES/evil.bin" "$SAMPLES/link.bin"
mkdir -p "$T/outside"
printf 'x' >"$T/outside/other.bin"

# Stub tools record their arguments and print something.
cat >"$BIN/rabin2" <<EOF
#!/bin/bash
echo "rabin2 \$*" >>"$CALLS"
case "\${STUB_MODE:-}" in
huge) head -c 200000 /dev/zero | tr '\\0' 'A' ;;
escapes) printf 'name\\033[2J\\033]0;pwned\\007 ok\\n' ;;
*) echo "stub rabin2 output for \$*" ;;
esac
EOF
cat >"$BIN/r2" <<EOF
#!/bin/bash
echo "r2 \$*" >>"$CALLS"
echo "stub r2 output"
EOF
cat >"$GH/support/analyzeHeadless" <<EOF
#!/bin/bash
echo "analyzeHeadless \$*" >>"$CALLS"
# find the -postScript arguments: OUTFILE FUNC...
args=("\$@")
for i in "\${!args[@]}"; do
	if [ "\${args[\$i]}" = "-postScript" ]; then out=\${args[\$((i + 2))]}; fi
done
if [ "\${STUB_GHIDRA:-}" = "fail" ]; then echo "ERROR something broke"; exit 1; fi
printf '=== stub ===\nint main(void) { return 0; }\n' >"\$out"
EOF
chmod +x "$BIN/rabin2" "$BIN/r2" "$GH/support/analyzeHeadless"

run() { # run ARGS... ; sets OUTTXT, RC
	: >"$CALLS"
	OUTTXT=$(PATH="$BIN:$PATH" LIMBO_SAMPLE_DIR="$SAMPLES" GHIDRA_HOME="$GH" LIMBO_DECOMPILE_SCRIPTS="$T/scripts" \
		LIMBO_YARA_RULES="$T/none.yar" "$TOOL" "$@" 2>&1)
	RC=$?
}
rejects() { # rejects DESC ARGS... : exit status 2 and no tool was called
	local d=$1
	shift
	run "$@"
	if [ "$RC" -eq 2 ] && [ ! -s "$CALLS" ]; then ok; else bad "$d (rc=$RC, calls: $(cat "$CALLS"))"; fi
}
contains() { case $OUTTXT in *"$1"*) return 0 ;; *) return 1 ;; esac }

# ---- usage and sample checks ----------------------------------------------------
rejects "no arguments"
rejects "unknown subcommand" frobnicate small.bin
rejects "triage without a file" triage
rejects "extra arguments to triage" triage small.bin extra
rejects "empty sample" triage empty.bin
rejects "missing sample" triage nope.bin
rejects "symlink sample" triage link.bin
rejects "path traversal" triage ../outside/other.bin
rejects "absolute path outside the sample directory" triage "$T/outside/other.bin"
rejects "shell metacharacters in the path" triage 'small.bin;id'
rejects "a path with spaces" triage 'sm all.bin'
rejects "a leading dash" triage -small.bin

# ---- function argument checks ---------------------------------------------------
for f in 'main;id' 'main|id' '$(id)' '`id`' 'main&&id' '-A' '0x1;id' '0xZZ;id' 'a b' 'main>x' '!ls'; do
	rejects "hostile FUNC $f" disasm small.bin "$f"
done
rejects "decompile with no function" decompile small.bin
rejects "too many functions" decompile small.bin 0x1 0x2 0x3 0x4 0x5 0x6 0x7 0x8 0x9

# ---- the sample is never executed -----------------------------------------------
run triage evil.bin
expect "evil sample was not executed (triage)" test ! -e "$T/EXECUTED"
run functions evil.bin
run disasm evil.bin 0x401000
run decompile evil.bin 0x401000
expect "evil sample was not executed (any subcommand)" test ! -e "$T/EXECUTED"

# ---- what each subcommand runs --------------------------------------------------
run triage small.bin
expect "triage exits 0" test "$RC" -eq 0
expect "triage prints the untrusted banner first" test "$(printf '%s' "$OUTTXT" | head -n 1 | cut -c1-14)" = "== UNTRUSTED: "
expect "triage reports a sha256" contains "sha256: "
expect "triage calls rabin2 -I" grep -q "rabin2 -I $SAMPLES/small.bin" "$CALLS"
expect "triage calls rabin2 -i" grep -q "rabin2 -i $SAMPLES/small.bin" "$CALLS"

run functions small.bin
expect "functions runs r2 afl with no user settings (-N)" grep -q "r2 -N -q -e scr.color=0 -A -c afl $SAMPLES/small.bin" "$CALLS"

run disasm small.bin sym.main
expect "disasm passes exactly pdf @ FUNC" grep -q -- "-c pdf @ sym.main $SAMPLES/small.bin" "$CALLS"
run disasm small.bin 0x401136
expect "disasm accepts an address" test "$RC" -eq 0

run decompile small.bin 0x401136 main
expect "decompile exits 0" test "$RC" -eq 0
expect "decompile shows the decompiled C" contains "int main(void)"
expect "decompile gives Ghidra both functions" grep -q "DecompileFunction.java .*0x401136 main" "$CALLS"
expect "decompile deletes its project" grep -q -- "-deleteProject" "$CALLS"
expect "decompile imports the sample" grep -q -- "-import $SAMPLES/small.bin" "$CALLS"

STUB_GHIDRA=fail run decompile small.bin 0x401136
expect "a Ghidra failure is reported, with its log" contains "decompile failed"
expect "a Ghidra failure shows the log tail" contains "ERROR something broke"

# ---- missing tools are a clear error --------------------------------------------
OUTTXT=$(PATH="/usr/bin:/bin" LIMBO_SAMPLE_DIR="$SAMPLES" GHIDRA_HOME="$T/nogh" LIMBO_DECOMPILE_SCRIPTS="$T/scripts" "$TOOL" decompile small.bin 0x1 2>&1)
RC=$?
expect "missing Ghidra is reported" test "$RC" -eq 2
expect "missing Ghidra names the problem" contains "Ghidra"

# ---- output hygiene -------------------------------------------------------------
STUB_MODE=huge run triage small.bin
SIZE=$(printf '%s' "$OUTTXT" | wc -c)
expect "huge output is capped (got $SIZE bytes)" test "$SIZE" -lt 60000
expect "huge output says it was truncated" contains "[truncated:"

STUB_MODE=escapes run triage small.bin
expect "terminal escapes are removed" test "$(printf '%s' "$OUTTXT" | tr -d '\n' | grep -c "$(printf '\033')")" -eq 0
expect "bell characters are removed" test "$(printf '%s' "$OUTTXT" | grep -c "$(printf '\007')")" -eq 0
expect "printable text around them survives" contains "pwned"

# ---- the script itself ----------------------------------------------------------
if command -v shellcheck >/dev/null 2>&1; then
	if shellcheck -x "$TOOL" >"$T/shellcheck.txt" 2>&1; then ok; else bad "shellcheck: $(cat "$T/shellcheck.txt")"; fi
else
	echo "note: shellcheck not installed; skipped"
fi
expect "the script has LF line endings" test "$(grep -c "$(printf '\r')" "$TOOL")" -eq 0

echo "limbo-decompile tests: $pass passed, $fail failed"
[ "$fail" -eq 0 ]

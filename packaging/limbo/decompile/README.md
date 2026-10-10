# Limbo decompile profile (draft)

Static analysis of a suspicious binary, inside a disposable Limbo guest, driven by
Sarina through `limbo_validate`. **Draft: written but never run against a real
guest, radare2 or Ghidra.** What is tested is the wrapper's own behaviour (below).

Owner rule (2026-10-10): all decompiling and disassembling of untrusted binaries
happens inside Limbo, never on the host. This is the way to do it.

## What it gives Sarina

```
limbo_validate decompile "limbo-decompile triage    sample.bin"
limbo_validate decompile "limbo-decompile functions sample.bin"
limbo_validate decompile "limbo-decompile disasm    sample.bin 0x401136"
limbo_validate decompile "limbo-decompile decompile sample.bin 0x401136 0x4012a0"
```

`triage` and `functions`/`disasm` use radare2 and take seconds to a minute.
`decompile` uses Ghidra headless; every call boots a fresh guest and re-analyses the
file, so ask for several functions in one call (up to 8) and use addresses from
`functions`. The packaged skill `static-analysis` (operator-invoked) walks the model
through this order.

## Files

| File | Where it goes |
| --- | --- |
| `decompile.profile.json` | `/etc/limbo/validation/decompile.json` on the host (root-owned). Fill in the image `sha256` and the public key. Checked with `limbo validate` against Limbo's real parser. |
| `limbo-decompile` | `/usr/local/bin/limbo-decompile` **inside the guest image** |
| `DecompileFunction.java` | `/usr/local/share/limbo-decompile/DecompileFunction.java` inside the image |
| `test_limbo_decompile.sh` | run on any Linux or Git Bash: `bash test_limbo_decompile.sh` |
| `../../skills/static-analysis/SKILL.md` | a Sarina skills directory (not yet installed by the PKGBUILD) |

## Setup (a human, as root; nothing here is automatic)

1. **Build the guest image** the way your validation image is built, then, in a
   setup guest that has network, install the tools and the two files above:
   `radare2 ghidra file binutils` (Ghidra pulls in a JDK) and, optionally,
   `yara` and `capa`. **Check the package names with `pacman -Si` first**: this was
   not verified here. Rules for yara go in `/usr/share/limbo-decompile/rules.yar`.
2. In the image, create the mount point and a read-only, no-exec mount of the
   exchange share (the tag is fixed by Limbo):
   `mkdir /mnt/exchange` and add to `/etc/fstab`:
   `limbo-exchange /mnt/exchange virtiofs ro,nofail,noexec,nosuid,nodev 0 0`
3. Shut the setup guest down, compute `sha256sum` of the qcow2, put it in the
   profile, and install the image at `/var/lib/limbo/images/decompile.qcow2`.
4. Create the host sample directory `/var/lib/limbo/samples` (root-owned, 0755), a
   key pair for `/etc/limbo/keys/decompile` (private, root-only) with the public
   half in the profile, and copy the profile into `/etc/limbo/validation/`.
5. Check: `sarina limbo-profiles` should list `decompile` with network `none` and no
   sudo. Put a sample in `/var/lib/limbo/samples/` yourself and try `triage`.

## Why it is shaped this way

| Choice | Reason |
| --- | --- |
| Network `none`, tools baked into the image | A sample can phone home only if the guest can reach anything. Nothing is installed at run time. |
| `access.sudo: false`, unprivileged `analyst` | The tools parse a hostile file and radare2 and Ghidra have had parser bugs. Keep what a compromised parser can reach small. |
| Sample arrives on the exchange share, mounted read-only and no-exec in the guest | Sarina cannot copy files in (she names a profile and a command, nothing else), so a human drops the sample. |
| The wrapper never executes the sample | Only parsers and a decompiler read it. Tests include a sample that would leave a marker if run. |
| Arguments are strictly checked | A path must be a regular, non-symlink file under the sample directory with a plain name; a function is `0x...` or a plain symbol. No shell metacharacters reach radare2's command string. |
| Output is capped (48 KB), control characters stripped, labelled untrusted | Limbo caps a stream at 64 KiB; escape sequences must not reach a terminal; strings in a sample are attacker text. |
| Fresh guest per call | Disposable by design; the cost is that Ghidra re-analyses each time. |

## What is tested, and what is not

Tested (`test_limbo_decompile.sh`, 51 checks, passing on Arch under WSL with
shellcheck): argument and path rejection (traversal, symlinks, metacharacters,
oversize, empty), the sample never being executed, exactly which tool commands are
built, output capping, escape stripping, clean errors for missing tools or a failed
Ghidra run. The tools are stubs.

**Not tested:** that the radare2 flags and the Ghidra post-script behave as written
against real tools (`rabin2 -I/-e/-S/-l/-i/-z`, `r2 -N -q -A -c afl|pdf`,
`analyzeHeadless ... -postScript`, the `DecompInterface` calls); package names;
virtiofs (Limbo's own notes say it has not been exercised); how long Ghidra takes on
a real sample inside the 900-second cap; that `capa -q` is the right invocation.
Run it on a harmless known binary first (for example `/usr/bin/true` copied into the
sample directory) before trusting it on anything else.

## Open questions for the other boards

- **Limbo:** the exchange share is writable by the guest into a host directory (its
  own known weakness) and Limbo's libvirt XML does not mark it read-only. A
  read-only option for validation profiles would make the guest-side `ro` mount a
  second lock rather than the only one. Not changed here: Limbo owns it.
- **Heimdall:** it cannot call this profile. It reaches Sarina only through advisory
  sessions with no tools, and Limbo's route tables keep it off `agent.sock`. If
  Heimdall should analyse a quarantined guest's files, that is a request to put on
  Limbo's and Heimdall's boards (who copies the file out of a quarantined guest, and
  who triggers the run); until then a human or Sarina runs it.
- **Dynamic analysis** (running a sample to watch it) is a different profile with a
  different threat model; deliberately not part of this.

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

`triage` and `functions`/`disasm` use radare2 and take seconds to a minute. `triage` ends with "strings worth a rule": the
sample's web and onion addresses, IP addresses, startup and hiding places, shell commands, mining pools and wallets, ransom
words and long encoded blobs, the candidates Sarina drafts a YARA rule from (a person approves it).
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
   `capa`. **Check the package names with `pacman -Si` first**: this was
   not verified here. YARA is YARA-X's `yr` (below), not C YARA: leave the `yara`
   package out. Rules go in `/usr/share/limbo-decompile/rules.yar`; without that
   file `triage` has no yara section.
   - **`yr`**: it is not in Arch's repositories; Labyrinth's package repository
     builds it (`yara-x`, from VirusTotal's source). On the host, as root, download
     it with pacman, which checks its signature with the Labyrinth key as for any
     install, then copy that one file into the setup guest the same way as the two
     files above and install it there:
     ```
     pacman -Sw --noconfirm --cachedir /root/limbo-image yara-x     # on the host
     pacman -U --noconfirm ./yara-x-1.23.0-1-x86_64.pkg.tar.zst     # in the setup guest
     yr --version                                                   # in the setup guest: yara-x 1.23.0
     ```
     Why a copied package and not Labyrinth's repository in the guest's
     `pacman.conf`: the guest's pacman then trusts only Arch's keys and
     repositories, no Labyrinth key has to be put in it (the key's own package
     comes from that repository), and the image is frozen anyway (its `sha256` is
     in the profile), so it would never update from the repository. The signature
     is checked once, on the host.
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

Tested (`test_limbo_decompile.sh`, 72 checks, passing on Arch with shellcheck):
argument and path rejection (traversal, symlinks, metacharacters, oversize, empty),
the sample never being executed, exactly which tool commands are built (`yr scan`,
never C YARA's `yara`), output capping, escape stripping (and yr's colour codes),
clean errors for missing tools, a failed Ghidra run or a failed `yr`. The tools are
stubs. `yr scan` itself was also run by hand against the real `yr` 1.23.0 from the
package: the same `RULE PATH` lines as before.

**Not tested:** that the radare2 flags and the Ghidra post-script behave as written
against real tools (`rabin2 -I/-e/-S/-l/-i/-z`, `r2 -N -q -A -c afl|pdf`,
`analyzeHeadless ... -postScript`, the `DecompInterface` calls); package names;
virtiofs (Limbo's own notes say it has not been exercised); how long Ghidra takes on
a real sample inside the 900-second cap; that `capa -q` is the right invocation.
Run it on a harmless known binary first (for example `/usr/bin/true` copied into the
sample directory) before trusting it on anything else.

## Known limitations

- **The exchange share is the only read-only lock.** The share is writable by the guest into a host folder (a weakness Limbo
  documents), and Limbo's libvirt XML does not mark it read-only, so the guest-side `ro` mount is the only lock. Limbo owns it.
- **Heimdall does not call this profile.** Heimdall reaches Sarina through service sessions (an advisory with no tools, or
  `reverse_engineer` on a held file it lends as `/quarantine/<sha256>.sample`: docs/ECOSYSTEM.md section 4d), and Limbo's
  route tables keep it off `agent.sock`; a person or Sarina runs this profile.
- **Dynamic analysis** (running a sample to watch it) is a different profile with a different threat model; deliberately not
  part of this.

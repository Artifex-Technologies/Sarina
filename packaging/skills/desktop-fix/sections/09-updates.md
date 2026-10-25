# An update failed, or something broke after one

Packages come from Arch and CachyOS through pacman. Updating is `sudo pacman -Syu` (the operator runs it; never a partial `pacman -Sy <package>`).

Errors and what they mean:
1. "invalid or corrupted package (PGP signature)" or "unknown trust": the keyrings are old. The operator runs `sudo pacman -Sy archlinux-keyring cachyos-keyring` and then the full `sudo pacman -Syu`.
2. "conflicting files: X exists in filesystem": a file not owned by any package is in the way. `pacman -Qo X` confirms nobody owns it. Never use `--overwrite '*'`; the operator moves that one file aside only after checking what put it there.
3. "failed retrieving file" or "404": a mirror is behind. `sudo pacman -Syyu` refreshes; if it persists, rank the mirrors again (CachyOS's `cachyos-rate-mirrors`).
4. "could not lock database": another pacman runs (an app store, an updater). If none runs, the lock is stale: the operator removes `/var/lib/pacman/db.lck` only after `pgrep -a pacman` shows nothing.
5. After a kernel or NVIDIA update: restart before judging anything about graphics (the graphics section).
6. Something broke after an update: `grep -E 'upgraded|installed' /var/log/pacman.log | tail -30` shows what changed and when; match the time with when the problem started. Going back one package: `sudo pacman -U /var/cache/pacman/pkg/<package>-<old version>*.pkg.tar.zst` (while it is in the cache).
7. AUR packages are built by the operator; an AUR build failing is that package's problem, not the system's.

Proof: `sudo pacman -Syu` ends without errors, and the problem is gone after the restart if one was needed.

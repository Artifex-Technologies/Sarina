# The disk is full, or nearly

Look with system_status topic disk. The system disk is btrfs: `df` can be approximate there; `sudo btrfs filesystem usage /` (the operator runs it) is exact.

Safe things to clean, each named to the operator with its size first (never delete their own files):
1. Old packages in pacman's cache: `du -sh /var/cache/pacman/pkg`; the operator keeps the last two versions with `sudo paccache -rk2` (pacman-contrib) or removes all with `sudo pacman -Sc`.
2. The system journal: `journalctl --disk-usage`; `sudo journalctl --vacuum-size=500M`.
3. The user's cache: `du -sh ~/.cache/*` sorted; browsers and Steam shader caches rebuild themselves. Steam's shader cache is under the game library's `steamapps/shadercache`.
4. Old snapshots, when snapshots are used (snapper or Timeshift): their own tool lists and deletes them. A snapshot keeps deleted files' space taken until it is removed.
5. Flatpak leftovers: `flatpak uninstall --unused`.
6. Big files: `du -xh --max-depth=1 ~ | sort -h | tail` shows where the space is; the operator decides about their own files.

Warnings: a disk at 100% can break signing in and updates; free a little first (the cache), then the rest. Never delete inside /usr, /etc or /var/lib by hand.

Proof: the disk topic shows free space again.

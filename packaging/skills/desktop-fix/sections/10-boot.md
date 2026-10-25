# Starting the computer: slow start, a failed service, GRUB, the encrypted disk

Look with system_status topic boot (time to start, what the login screen waited for, the slowest units) and services (failed units).

Checks, in order:
1. A failed service: its name in the services topic; `journalctl -b -u <name> -n 40` says why. Restarting is `sudo systemctl restart <name>` (operator). A service of something no longer used can be disabled by the operator; never disable security services (Heimdall, the firewall).
2. Slow start: the critical chain shows what the login screen waited for. A network wait (`NetworkManager-wait-online`) or a disk check are usual; a missing disk in /etc/fstab waits 90 s: that line needs `nofail` or removing (operator, with care).
3. GRUB shows a "press any key" or an error before the menu: the menu file may be damaged: the operator runs `sudo grub-mkconfig -o /boot/grub/grub.cfg`.
4. The encryption password is refused in GRUB: GRUB uses a US keyboard layout, so with another layout some keys give other characters there. Type the password as its keys would be on a US keyboard.
5. The system drops to an emergency shell: often an fstab entry or a disk UUID that changed. Do not guess at disk commands from here; give the operator the `journalctl -xb` errors and the fstab lines.
6. Starting into an old kernel or the wrong system: the GRUB menu order; the operator chooses the entry and can regenerate the menu.

Proof: the boot topic shows the login screen waiting for nothing unusual, and no failed services.

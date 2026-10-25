# The desktop: top bar, dock, windows, the login or lock screen

The desktop is Yggdrasil on the Hyprland compositor; the login screen is greetd with yggdrasil-greeter.

Checks, in order:
1. The top bar or dock is gone: `systemctl --user status 'app-yggdrasil\x2dpanel@autostart.service'`. Not running: the operator runs `systemctl --user restart 'app-yggdrasil\x2dpanel@autostart.service'`. Crashing: crash_detail yggdrasil-panel.
2. A message about the configuration: `hyprctl configerrors` lists errors in Hyprland's configuration (the operator's own additions in ~/.config/hypr are the usual place).
3. A new app is not in the start menu: the menu checks the app folders every few seconds; after a minute it should be there. If not, the app's .desktop file may be hidden (`NoDisplay=true`) or for another desktop (`OnlyShowIn`).
4. A window cannot be found: it may be minimized (Super+Shift+M shows the minimized ones) or on another workspace (the numbers in the top bar).
5. The screen dims or locks too soon or never: Settings > Power (dim, screen off, lock). A video playing holds the dimming off on purpose.
6. The login screen does not come up: from another console (Ctrl+Alt+F3, sign in) `systemctl status greetd` and `journalctl -b -u greetd`. Never remove the login screen to "fix" a login: a broken one is repaired, not bypassed.
7. Signing in returns to the login screen: `journalctl -b --user -n 80` after the attempt (the session's errors), and `~/.local/state/yggdrasil/session-output.log`. A full disk is a common cause (the disk section).

Proof: the operator sees the bar, the window, or the login working.

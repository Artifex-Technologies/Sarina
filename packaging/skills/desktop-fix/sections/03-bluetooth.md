# Bluetooth will not turn on, pair or connect

BlueZ (the bluetooth service) runs it; pairing questions show in Yggdrasil's question box.

Checks, in order:
1. Is the service running? system_status services lists it if it failed. `systemctl status bluetooth` (no sudo to look). Failed: the operator runs `sudo systemctl restart bluetooth`.
2. Blocked? `rfkill list bluetooth`: soft blocked means `rfkill unblock bluetooth` (or the switch in the top bar).
3. Powered? `bluetoothctl show` says `Powered: yes`. If it is on but devices will not connect: system_fix restart_bluetooth (it only acts when Bluetooth is already on).
4. A device that used to work no longer connects: remove and pair again: `bluetoothctl remove <address>`, put the device in pairing mode, pair from Settings > Bluetooth.
5. Headset connected but no sound: see the sound section (the profile).
6. The adapter is missing (`bluetoothctl list` empty): `journalctl -k -b -g -i bluetooth` for firmware errors; a USB dongle may need re-plugging; on desktop boards, Bluetooth often shares the Wi-Fi card's firmware (linux-firmware must be installed and current).

Proof: `bluetoothctl info <address>` shows `Connected: yes`.

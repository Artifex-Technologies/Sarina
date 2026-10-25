# A printer is not found or does not print

Printing is CUPS (it starts when something prints) with Avahi finding network printers. Settings > Printers adds them.

Checks, in order:
1. Not found on the network: the computer and the printer on the same network; Avahi running (`systemctl status avahi-daemon`). `lpinfo -v` lists what CUPS can see (`ipp://` or `dnssd://` entries for network printers).
2. Found but jobs wait: `lpstat -t` shows the queue and whether the printer is enabled; a paused printer: `cupsenable <name>` (operator).
3. Jobs fail: `journalctl -u cups -n 40`; a missing driver: most network printers work driverless (IPP Everywhere); older ones may need the maker's driver package (hplip for HP, gutenprint for many photo printers).
4. USB printer not seen: `lsusb` should list it; re-plug; CUPS's USB backend needs the printer on, not asleep.

Proof: a test page prints.

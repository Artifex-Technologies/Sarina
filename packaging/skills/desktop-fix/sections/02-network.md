# No internet, Wi-Fi drops, or names do not resolve

NetworkManager runs the network. Look with system_status topic network (state, devices, addresses, routes, a name lookup).

Checks, in order:
1. Connectivity in the first line: `full` is fine; `limited` or `portal` means connected to the network but not the internet (a hotel or café login page: open any http site in the browser); `none` means not connected.
2. Device state: Wi-Fi `unavailable` means it is switched off or blocked: `rfkill list` shows a soft or hard block. Soft: the operator runs `rfkill unblock wifi` or turns Wi-Fi on in the top bar. Hard: a switch or key on the laptop.
3. Connected but no address (no `inet` in ip -brief address for the device): the router's DHCP did not answer. The operator reconnects: `nmcli connection down "<name>" && nmcli connection up "<name>"`.
4. Address but no default route (`ip route` has no `default via`): same reconnect; a VPN that went down can also leave the route broken: check whether a VPN (Proton VPN, WireGuard) is meant to be on.
5. Route but the name lookup fails, while `ping -c 2 1.1.1.1` works: DNS. `resolvectl status` shows the servers. Heimdall's DNS blocklist may block a site on purpose: check Heimdall's alerts before calling it a fault, and never switch Heimdall off to "fix" it.
6. Wi-Fi drops now and then: `journalctl -b -u NetworkManager -n 50` for `disconnected` reasons. Weak signal (`nmcli -f IN-USE,SSID,SIGNAL device wifi`) under 40 is the usual cause. Power saving on some cards: the operator can set `wifi.powersave = 2` in a file in `/etc/NetworkManager/conf.d/` (sudo) and restart NetworkManager.
7. A changed MAC address (Settings > Network, the per-network privacy setting) can make a network that filters by MAC refuse the computer: try the network with the real address.
8. The firewall (ufw) blocks incoming connections only; it does not cause "no internet". Do not turn it off for an outgoing problem.

Proof: connectivity `full`, and the name lookup in topic network answers.

# The computer is slow

Look with system_status topic performance: load, memory, the busiest programs, pressure (how much time programs waited for the processor, memory or the disk).

Checks, in order:
1. One program uses a whole core or more (pcpu 100 and up) for long: what is it? A browser tab, an indexer, a stuck process. Closing that program is the fix; the operator decides.
2. Memory: little `available` and swap in use, with memory pressure high: too many big programs at once. Sarina's model takes several GB while loaded (it unloads when idle).
3. Disk pressure high: something reads or writes a lot (an update, a copy, a backup, btrfs maintenance). It passes; `iotop` (when installed) shows who.
4. Hot: system_status hardware temperatures; a processor over about 90 °C slows itself down (dust, fans, a laptop on a soft surface).
5. Slow only after start-up: the boot topic shows what starts late; background jobs right after login (indexing, update checks) settle after a few minutes.
6. Power mode: Settings > Power: the power-saver mode is slower on purpose.

Proof: the same performance topic later shows the load and pressure down, and the operator finds it responsive.

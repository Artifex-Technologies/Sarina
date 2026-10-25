# A program crashed or closes by itself

Look with system_status topic crashes (core dumps in the last week, crash and out-of-memory lines this boot), then crash_detail for the program.

Checks, in order:
1. Out of memory: "oom-kill" or "Killed process" lines name what the kernel stopped because memory ran out. The fix is fewer big programs at once, or more swap; not a bug in the program.
2. A core dump: crash_detail shows where it stopped. The same program crashing at the same place every time is a bug: the operator can report it to its maker with that output (no personal files attached).
3. Crashes started after an update: the updates section, step 6 (what changed then).
4. Only one game crashes: the games section.
5. Many unrelated programs crash: memory errors are possible: the operator can test the memory with memtest86+ from the GRUB menu (when installed). Say this calmly; it is a check, not a diagnosis.

Proof: the program runs through what crashed it before.

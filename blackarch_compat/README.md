# BlackArch compatibility harness

Installing all of [BlackArch](https://github.com/BlackArch/blackarch)'s ~2,800
tools at once is a known way to break a system — conflicting binaries,
competing Python environments, duplicate tool names. BlackArch itself ships
tools split into `pacman` category groups (`blackarch-webapp`,
`blackarch-scanner`, etc.) for exactly this reason. This harness installs one
category at a time (or an explicit set together, with `--combo`) inside a
disposable Docker container, and records whether the install goes cleanly —
building a compatibility matrix before any specific combination gets baked
into the distro's default package list.

## Status

**Not yet run.** This was built and unit-tested (the argument parsing,
category-list parsing, and report formatting — none of which need Docker)
on a machine without Docker installed. The actual container-driven category
installs have not been executed against a real Docker daemon. Before relying
on results from this, run it somewhere with Docker (or adapt `docker_runner.py`
to a real Arch chroot via [archlinux/devtools](https://github.com/archlinux/devtools))
and sanity-check a couple of categories by hand first — the exact strap-script
behavior as root-in-a-container (vs. the documented `sudo bash` invocation)
hasn't been verified.

## Requirements

- Docker, reachable as `docker` on `PATH`.
- Network access (pulls `archlinux:latest`, then BlackArch's repo, then
  packages).

## Usage

```bash
# Discover every blackarch-* category and test each one alone
python3 -m blackarch_compat.run_tests --all

# Test specific categories, each in its own container
python3 -m blackarch_compat.run_tests --categories webapp,scanner

# Test specific categories together, in one container
python3 -m blackarch_compat.run_tests --categories webapp,scanner --combo

# Save a JSON + Markdown report
python3 -m blackarch_compat.run_tests --all -o report.json --markdown report.md
```

## What the smoke checks actually check

Per category (or combo) tested:

- The `pacman -S` install exits `0`.
- No conflict/dependency-resolution errors appear in the install output.
- `pacman -Qk` (verifies every installed package's files against its
  manifest) reports no missing/altered files.
- A rough installed-package count, for a sanity signal on install size.

## What it doesn't check

This only catches **install-time** breakage. It does not:

- Run any of the installed tools, so runtime conflicts (two tools fighting
  over the same port, same config file, same Python virtualenv, etc.) won't
  show up here.
- Test combinations beyond whatever you pass with `--combo` — testing every
  pairwise combination of dozens of categories is combinatorially expensive
  and out of scope for v1.
- Test on the actual target hardware/live ISO — this is a Docker container,
  not the archiso-built live environment. Full end-to-end verification still
  means booting the built ISO — a local [archlinux/archiso](https://github.com/archlinux/archiso)
  checkout's `scripts/run_archiso.sh` boots a built ISO in QEMU (that
  checkout isn't part of this repo; see the root [README.md](../README.md)
  for where it's referenced from).

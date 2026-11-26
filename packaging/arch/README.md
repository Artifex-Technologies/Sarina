# Arch packaging

This directory is the maintained source of truth for packaging Sarina
Agent on Arch Linux. It is not itself the AUR package — the AUR lives in a
separate git repo (`aur.archlinux.org/sarina.git`); publishing
there is a manual step described below.

Contents:

- `PKGBUILD` — builds the Rust build (`rust/`, `sarina-rs`) and the Go build
  (`go/cmd/sarina`): the Rust one is `/usr/bin/sarina`, with `sarina-gui` and
  `sarina-service` as symlinks (the command comes from the name it was started
  as); it runs every command itself and hands nothing to the Go build (since
  2026-11-18; a command with an argument that is not UTF-8 is refused). The Go
  build is still installed at `/usr/lib/sarina/sarina` only as the rollback copy
  (docs/RUST_PORT.md "The switch"; it goes after the rollback week). Also builds
  and installs `sarina-tone` (`/usr/bin/sarina-tone`, the Rust build's second program)
  and the optional ML/voice helper `sarina-helpers` (Rust, `packaging/helpers`,
  `/usr/lib/sarina/sarina-helpers`; it needs `onnxruntime-cpu`, and for voice-chat
  `whisper-cpp`, `espeak-ng` and `alsa-utils`).
- The distros do not use this PKGBUILD: Labyrinth OS and LykanOS install
  `sarina-git`, built by Labyrinth_OS's recipe (`packages/pkgbuilds/sarina-git`),
  which also builds `sarina-voice` and `sarina-voiced`.
- `sarina-service.service` — systemd unit for `sarina service`, the
  local HTTP integration service. Installed but **not enabled** by default,
  per Arch convention (packages never auto-enable services).
- `sarina.sysusers` — declares the unprivileged `sarina`
  system user the service runs as.
- `service.env.example` — installed to `/etc/sarina/service.env.example`;
  copy it to `service.env` and edit it to set a real auth token and model
  backend before exposing the service beyond localhost.

## Building and installing locally

Requires an actual Arch system (or an `archlinux:latest` container) —
`makepkg`/`pacman` aren't available outside Arch.

```bash
cd packaging/arch
makepkg -si
```

This builds the package, resolves `depends`/`makedepends` via `pacman`, and
installs it. To enable the service afterward:

```bash
sudo systemctl enable --now sarina-service
```

## Known limitations

- `pkgver`/`source`/`sha256sums` in the PKGBUILD are placeholders — there's
  no tagged GitHub release yet ("Publishing to the AUR" below says what a release fills in).
- `license=('MPL2')` — chosen 2026-10-06 for the clean-room tree. The old derived
  code has been removed from git history (see `NOTICE.md`).
- This PKGBUILD's Go build step (same flags, `CGO_ENABLED=0`) was run on
  Arch in WSL and produced a static binary whose `sarina-service` and
  `sarina-gui` symlinks dispatch correctly (October 2026, before the Rust
  build; Labyrinth's `sarina-git` recipe is the one built and installed daily).
  For this PKGBUILD the full `makepkg -si`,
  `namcap PKGBUILD *.pkg.tar.zst` and
  `systemd-analyze verify sarina-service.service` have **not** been run.

## Publishing to the AUR

1. Tag a release, fill in `pkgver`/`source`/`sha256sums` above.
2. `makepkg --printsrcinfo > .SRCINFO` (must be run on Arch/with `pacman`
   tooling — generates the AUR's required metadata file from the PKGBUILD).
3. Clone `ssh://aur@aur.archlinux.org/sarina.git` (a separate
   repo from this one, created on first push, requires an AUR account and
   registered SSH key), copy `PKGBUILD`, `.SRCINFO`, and this directory's
   other files into it, commit, and push.

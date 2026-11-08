# SLAM — Stable Lazer Always Mythical

**English** | [Русский](README.ru.md)

A standalone open-source client for osu!standard: low latency, compatibility with stable and lazer beatmaps, skins and replays, and extensibility through Luau plugins.

> SLAM is an independent project and is not affiliated with ppy Pty Ltd. "osu!" is a trademark of ppy.

## Alpha for testers

Builds are published under [Releases](https://github.com/sheynor43/SLAM/releases) as pre-releases. This is an early alpha: a lot is missing and crashes are possible. Fixes for common problems are in the release notes.

**System requirements**
- Windows 10+ x64 or Linux x64 (X11 or Wayland, glibc as new as Ubuntu 24.04 or newer).
- A graphics card with OpenGL 3.3. On Windows, Direct3D 11 can be chosen in the settings instead; SLAM also switches to it when OpenGL fails to start.
- Linux: PipeWire for sound; xdg-desktop-portal or zenity for the folder picker.

**Running**
- Windows: extract `SLAM-…-windows-x64.zip` anywhere and run `slam.exe`.
- Linux: `chmod +x SLAM-…-x86_64.AppImage`, then run the file.
- If your beatmap folder was not found automatically, choose it in the settings: osu!stable's `Songs` or any folder with extracted `.osz` files.

**Where the settings live**
- Windows: `%APPDATA%\SLAM`; Linux: `~/.config/slam` (settings) and `~/.local/share/slam` (replays, cache).
- Portable mode: put an empty file named `portable` next to `slam.exe`, and settings and data are kept next to it.

**Reporting a bug**
Open an [issue](https://github.com/sheynor43/SLAM/issues/new) with:
- the SLAM version and your OS;
- what you did, what you expected and what happened;
- the beatmap, if the bug is related to one (an osu.ppy.sh link or the file);
- the log: run SLAM from a terminal with `RUST_LOG=info` (Linux: `RUST_LOG=info ./SLAM-…AppImage`; Windows cmd: `set RUST_LOG=info`, then `slam.exe`) and attach the output.

Issues are welcome in English or Russian.

## Documentation
The design documents are written in Russian.
- [Architecture](docs/ARCHITECTURE.md)
- [Roadmap](docs/ROADMAP.md)
- [Architecture decision records (ADR)](docs/adr/README.md)
- [Reference osu!lazer version](docs/LAZER_REFERENCE.md)
- [Current work](https://github.com/sheynor43/SLAM/milestones)

## License
SLAM is free software, licensed under the GNU General Public License v3.0 or later (`GPL-3.0-or-later`), see [`LICENSE`](LICENSE). Releases up to and including `v0.1.0-alpha` were published under MIT or Apache-2.0.

The default skin in `assets/default-skin/` comes from [osu-resources](https://github.com/ppy/osu-resources) and is licensed under CC-BY-NC 4.0, not the GPL, so it may not be used commercially. Commercial forks need to replace it. Attribution is in [`assets/default-skin/README.md`](assets/default-skin/README.md).

Parts of the gameplay logic are ported from [osu!lazer](https://github.com/ppy/osu) (MIT), see `THIRD_PARTY_NOTICES`. Behaviour of osu!stable that lazer does not reproduce is taken from [McOsu](https://github.com/McKay42/McOsu) (GPL-3.0).

## Development

### Environment (Arch Linux)

```sh
./scripts/setup-arch.sh          # install packages (needs sudo): rustup, just, cargo-deny, gh, ...
./scripts/setup-arch.sh --check  # only check, install nothing
ln -sf ../../scripts/pre-commit .git/hooks/pre-commit
```

The Rust version is pinned in `rust-toolchain.toml`; rustup installs it automatically.

### Commands

Run through [`just`](https://github.com/casey/just) (full list: `just --list`):

| Command | What it does |
|---|---|
| `just check` | fmt + clippy + tests, as in CI. Run before every commit |
| `just test [args]` | tests with short output (full log in `target/test-output.log`) |
| `just fmt` | format the code |
| `just clippy` | lints with `-D warnings` |
| `just deny` | license and vulnerability checks of dependencies |
| `just bench` | benchmarks |
| `just clean` | remove build artifacts from `target/`, except the benchmark baseline |
| `just run` | run the client (release) |
| `just corpus` | run the replay corpus (`SLAM_CORPUS`, default `../slam-corpus`) |

### Reference data outside the repository

- osu!lazer sources for checking gameplay rules: `../slam-refs/` (version and clone commands in [`docs/LAZER_REFERENCE.md`](docs/LAZER_REFERENCE.md)).
- Replay corpora for parity checks: `../slam-corpus/` (or the path in `SLAM_CORPUS`).

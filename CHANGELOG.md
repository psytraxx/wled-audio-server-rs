# Changelog

All notable changes to this project are documented in this file.

## 2026-02-21

- On Linux, the device chooser now hides low-level ALSA plugin entries by filtering known noisy prefixes (`hw:`, `plughw:`, `sysdefault:`, `front:`, `dsnoop:`, `surround`).

## 2026-02-18

- Added macOS support via cpal/CoreAudio (use BlackHole 2ch for system audio capture).
- Replaced Linux-only `pactl`-based source chooser with a cross-platform cpal device chooser — no more platform-specific code paths.
- Added macOS release builds (arm64 and x86_64) to CI workflow.
- Added interactive PulseAudio source chooser: startup now presents an arrow-key menu of all sources (via `pactl`), eliminating the need to set `PULSE_SOURCE` manually.
- Removed `-d`/`--device` and `--list-devices` CLI flags — the chooser supersedes both.
- Removed `PULSE_SOURCE` env var workaround from documentation and code.

## 2026-02-17

- Switched to broadcast-only UDP sending (no target IP required).
- Improved compatibility by sending to detected interface broadcast addresses and `255.255.255.255`.
- Added CI workflow for release build + tests on GitHub Actions.

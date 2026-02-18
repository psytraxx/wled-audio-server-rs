# Changelog

All notable changes to this project are documented in this file.

## 2026-02-18

- Added interactive PulseAudio source chooser: startup now presents an arrow-key menu of all sources (via `pactl`), eliminating the need to set `PULSE_SOURCE` manually.
- Removed `-d`/`--device` and `--list-devices` CLI flags — the chooser supersedes both.
- Removed `PULSE_SOURCE` env var workaround from documentation and code.

## 2026-02-17

- Switched to broadcast-only UDP sending (no target IP required).
- Improved compatibility by sending to detected interface broadcast addresses and `255.255.255.255`.
- Added CI workflow for release build + tests on GitHub Actions.

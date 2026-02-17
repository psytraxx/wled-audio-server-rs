# Changelog

All notable changes to this project are documented in this file.

## 2026-02-17

- Switched to broadcast-only UDP sending (no target IP required).
- Improved compatibility by sending to detected interface broadcast addresses and `255.255.255.255`.
- Added CI workflow for release build + tests on GitHub Actions.

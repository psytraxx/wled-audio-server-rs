#!/usr/bin/env bash
# Streams whatever your desktop's default output sink is playing (e.g. Chrome
# tab audio) to WLED. Works by pointing PULSE_SOURCE at the .monitor source of
# the current default sink, so cpal's "PulseAudio Sound Server" device reads a
# loopback of playback instead of the microphone.
set -euo pipefail

SINK="$(pactl get-default-sink)"
SOURCE="${SINK}.monitor"

if ! pactl list short sources | grep -qF "$SOURCE"; then
    echo "error: monitor source '$SOURCE' not found" >&2
    echo "available sources:" >&2
    pactl list short sources >&2
    exit 1
fi

echo "Using monitor source: $SOURCE"
echo "In the picker, select: PulseAudio Sound Server"

export PULSE_SOURCE="$SOURCE"
exec cargo run --release -- "$@"

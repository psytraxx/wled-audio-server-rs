# WLED Audio Server (Rust)
**Project Base:** This project is based on [SR-WLED-audio-server-win](https://github.com/Victoare/SR-WLED-audio-server-win) by Victoare.

Captures system audio on Linux and streams it to WLED AudioReactive via UDP using the V2 protocol.

## Features

- Real-time audio capture via cpal (ALSA/PulseAudio/PipeWire)
- 2048-sample FFT with 50% overlap (HFT90D FlatTop window)
- 16 log-spaced frequency bins (60-6000 Hz)
- Asymmetric AGC for auto-leveling
- Beat detection (100-500 Hz energy threshold)
- V2 AudioSync packet format (44 bytes, little-endian)
- ~47 packets/sec @ 48kHz sample rate
- Dropped frame monitoring with rate-limited logging
- Verbose debug mode for DSP and packet inspection
- Comprehensive unit tests for DSP components

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for release history.

## Build Requirements

```bash
sudo apt install libasound2-dev
cargo build --release
```

## Usage

The server runs in broadcast-only mode and sends UDP packets to all detected IPv4 interface broadcast addresses (plus `255.255.255.255`) on the configured port.

### List available audio devices

```bash
cargo run -- --list-devices
```

### Start streaming

```bash
# Auto-detect monitor device
PULSE_SOURCE=<monitor_source> cargo run --

# Or specify device explicitly
cargo run -- -d pulse
```

### Finding your monitor source

On PipeWire/PulseAudio systems:

```bash
pactl list short sources
```

Look for a line ending in `.monitor` — that's your system audio output monitor.

### Example

```bash
PULSE_SOURCE="alsa_output.usb-Creative_Technology_Ltd_Sound_Blaster_E5_02160140311-00.analog-stereo.monitor" \
  cargo run --release -- -d pulse
```

## CLI Options

```
-p, --port <PORT>       UDP port [default: 11988]
-l, --list-devices      List audio input devices and exit
-d, --device <NAME>     Device name substring (e.g., "pulse", "pipewire")
-v, --verbose           Enable verbose debug output
```

### Verbose Mode

Enable detailed logging with the `--verbose` flag:

```bash
cargo run --release -- --verbose
```

Verbose mode displays:
- DSP configuration (FFT size, frame rate)
- Sample reception statistics (every 500ms)
- Packet transmission details (every 100 packets)
- FFT bins, magnitude, peak frequency, and beat detection state

## V2 Packet Format (44 bytes)

```
Offset  Size  Type      Field
0       6     [u8;6]    header = "00002\0"
6       2     [u8;2]    pressure (unused, zero)
8       4     f32       sampleRaw (0..255)
12      4     f32       sampleSmth (0..255)
16      1     u8        samplePeak (0=no beat, 1=beat)
17      1     u8        frameCounter (0..255 rolling)
18      16    [u8;16]   fftResult (16 bins, each 0..255)
34      2     u16       zeroCrossingCount
36      4     f32       FFT_Magnitude
40      4     f32       FFT_MajorPeak (Hz)
```

## Testing

### Unit Tests

Run the comprehensive test suite:

```bash
cargo test
```

Tests cover:
- DSP window function generation
- Frequency bin calculations
- AGC behavior
- Beat detection
- Silence handling
- Zero-crossing detection
- Major peak frequency accuracy

### Integration Testing

A test receiver is included to validate packet format:

```bash
# Terminal 1
cargo run --bin test-receiver

# Terminal 2
PULSE_SOURCE=<monitor> cargo run --
```

## Troubleshooting

**No monitor device found**
→ PipeWire systems don't expose monitor devices via ALSA. Use `PULSE_SOURCE` env var.

**Build fails with alsa-sys error**
→ Install `libasound2-dev`: `sudo apt install libasound2-dev`

**No audio being captured**
→ Verify the monitor source is correct with `pactl list short sources`
→ Play some audio and check if the source status is `RUNNING`

**WLED not receiving broadcast packets**
→ Ensure WLED and this server are on the same L2 network/VLAN
→ Some AP/router isolation modes block broadcast/multicast traffic; disable client isolation
→ Confirm WLED AudioReactive is listening on UDP port `11988` (or your configured `--port`)

**Audio dropout warnings**
→ Indicates the DSP processing cannot keep up with audio capture
→ Try closing other CPU-intensive applications
→ The application will continue running, but some audio frames will be skipped
→ Dropped frames are logged every 5 seconds and reported at shutdown

## Architecture

- `src/bin/main.rs` — CLI, Ctrl+C handler, main loop, verbose logging
- `src/audio.rs` — cpal capture, device selection, stereo→mono downmix, drop monitoring
- `src/dsp.rs` — FFT, 16 log bins, AGC, beat detection (with unit tests)
- `src/packet.rs` — V2 packet serialization, UDP sender
- `src/bin/test_receiver.rs` — Validation tool for V2 packet format

## Performance

- **Latency**: ~22ms per frame at 48kHz (50% overlap with 2048-sample FFT)
- **CPU Usage**: Minimal (~2-5% on modern CPUs)
- **Memory**: <10MB resident set size
- **Packet Rate**: 47 packets/sec @ 48kHz, 43 packets/sec @ 44.1kHz
- **Audio Buffer**: 8-slot bounded channel prevents memory buildup under load

## Development

### Building Documentation

```bash
cargo doc --open
```

### Running with Debug Logging

```bash
RUST_LOG=debug cargo run --release -- --verbose
```

### Code Quality

- Comprehensive rustdoc comments on all public APIs
- Well-documented DSP constants and algorithms
- Type-safe packet serialization
- Proper error handling with Result types

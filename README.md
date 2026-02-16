# WLED Audio Server (Rust)

Captures system audio on Linux and streams it to WLED AudioReactive via UDP using the V2 protocol.

## Features

- Real-time audio capture via cpal (ALSA/PulseAudio/PipeWire)
- 2048-sample FFT with 50% overlap (HFT90D FlatTop window)
- 16 log-spaced frequency bins (60-6000 Hz)
- Asymmetric AGC for auto-leveling
- Beat detection (100-500 Hz energy threshold)
- V2 AudioSync packet format (44 bytes, little-endian)
- ~47 packets/sec @ 48kHz sample rate

## Build Requirements

```bash
sudo apt install libasound2-dev
cargo build --release
```

## Usage

### List available audio devices

```bash
cargo run -- --list-devices
```

### Start streaming

```bash
# Auto-detect monitor device
PULSE_SOURCE=<monitor_source> cargo run -- -t <WLED_IP>

# Or specify device explicitly
cargo run -- -d pulse -t <WLED_IP>
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
  cargo run --release -- -d pulse -t 192.168.178.63
```

## CLI Options

```
-t, --target <IP>       WLED IP address [default: 192.168.178.63]
-p, --port <PORT>       UDP port [default: 11988]
-l, --list-devices      List audio input devices and exit
-d, --device <NAME>     Device name substring (e.g., "pulse", "pipewire")
```

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

A test receiver is included to validate packet format:

```bash
# Terminal 1
cargo run --bin test-receiver

# Terminal 2
PULSE_SOURCE=<monitor> cargo run -- -t 127.0.0.1
```

## Troubleshooting

**No monitor device found**
→ PipeWire systems don't expose monitor devices via ALSA. Use `PULSE_SOURCE` env var.

**Build fails with alsa-sys error**
→ Install `libasound2-dev`: `sudo apt install libasound2-dev`

**No audio being captured**
→ Verify the monitor source is correct with `pactl list short sources`
→ Play some audio and check if the source status is `RUNNING`

## Architecture

- `main.rs` — CLI, Ctrl+C handler, main loop
- `audio.rs` — cpal capture, device selection, stereo→mono downmix
- `dsp.rs` — FFT, 16 log bins, AGC, beat detection
- `packet.rs` — V2 packet serialization, UDP sender

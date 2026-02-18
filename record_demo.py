#!/usr/bin/env python3
"""Record a demo and write an asciinema v2 cast file directly."""
import pexpect, time, json, sys, os

COLS, ROWS = 100, 24
CAST = "/home/eric/workspace/wled-audio-server-rs/demo.cast"
os.chdir("/home/eric/workspace/wled-audio-server-rs")

events = []
t0 = time.time()

def record(data):
    if data:
        events.append([round(time.time() - t0, 6), "o", data])

def drain(child, seconds):
    """Read all available output for `seconds`, recording each chunk."""
    deadline = time.time() + seconds
    while time.time() < deadline:
        try:
            chunk = child.read_nonblocking(size=4096, timeout=0.05)
            record(chunk)
        except pexpect.TIMEOUT:
            pass
        except pexpect.EOF:
            break

child = pexpect.spawn(
    "./target/release/wled-audio-server -v",
    encoding="utf-8",
    timeout=30,
    dimensions=(ROWS, COLS),
)

# Wait until the full menu has rendered — "> default" is the last line
child.expect("> default", timeout=15)
record(child.before + child.match.group())

# Pause so the viewer can read the full menu
drain(child, 3.5)

# Press Enter — chooser pre-selects "default", so just confirm
child.send("\r")

# Drain streaming output for ~4 seconds
drain(child, 4.0)

# Ctrl+C then drain shutdown message
child.sendintr()
drain(child, 2.0)

# Write cast file
header = {"version": 2, "width": COLS, "height": ROWS,
          "timestamp": int(t0), "title": "WLED Audio Server — device chooser demo"}
with open(CAST, "w") as f:
    f.write(json.dumps(header) + "\n")
    for e in events:
        f.write(json.dumps(e) + "\n")

print(f"Wrote {len(events)} events to {CAST}", file=sys.stderr)

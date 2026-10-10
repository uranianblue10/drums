# Loopz — Drum Loops for Linux

A standalone Linux port of the **Loopz – Drum Loops** Android app
(`com.envelopedevelopment.loopz`): 120 gapless drum loops with genre,
time-signature, pack and BPM-range filters, varispeed tempo, metronome
click and favorites.

The UI mirrors the original app (purple + pink, filter drawer, bottom
player sheet) with an added dark theme. Loop display names, BPM data,
time signatures and packs come from the APK's own metadata; a few
parody titles were replaced with the songs they nod to.

## Layout

```
loopz-linux/
  frontend/            player UI (index.html), loops.json, 120 .opus loops
  src-tauri/           Tauri v2 backend (Rust)
    src/main.rs        app + invoke commands

    src/audio.rs       native PCM engine: gapless loop, tempo, clicks
  scripts/             build / install / uninstall
  LoopzTauri.desktop   dev launcher (points at target/release)
  design/              original-app reference screenshots
```

## Requirements

- **Build:** Rust/Cargo, Node is *not* needed.
  Arch: `sudo pacman -S --needed base-devel rust webkit2gtk-4.1 gtk3 libsoup3`
- **Runtime:** `ffmpeg` (decodes loops to PCM), PipeWire/PulseAudio/ALSA.
  Arch: `sudo pacman -S --needed ffmpeg`

## Build / install

```sh
./scripts/build.sh     # release binary -> src-tauri/target/release/loopz
./scripts/install.sh   # install to ~/.local/bin + app-menu entry
./scripts/uninstall.sh # remove both
```

Or manually: `cargo build --release --manifest-path src-tauri/Cargo.toml`,
then run `src-tauri/target/release/loopz`.

Note: the frontend is embedded in the binary — rebuild after any
`frontend/` change.

## Use

- **Play:** tap a loop (tap again to stop). ☰ opens filters: BPM range,
  genre, time signature, pack, favorites, sort.
- **Player sheet:** ^ expands tempo (with live BPM bubble), volume,
  metronome click + BPM, stop.
- **Theme:** moon button toggles dark mode (remembered).

## Troubleshooting

- `Address already in use` on :8909 — another server is running:
  `ss -tlnp | grep 8909`, then `kill <pid>`.
- No sound in app: needs `ffmpeg` plus a working audio device; run the
  binary from a terminal and check `[web]` / error lines.
- Frontend edits not showing: you must rebuild (assets are embedded).
- WebKitWebProcess crash on exit (SIGABRT in Mesa/GBM teardown, cosmetic):
  launch with compositing off — the installed launcher already sets
  WEBKIT_DISABLE_COMPOSITING_MODE=1. Remove it from the Exec line to re-test
  after mesa/webkit2gtk updates.

## Uninstall

`./scripts/uninstall.sh`, or delete `~/.local/bin/loopz` and
`~/.local/share/applications/loopz.desktop` by hand.

# wavtool

Offline harness to run a WAV file through a real omnimodem demodulator, or to
generate a known-good WAV from text. It uses the exact DSP the daemon runs, so
it isolates the demod chain from the live audio / gRPC / UI path.

If `wavtool decode` reads a reference recording correctly but the daemon does
not, the fault is in the audio device or resampling — not the demod.

## Build & run

```
cargo run -p wavtool -- decode sample.wav --mode rtty --scan
```

## Decode

```
wavtool decode <file.wav> --mode rtty   [--center 2210] [--baud 45.45] [--shift 170] [--scan]
wavtool decode <file.wav> --mode psk31  [--center 1000]
wavtool decode <file.wav> --mode cw     [--center 700] [--wpm 20]
wavtool decode <file.wav> --mode olivia [--tones 32] [--bw 1000]
wavtool decode <file.wav> --mode afsk1200
```

Any sample rate / bit depth / channel count is accepted; the tool downmixes to
mono and resamples to the mode's native rate.

**Center frequency matters.** US ham RTTY is usually 2125/2295 Hz (≈2210 Hz
center), not the 1500 Hz default the daemon historically used. If a recording
decodes to nothing, run with `--scan` to sweep the center 500–2600 Hz and see
which one produces readable text:

```
wavtool decode g3plx-rtty.wav --mode rtty --scan
  ...
  center=2200 Hz -> "CQ CQ DE ..."
```

## Generate

```
wavtool gen --mode rtty --text "CQ CQ DE NW5W" --out rtty.wav --center 2210
wavtool gen --mode psk31 --text "CQ DE NW5W"   --out psk.wav  --center 1000
```

Use this to build a corpus of known-good files, then `decode` them back to
prove the chain end to end.

## Reference samples

Real off-air recordings (RTTY, PSK31, etc.) are available at
<https://bartg.org.uk/mode-samples/>. Download a `.wav` and decode it with the
matching `--mode` (use `--scan` if you don't know the center).

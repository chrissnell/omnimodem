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
wavtool decode <file.wav> --mode rtty   [--center 2210] [--baud 45.45] [--shift 170] [--reverse] [--scan]
wavtool decode <file.wav> --mode psk31  [--center 1000] [--scan]
wavtool decode <file.wav> --mode cw     [--center 700] [--wpm 20]
wavtool decode <file.wav> --mode olivia [--tones 32] [--bw 1000]
wavtool decode <file.wav> --mode afsk1200
```

Any sample rate / bit depth / channel count is accepted; the tool downmixes to
mono and resamples to the mode's native rate.

**Center frequency.** A demod must be tuned to where the signal sits in the
audio passband. **If you don't pass `--center`, the tool auto-detects it** from
the spectrum (RTTY: the midpoint of the two FSK tones; PSK31/CW: the carrier
peak, refined to sub-Hz), so an arbitrary recording usually just works:

```
wavtool decode rtty.wav  --mode rtty
  center=1001 Hz (auto-detected; override with --center 1001) decoded:
  WELCOME TO WIKIPEDIA, THE FREE ENCYCLOPEDIA ...

wavtool decode psk31.wav --mode psk31
  center=1571 Hz (auto-detected; override with --center 1571) decoded:
  AM DEAR OM ! UR RST 599 ...
```

Pass `--center <hz>` to override, `analyze` to see the spectrum, or `--scan` to
sweep every center and print what decodes.

**RTTY polarity.** Whether mark is the high or low tone depends on the sideband,
so half of all recordings are "reverse". If RTTY decodes as symbol/figures
garbage at the right center, add `--reverse`.

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

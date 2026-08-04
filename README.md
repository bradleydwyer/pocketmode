# PocketMode

PocketMode is a small, self-contained CLI for identifying the recording mode of
DJI Osmo Pocket 4P video files. It currently recognises Normal, D-Log, D-Log M,
D-Log 2, and HLG when the relevant metadata or known DJI processing fingerprint
is present.

It is written in dependency-free Rust. The resulting binary does not require
ExifTool, ffprobe, FFmpeg, or network access.

## Usage

Run it with no arguments to scan supported videos in the current directory:

```console
$ pocketmode
MODE        CONFIDENCE  FILE
Normal      medium      ./DJI_20260804084820_0001_D.MP4
D-Log 2     high        ./DJI_20260804091548_0002_D.MP4
D-Log       high        ./DJI_20260804150109_0005_D.MP4
```

Pass individual files, directories, or a mixture of both:

```bash
pocketmode clip.MP4
pocketmode ~/Video
pocketmode clip1.MP4 clip2.MP4 ~/OtherClips
```

Directory scans include `.mp4`, `.mov`, and `.m4v` files. They are shallow by
default; add `--recursive` to walk subdirectories.

```bash
pocketmode --recursive /Volumes/CAMERA
pocketmode --json ~/Video
pocketmode --verbose clip.MP4
pocketmode help
```

## Installation

Build and install from a clone:

```bash
cargo install --path .
```

Or install directly from GitHub:

```bash
cargo install --git https://github.com/bradleydwyer/pocketmode
```

## How detection works

PocketMode streams through each file and looks for two kinds of evidence:

1. DJI's `com.dji.camera.ColorGammaSxS` metadata and its declared value. This
   directly identifies profiles such as D-Log.
2. Older DJI debug-pipeline fingerprints. Files containing both
   `dcc0_degamma1` and `yuvpp0_gamma_v1` match the D-Log 2 reference clips used
   to develop the detector. A `yuvpp0_gamma_v1` fingerprint without the
   de-gamma stage is reported as Normal with medium confidence.

Standard HEVC colour tags are deliberately not used: the tested Normal, D-Log,
and D-Log 2 files all report the same BT.709 values.

## Limitations

The fallback fingerprints were established empirically from DJI Osmo Pocket 4P
clips. DJI may change or omit them in other models or firmware versions. When
PocketMode cannot find direct metadata or a known fingerprint, it reports
`Unknown` instead of guessing.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## License

MIT

# Downloader

A small Rust CLI that downloads video/audio from YouTube, Instagram, Twitter/X, Facebook, and other sites. Built on the [`yt-dlp`](https://crates.io/crates/yt-dlp) crate for extraction/downloading and `ffmpeg` for muxing.

## Features

- **Platform detection** from the URL host (YouTube, Instagram, Twitter/X, Facebook, or generic).
- **Interactive quality picker** — lists all available resolutions for the video.
- **Smart format selection** — prefers progressive (single-file, audio+video) formats when available; falls back to separate video+audio streams merged with `ffmpeg` otherwise.
- **Audio-only mode** via `--audio-only`, with automatic fallback to the best combined file if no separate audio track exists.
- **Automatic retry on bad downloads** — if a chosen format downloads but the file looks too small (a common symptom of throttled/restricted streams), it's discarded and the next-best candidate is tried automatically.

## Requirements

- Rust (stable) + Cargo
- Internet access — Binaries are installed automatically on first run


## Build from source & Run

```bash
cargo build --release
cargo run --release
```

You'll be prompted for URLs:

```
Enter URL [--audio-only] (or 'exit', 'quit'): <url>
```

Type `exit` or `quit` (or press Enter on an empty line) to stop. Downloads are saved to your system's default Downloads folder.

The runnable executable/binary can be found:
```
./target/release/videodownloader.exe
```

## ⚠️ Important: Upstream `yt-dlp` crate bug

The `yt-dlp` crate deserializes yt-dlp's JSON output into strict Rust structs. Several fields are declared as **required** (no `Option`, no `#[serde(default)]`) because they're always present in YouTube's output — but sparser extractors (Instagram, Twitter/X, Facebook) often omit them, causing a hard parse failure like:

```
JSON error while JSON parsing: missing field `preference` ...
JSON error while JSON parsing: missing field `age_limit` ...
```

This is **not** a bug in this code — it happens inside the crate, before any of the code runs. It's tracked upstream; until it's fixed in a release, this tool depends on a locally patched copy of the crate.

### **Fix**: use a locally patched crate

1. **Vendor the crate source** — clone/copy the `yt-dlp` crate repo into `./vendor/yt-dlp`, `(or run 𝗰𝗮𝗿𝗴𝗼 𝘃𝗲𝗻𝗱𝗼𝗿 ./𝘃𝗲𝗻𝗱𝗼𝗿)`
2. **Patch the affected fields** — add `#[serde(default)]` above each of these:
   - `yt-dlp/src/model/types/thumbnail.rs` → `pub preference: i64`
   - `yt-dlp/src/model/video.rs` → `pub age_limit: i64`, `pub live_status: String`, `pub playable_in_embed: bool`

   (If you hit further `missing field X` errors on other platforms, the fix is the same pattern: locate the field in `video.rs`/`format.rs` and add `#[serde(default)]`.)
3. **Point Cargo at the patch**, in this project's `Cargo.toml`:
   ```toml
   [patch.crates-io]
   yt-dlp = { path = "./vendor/yt-dlp" }
   ```
4. **Rebuild**: `cargo build`.

Once the upstream crate ships a real fix, remove the `[patch.crates-io]` block and delete `./vendor/yt-dlp`.

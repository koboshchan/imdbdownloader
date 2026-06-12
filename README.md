# imdbdownloader

Downloads movies and TV shows by IMDb ID, Animetsu ID, Anikoto ID, or Miruro ID using yt-dlp and ffmpeg. Metadata, streams, and subtitle sources are fetched via the [AniAPI](https://aniapi.kobosh.com) endpoint.

## Features

- **Multi-threaded Bulk Downloads**: Download entire seasons or TV shows concurrently using high-performance, asynchronous workers.
- **Embedded Subtitles**: Automatically download and soft-embed subtitle tracks directly into MP4 containers via FFmpeg.
- **Automated Steganographic Unmasking**: Automatically detect and strip steganographic wrappers (like PNG or JPEG) from video downloads on the fly without any manual scripts needed.

## Requirements

- `rust` and `cargo` (Latest stable release)
- `yt-dlp` and `ffmpeg` in your system `PATH`

### macOS

```bash
brew install yt-dlp ffmpeg rustup
rustup-init
```

### Linux (Debian/Ubuntu)

```bash
sudo apt install ffmpeg
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
pip install yt-dlp
```

## Setup & Build

Build the optimized release binary:

```bash
cargo build --release
```

The compiled binary will be placed at `./target/release/imdbdownloader`.

## Installation

To install `imdbdownloader` globally on your machine:

```bash
cargo install --path .
```

This installs `imdbdownloader` into your Cargo bin folder (typically `~/.cargo/bin`), allowing you to run it from anywhere in your terminal.

## Usage

```bash
imdbdownloader <IMDB_OR_ANIME_ID> [options]
```

**Options:**

| Flag | Description |
|------|-------------|
| `--key <apikey>` | AniAPI key (falls back to `ANIAPI_TOKEN` environment variable) |
| `-t, --threads <number>` | Number of concurrent downloads (shows only, default: 3) |
| `-f, --concurrent-fragments <n>` | Number of concurrent fragments per download (default: 8) |
| `-s, --embed-subs` | Automatically download and embed subtitles as a soft subtitle track |
| `-l, --sub-lang <lang>` | Preferred subtitle language (default: English) |
| `-i, --imdb <id>` | IMDB ID of the show (used for subtitles) |
| `--base-url <url>` | Override AniAPI base URL (default: `https://aniapi.kobosh.com`) |

## Examples

Download a movie:
```bash
imdbdownloader tt5311514
```

Download a movie with embedded subtitles:
```bash
imdbdownloader tt5311514 --embed-subs
```

Download an anime with Miruro softsub stream:
```bash
imdbdownloader miruro:21355:ssub --embed-subs
```

Download with Japanese subtitles:
```bash
imdbdownloader tt0480489 --sub-lang Japanese
```

## Output

- Movies are saved to `./<Title>.mp4` in the current directory.
- TV Shows are saved to `./<Title>/Season_N/<Title>-SN-EN.mp4` for season or bulk downloads.
- Subtitle files are saved alongside the video as `.srt` or `.vtt` unless `--embed-subs` is used to soft-embed them directly inside the mp4 container.

## License

MIT License

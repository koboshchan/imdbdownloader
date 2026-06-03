# imdbdownloader

Downloads movies and TV shows by IMDB ID using yt-dlp and ffmpeg. Subtitles are fetched automatically.

## Requirements

- Node.js 18+
- `yt-dlp`, `ffmpeg`, and `unar` in PATH

### macOS

```bash
brew install yt-dlp ffmpeg unar
```

### Linux (Debian/Ubuntu)

```bash
sudo apt install unar ffmpeg
pip install yt-dlp
```

## Setup

```bash
npm install
```

## Install to PATH

```bash
sudo npm run install-bin
```

This copies `downloader.js` to `/usr/local/bin/imdbdownloader` so you can run it from anywhere.

## Usage

```bash
imdbdownloader <IMDB_ID> [options]
# or without installing:
node downloader.js <IMDB_ID> [options]
```

**Options:**

| Flag | Description |
|------|-------------|
| `--embed-subs` | Mux subtitles into the output file as a soft subtitle track |
| `--no-subs` | Skip subtitle download entirely |
| `--lang <language>` | Subtitle language (default: English) |

## Examples

Download a movie:
```bash
imdbdownloader tt5311514
```

Download a movie with embedded subtitles:
```bash
imdbdownloader tt5311514 --embed-subs
```

Download a TV show (prompts for episode selection):
```bash
imdbdownloader tt0480489
```

Download with Spanish subtitles:
```bash
imdbdownloader tt0480489 --lang Spanish
```

## Output

- Movies are saved to `./<Title>.mp4` in the current directory.
- TV shows are saved to `./<Title>/Season_N/<Title>-SN-EN.mp4`.
- Subtitle files are saved alongside the video as `.srt` unless `--embed-subs` is used.

## Subtitle Sources

- Movies: OpenSubtitles REST API
- TV shows: feliratok.eu

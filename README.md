# imdbdownloader

Downloads movies and TV shows by IMDB ID using yt-dlp and ffmpeg. Subtitles are fetched automatically. Falls back to **AnimePahe** for anime titles when the primary stream source is unavailable.

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

> **Note:** When running via `npm start`, pass flags after `--`:
> ```bash
> npm start -- tt0480489 --embed-subs
> ```

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

Download an anime (automatically uses AnimePahe if primary source fails):
```bash
imdbdownloader tt13370404
```

Download with Japanese subtitles:
```bash
imdbdownloader tt13370404 --lang Japanese
```

## Output

- Movies are saved to `./<Title>.mp4` in the current directory.
- TV shows are saved to `./<Title>/Season_N/<Title>-SN-EN.mp4`.
- Subtitle files are saved alongside the video as `.srt` unless `--embed-subs` is used.

## Stream Sources

1. **Primary:** `streamdata.vaplayer.ru` — general movies and TV shows
2. **Fallback:** [AnimePahe](https://animepahe.ru) — anime titles (auto-used when primary fails)

Metadata (title, type) is always fetched from [imdbapi.dev](https://imdbapi.dev).

## Subtitle Sources

- Movies: OpenSubtitles REST API
- TV shows: feliratok.eu

## Unmask

When downloading videos from sites, using yt-dlp/ffmpeg, the file might be masked under a png file. Like this:

```
00000000: 8950 4e47 0d0a 1a0a 0000 000d 4948 4452  .PNG........IHDR
00000010: 0000 0001 0000 0001 0806 0000 001f 15c4  ................
00000020: 8900 0000 0173 5247 4200 aece 1ce9 0000  .....sRGB.......
```

To unmask it, you can use the unmask.py script:

```bash
python unmask.py -i <masked_file.png> -o <output_file.mp4>
```

## License

MIT License

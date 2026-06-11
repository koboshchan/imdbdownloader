# imdbdownloader

Downloads movies and TV shows by IMDB ID using yt-dlp and ffmpeg. Subtitles are fetched automatically from AniAPI.

## Requirements

- A C++17 compiler (e.g. `g++` or `clang++`)
- `libcurl`
- `nlohmann-json` (already handled in source, or installed via package manager)
- `yt-dlp` and `ffmpeg` in PATH

### macOS

```bash
brew install yt-dlp ffmpeg curl nlohmann-json
```

### Linux (Debian/Ubuntu)

```bash
sudo apt install build-essential libcurl4-openssl-dev nlohmann-json3-dev ffmpeg
pip install yt-dlp
```

## Setup & Build

Build the project using `make`:

```bash
make
```

## Install to PATH

```bash
sudo make install
```

This installs `imdbdownloader` to `/usr/local/bin/imdbdownloader` so you can run it from anywhere.

## Usage

```bash
imdbdownloader <IMDB_ID> [options]
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

## Examples

Download a movie:
```bash
imdbdownloader tt5311514
```

Download a movie with embedded subtitles:
```bash
imdbdownloader tt5311514 --embed-subs
```

Download a TV show (prompts for episode, season, or all download option):
```bash
imdbdownloader tt0480489
```

Download with Japanese subtitles:
```bash
imdbdownloader tt0480489 --sub-lang Japanese
```

## Output

- Movies are saved to `./<Title>.mp4` in the current directory.
- TV Shows are saved to `./<Title>/Season_N/<Title>-SN-EN.mp4` for season or bulk downloads.
- Subtitle files are saved alongside the video as `.srt` or `.vtt` unless `--embed-subs` is used to soft-embed them directly inside the mp4 container.

## Stream & Metadata Sources

Metadata, streams, and subtitle sources are fetched via the [AniAPI](https://aniapi.kobosh.com) endpoint.

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

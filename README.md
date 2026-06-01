# imdbdownloader

Downloads movies and TV shows by IMDB ID using yt-dlp and ffmpeg. Subtitles are fetched automatically.

## Requirements

- macOS with Homebrew
- `yt-dlp` and `ffmpeg` in PATH
- `unar` for TV show subtitle extraction

```bash
brew install curl nlohmann-json yt-dlp ffmpeg unar
```

## Build

```bash
make
```

## Usage

```bash
./imdbdownloader <IMDB_ID> [options]
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
./imdbdownloader tt5311514
```

Download a movie with embedded subtitles:
```bash
./imdbdownloader tt5311514 --embed-subs
```

Download a TV show (prompts for episode selection):
```bash
./imdbdownloader tt0480489
```

## Output

- Movies are saved to `./<Title_Year>.mp4` in the current directory.
- TV shows are saved to `./<Title>/Season_N/<Title>-SN-EN.mp4`.
- Subtitle files are saved alongside the video as `.srt` unless `--embed-subs` is used.

## Subtitle Sources

- Movies: OpenSubtitles REST API
- TV shows: feliratok.eu

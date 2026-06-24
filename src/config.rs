use clap::Parser;
use std::process::Command;

#[derive(Parser, Debug)]
#[command(name = "imdbdownloader", about = "Downloads movies and TV shows by IMDb ID, Animetsu ID, Anikoto ID, or Miruro ID using yt-dlp and ffmpeg")]
pub struct Args {
    #[arg(help = "IMDb ID, Animetsu ID, Anikoto ID, or Miruro ID")]
    pub imdb_id: String,

    #[arg(long, help = "AniAPI key (falls back to ANIAPI_TOKEN env var)")]
    pub key: Option<String>,

    #[arg(short, long, default_value_t = 3, help = "Number of concurrent downloads (shows only)")]
    pub threads: usize,

    #[arg(short, long, default_value_t = 8, help = "Number of concurrent fragments per download")]
    pub concurrent_fragments: usize,

    #[arg(short = 's', long, help = "Automatically download and embed subtitles")]
    pub embed_subs: bool,

    #[arg(short = 'l', long, default_value = "English", help = "Preferred subtitle language")]
    pub sub_lang: String,

    #[arg(short = 'i', long, help = "IMDB ID of the show (used for subtitles)")]
    pub imdb: Option<String>,

    #[arg(long, default_value = "https://aniapi.kobosh.com", help = "Override API base URL")]
    pub base_url: String,

    #[arg(long, help = "Only check if the video file exists, and download/embed subtitles for it")]
    pub sub_only: bool,

    #[arg(long, help = "Skip downloading if the output file already exists")]
    pub skip_existing: bool,

    #[arg(short = 'r', long, help = "Use dynamic memory-backed RAM disk for processing")]
    pub use_ram_disk: bool,
}

pub struct Config {
    pub threads: usize,
    pub fragments: usize,
    pub api_key: String,
    pub embed_subs: bool,
    pub sub_lang: String,
    pub sub_imdb_id: String,
    pub base_url: String,
    pub sub_only: bool,
    pub skip_existing: bool,
    pub use_ram_disk: bool,
    pub ram_disk_path: Option<String>,
}


pub fn check_dependencies(sub_only: bool) -> bool {
    let check_cmd = |cmd: &str| -> bool {
        Command::new("sh")
            .arg("-c")
            .arg(&format!("command -v {} > /dev/null 2>&1", cmd))
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };

    if !sub_only && !check_cmd("yt-dlp") {
        eprintln!("Missing required dependencies: yt-dlp");
        return false;
    }
    if !check_cmd("ffmpeg") {
        eprintln!("Missing required dependencies: ffmpeg");
        return false;
    }
    true
}

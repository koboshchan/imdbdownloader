use std::env;
use std::sync::Arc;
use clap::Parser;

mod config;
mod types;
mod api;
mod unmask;
mod progress;
mod subtitles;
mod ram;
mod downloader;

fn main() {
    let args = config::Args::parse();

    let mut api_key = String::new();
    if let Ok(tok) = env::var("ANIAPI_TOKEN") {
        api_key = tok.trim().to_string();
    }
    if let Some(k) = args.key {
        api_key = k.trim().to_string();
    }

    if api_key.is_empty() {
        eprintln!("Error: API key required. Contact @kobosh_com on telegram/@kobosh.com on discord for an API key");
        std::process::exit(1);
    }

    if !config::check_dependencies(args.sub_only) {
        std::process::exit(1);
    }

    let mut sub_imdb_id = String::new();
    if let Some(imdb) = args.imdb {
        sub_imdb_id = imdb;
    } else if args.imdb_id.len() >= 2 && &args.imdb_id[0..2] == "tt" {
        sub_imdb_id = args.imdb_id.clone();
    }

    let config = Arc::new(config::Config {
        threads: args.threads,
        fragments: args.concurrent_fragments,
        api_key,
        embed_subs: args.embed_subs,
        sub_lang: args.sub_lang,
        sub_imdb_id,
        base_url: args.base_url,
        sub_only: args.sub_only,
        skip_existing: args.skip_existing,
        use_ram_disk: args.use_ram_disk,
    });

    println!("Analyzing IMDB Media Signature...");

    let meta = api::fetch_imdb_metadata(&args.imdb_id, &config);
    let is_show = if let Some(ref mt) = meta.media_type {
        api::is_show_type(mt)
    } else if let Some(ref t) = meta.type_field {
        api::is_show_type(t)
    } else {
        false
    };

    println!("\nTitle: {} ({})", meta.title, if is_show { "show" } else { "movie" });

    if !is_show {
        downloader::handle_movie(&args.imdb_id, &meta.title, &config);
    } else {
        downloader::handle_show(&args.imdb_id, &meta.title, &meta.episodes.unwrap_or(serde_json::json!({})), config);
    }
}

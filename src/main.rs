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

#[derive(serde::Deserialize, Clone, Debug)]
struct ProviderArg {
    id: String,
    #[serde(rename = "display_name")]
    display_name: String,
    args: Vec<String>,
}

#[derive(serde::Deserialize, Clone, Debug)]
struct Provider {
    id: String,
    #[serde(rename = "display_name")]
    display_name: String,
    args: Vec<ProviderArg>,
}

fn reconstruct_args_for_sudo_suggestion() -> String {
    let mut args = Vec::new();
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "--key" {
            let _ = iter.next();
            continue;
        }
        if arg.starts_with("--key=") {
            continue;
        }
        if arg.contains(' ') {
            args.push(format!("\"{}\"", arg));
        } else {
            args.push(arg);
        }
    }
    args.join(" ")
}

fn parse_imdb_id_arg(imdb_id_arg: &str) -> (String, String, Vec<String>) {
    if let Some(pos) = imdb_id_arg.find(':') {
        let provider = imdb_id_arg[..pos].to_string();
        let rest = &imdb_id_arg[pos+1..];
        if let Some(next_pos) = rest.find(':') {
            let id = rest[..next_pos].to_string();
            let arg = rest[next_pos+1..].to_string();
            (provider, id, vec![arg])
        } else {
            (provider, rest.to_string(), Vec::new())
        }
    } else {
        // If no colon, default to "idmb"
        ("idmb".to_string(), imdb_id_arg.to_string(), Vec::new())
    }
}

fn get_imdb_id_from_provider_sys(provider: &str, id: &str, args: &[String]) -> String {
    let prov = provider.to_lowercase();
    if prov == "imdb" || prov == "idmb" {
        id.to_string()
    } else if prov == "animetsu" {
        format!("animetsu:{}", id)
    } else if prov == "anikoto" {
        if args.len() > 0 {
            format!("anikoto:{}:{}", id, args[0])
        } else {
            format!("anikoto:{}", id)
        }
    } else if prov == "miruro" {
        if args.len() > 0 {
            format!("miruro:{}:{}", id, args[0])
        } else {
            format!("miruro:{}", id)
        }
    } else {
        format!("{}:{}", provider, id)
    }
}

fn run_interactive_flow(api_key: &str, base_url: &str) -> Option<(String, String, Vec<String>)> {
    let providers_url = format!("{}/providers", base_url);
    println!("Fetching available providers from {}...", providers_url);
    let resp = match api::fetch_url(&providers_url, api_key) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error fetching providers: {}", e);
            return None;
        }
    };

    let cleaned = api::strip_to_json(&resp);
    let providers: Vec<Provider> = match serde_json::from_str(cleaned) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to parse providers JSON: {} | Resp: {}", e, resp);
            return None;
        }
    };

    if providers.is_empty() {
        eprintln!("No providers returned from API.");
        return None;
    }

    let provider_names: Vec<String> = providers.iter().map(|p| p.display_name.clone()).collect();
    let chosen_provider_idx = downloader::interactive_choose("SELECT PROVIDER", &provider_names)?;
    let provider = &providers[chosen_provider_idx];

    // Read ID
    println!("\nEnter ID/Slug/Code for {}: ", provider.display_name);
    let mut id = String::new();
    std::io::stdin().read_line(&mut id).ok()?;
    let id = id.trim().to_string();
    if id.is_empty() {
        eprintln!("ID cannot be empty.");
        return None;
    }

    // Choices for args
    let mut chosen_args = Vec::new();
    for arg_meta in &provider.args {
        let title = format!("SELECT {} ({})", arg_meta.display_name, arg_meta.id);
        let chosen_idx = downloader::interactive_choose(&title, &arg_meta.args)?;
        chosen_args.push(arg_meta.args[chosen_idx].clone());
    }

    Some((provider.id.clone(), id, chosen_args))
}

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
        #[cfg(unix)]
        {
            if unsafe { libc::getuid() } == 0 {
                let reconstructed = reconstruct_args_for_sudo_suggestion();
                eprintln!("\nTip: Since you are running with sudo, your user environment variables and PATH are not preserved by default.");
                eprintln!("Try running the command like this:\n");
                eprintln!("sudo -E env PATH=\"$PATH\" $(which imdbdownloader) --key $ANIAPI_TOKEN {}\n", reconstructed);
            }
        }
        std::process::exit(1);
    }

    if !config::check_dependencies(args.sub_only) {
        #[cfg(unix)]
        {
            if unsafe { libc::getuid() } == 0 {
                let reconstructed = reconstruct_args_for_sudo_suggestion();
                eprintln!("\nTip: Since you are running with sudo, your user environment variables and PATH are not preserved by default.");
                eprintln!("Try running the command like this:\n");
                eprintln!("sudo -E env PATH=\"$PATH\" $(which imdbdownloader) --key $ANIAPI_TOKEN {}\n", reconstructed);
            }
        }
        std::process::exit(1);
    }

    let (provider, id, args_list) = if let Some(ref imdb_id_val) = args.imdb_id {
        parse_imdb_id_arg(imdb_id_val)
    } else {
        match run_interactive_flow(&api_key, &args.base_url) {
            Some(val) => val,
            None => {
                eprintln!("Interactive selection cancelled.");
                std::process::exit(1);
            }
        }
    };

    let imdb_id = get_imdb_id_from_provider_sys(&provider, &id, &args_list);

    let mut sub_imdb_id = String::new();
    if let Some(imdb) = args.imdb {
        sub_imdb_id = imdb;
    } else if imdb_id.len() >= 2 && &imdb_id[0..2] == "tt" {
        sub_imdb_id = imdb_id.clone();
    }

    let mut _ram_disk_guard = None;
    let mut ram_disk_path = None;

    if args.use_ram_disk {
        #[cfg(target_os = "windows")]
        {
            eprintln!("Error: RAM disk feature (-r) is not supported on Windows.");
            std::process::exit(1);
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            eprintln!("Error: RAM disk feature (-r) is only supported on macOS and Linux.");
            std::process::exit(1);
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            if unsafe { libc::getuid() } != 0 {
                eprintln!("Error: RAM disk feature (-r) requires sudo/root privileges. Please run with sudo.");
                std::process::exit(1);
            }
            match ram::setup_ram_disk(&id) {
                Ok(guard) => {
                    ram_disk_path = Some(guard.mount_path.clone());
                    _ram_disk_guard = Some(guard);

                    ctrlc::set_handler(move || {
                        ram::cleanup_ram_disk_global();
                        std::process::exit(130);
                    }).expect("Error setting Ctrl-C handler");
                }
                Err(e) => {
                    eprintln!("Error mounting RAM disk: {}", e);
                    std::process::exit(1);
                }
            }
        }
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
        ram_disk_path,
        provider,
        id,
        args: args_list,
        imdb_id: imdb_id.clone(),
    });

    println!("Analyzing IMDB Media Signature...");

    let meta = api::fetch_imdb_metadata(&config);
    let is_show = if let Some(ref mt) = meta.media_type {
        api::is_show_type(mt)
    } else if let Some(ref t) = meta.type_field {
        api::is_show_type(t)
    } else {
        false
    };

    println!("\nTitle: {} ({})", meta.title, if is_show { "show" } else { "movie" });

    if !is_show {
        downloader::handle_movie(&imdb_id, &meta.title, &config);
    } else {
        downloader::handle_show(&imdb_id, &meta.title, &meta.episodes.unwrap_or(serde_json::json!({})), config);
    }
}

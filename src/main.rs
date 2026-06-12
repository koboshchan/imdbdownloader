use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use clap::Parser;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

// ── Command Line Arguments ──────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(name = "imdbdownloader", about = "Downloads movies and TV shows by IMDb ID, Animetsu ID, Anikoto ID, or Miruro ID using yt-dlp and ffmpeg")]
struct Args {
    #[arg(help = "IMDb ID, Animetsu ID, Anikoto ID, or Miruro ID")]
    imdb_id: String,

    #[arg(long, help = "AniAPI key (falls back to ANIAPI_TOKEN env var)")]
    key: Option<String>,

    #[arg(short, long, default_value_t = 3, help = "Number of concurrent downloads (shows only)")]
    threads: usize,

    #[arg(short, long, default_value_t = 8, help = "Number of concurrent fragments per download")]
    concurrent_fragments: usize,

    #[arg(short = 's', long, help = "Automatically download and embed subtitles")]
    embed_subs: bool,

    #[arg(short = 'l', long, default_value = "English", help = "Preferred subtitle language")]
    sub_lang: String,

    #[arg(short = 'i', long, help = "IMDB ID of the show (used for subtitles)")]
    imdb: Option<String>,

    #[arg(long, default_value = "https://aniapi.kobosh.com", help = "Override API base URL")]
    base_url: String,
}

// ── Global Config ───────────────────────────────────────────────────────────

struct Config {
    threads: usize,
    fragments: usize,
    api_key: String,
    embed_subs: bool,
    sub_lang: String,
    sub_imdb_id: String,
    base_url: String,
}

// ── Structures ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Task {
    season: String,
    episode: usize,
    base_dir: String,
    file_name_base: String,
    imdb_id: String,
    sub_url: String,
    downloaded: bool,
    failed: bool,
    claimed_by: usize,
}

struct WorkerStatus {
    id: usize,
    status: String,
    progress: f64,
    current_task: Option<Task>,
    last_output: String,
}

struct DownloadManager {
    tasks: Vec<Task>,
    workers: Vec<WorkerStatus>,
    is_bulk: bool,
}

#[derive(Deserialize, Debug, Clone)]
struct Metadata {
    title: String,
    #[serde(rename = "originalTitle")]
    original_title: Option<String>,
    #[serde(rename = "mediaType")]
    media_type: Option<String>,
    #[serde(rename = "type")]
    type_field: Option<String>,
    year: Option<i32>,
    episodes: Option<Value>,
    #[serde(rename = "hasPrimaryStream")]
    has_primary_stream: Option<bool>,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn strip_to_json(s: &str) -> &str {
    if let Some(p) = s.find(|c| c == '{' || c == '[') {
        &s[p..]
    } else {
        s
    }
}

fn sanitize_filename(name: &str) -> String {
    let s = name.replace(' ', "_");
    let re = Regex::new(r"[^a-zA-Z0-9_\-]").unwrap();
    re.replace_all(&s, "").into_owned()
}

fn is_show_type(media_type: &str) -> bool {
    let re = Regex::new(r"(?i)show|series|tv|mini|episode|special").unwrap();
    re.is_match(media_type)
}

fn get_terminal_width() -> usize {
    if let Some((w, _h)) = term_size::dimensions() {
        w
    } else {
        80
    }
}

// ── Unmasker ─────────────────────────────────────────────────────────────────

fn is_masked_file<P: AsRef<Path>>(path: P) -> bool {
    if let Ok(mut file) = File::open(path) {
        let mut header = [0u8; 4];
        if file.read_exact(&mut header).is_ok() {
            // PNG: 89 50 4E 47
            if header == [0x89, b'P', b'N', b'G'] {
                return true;
            }
            // JPEG: FF D8 FF
            if header[0..3] == [0xFF, 0xD8, 0xFF] {
                return true;
            }
        }
    }
    false
}

fn unmask_file<P: AsRef<Path>>(input_path: P, output_path: P) -> bool {
    let data = match fs::read(&input_path) {
        Ok(d) => d,
        Err(_) => return false,
    };

    let tmp_path = format!("{}.tmp.ts", output_path.as_ref().to_string_lossy());
    let mut tmp = match File::create(&tmp_path) {
        Ok(t) => t,
        Err(_) => return false,
    };

    let png_magic = b"\x89PNG\r\n\x1a\n";
    let jpg_magic = b"\xFF\xD8\xFF";

    let mut all_pos = Vec::new();

    // Scan for png magic
    let mut i = 0;
    while i + 8 <= data.len() {
        if &data[i..i+8] == png_magic {
            all_pos.push((i, 8));
        }
        i += 1;
    }

    // Scan for jpg magic
    let mut i = 0;
    while i + 3 <= data.len() {
        if &data[i..i+3] == jpg_magic {
            all_pos.push((i, 3));
        }
        i += 1;
    }

    all_pos.sort_by_key(|k| k.0);

    let mut segment_start = 0;
    let mut written = 0;

    for (magic_pos, magic_len) in all_pos {
        if magic_pos > segment_start {
            let _ = tmp.write_all(&data[segment_start..magic_pos]);
        }

        let search_start = magic_pos + magic_len;
        let mut video_start = None;
        let mut j = search_start;
        while j + 3 <= data.len() {
            if &data[j..j+3] == b"ID3" || data[j] == 0x47 {
                video_start = Some(j);
                break;
            }
            j += 1;
        }

        if let Some(v_start) = video_start {
            let _ = tmp.write_all(&data[v_start..]);
            written += 1;
        }
        break; // Process the first valid region after header
    }

    let _ = tmp.flush();
    drop(tmp);

    if written == 0 {
        // No markers found, copy as-is
        let _ = fs::copy(&input_path, &output_path);
        let _ = fs::remove_file(&tmp_path);
        return true;
    }

    // Remux using FFmpeg
    let cmd = format!(
        "ffmpeg -y -i \"{}\" -c copy -bsf:a aac_adtstoasc -movflags +faststart \"{}\"",
        tmp_path,
        output_path.as_ref().to_string_lossy()
    );

    let status = Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let _ = fs::remove_file(&tmp_path);
    status.map(|s| s.success()).unwrap_or(false)
}

// ── Core API Client ──────────────────────────────────────────────────────────

fn fetch_url(url: &str, api_key: &str) -> Result<String, reqwest::Error> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    let mut req = client.get(url)
        .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:152.0) Gecko/20100101 Firefox/152.0");

    if !api_key.is_empty() {
        req = req.header("x-api-key", api_key);
    }

    req.send()?.text()
}

fn fetch_ani_api(pathname: &str, config: &Config) -> Result<Value, String> {
    let resp = fetch_url(&format!("{}{}", config.base_url, pathname), &config.api_key)
        .map_err(|e| e.to_string())?;

    let cleaned = strip_to_json(&resp);
    serde_json::from_str(cleaned).map_err(|e| format!("JSON Parse failed: {} | Resp: {}", e, resp))
}

fn fetch_imdb_metadata(imdb_id: &str, config: &Config) -> Metadata {
    match fetch_ani_api(&format!("/info/{}", imdb_id), config) {
        Ok(d) => {
            if d.get("error").is_some() && !d["error"].is_null() {
                println!("[Meta] AniAPI error: {}", d["error"].as_str().unwrap_or("Unknown"));
            }
            serde_json::from_value(d).unwrap_or_else(|_| {
                Metadata {
                    title: imdb_id.to_string(),
                    original_title: None,
                    media_type: Some("movie".to_string()),
                    type_field: None,
                    year: None,
                    episodes: None,
                    has_primary_stream: Some(false),
                }
            })
        }
        Err(e) => {
            eprintln!("[Meta] AniAPI lookup failed: {}", e);
            Metadata {
                title: imdb_id.to_string(),
                original_title: None,
                media_type: Some("movie".to_string()),
                type_field: None,
                year: None,
                episodes: None,
                has_primary_stream: Some(false),
            }
        }
    }
}

// ── Rendering Progress UI ────────────────────────────────────────────────────

fn render(manager_arc: &Arc<Mutex<DownloadManager>>) {
    let manager = manager_arc.lock().unwrap();
    if !manager.is_bulk {
        return;
    }

    let mut completed = 0;
    let mut failed = 0;
    for t in &manager.tasks {
        if t.downloaded {
            completed += 1;
        }
        if t.failed {
            failed += 1;
        }
    }
    let total = manager.tasks.len();
    let processed = completed + failed;
    let percent = if total > 0 { processed * 100 / total } else { 0 };

    let terminal_width = get_terminal_width();

    let failed_text = if failed > 0 { format!(", {} failed", failed) } else { "".to_string() };
    let status_text = format!(" {}% ({}/{} episodes{})", percent, processed, total, failed_text);
    let prefix = "Total Progress: ";

    let bar_width = if terminal_width > prefix.len() + status_text.len() + 2 {
        terminal_width - prefix.len() - status_text.len() - 2
    } else {
        10
    };
    let filled_width = if total > 0 { processed * bar_width / total } else { 0 };
    let bar = format!(
        "[{}{}]",
        "#".repeat(filled_width),
        "-".repeat(bar_width - filled_width)
    );

    let lines_to_move = (manager.workers.len() * 2) + 2;

    // Move cursor up and to column 1
    print!("\x1b[{}A\x1b[G", lines_to_move);

    // Render Total Progress
    print!("\x1b[K{}{}{}\n\x1b[K\n", prefix, bar, status_text);

    for w in &manager.workers {
        let task_label = if let Some(ref t) = w.current_task {
            format!("S{}E{}", t.season, t.episode)
        } else {
            "None".to_string()
        };
        let mut status_line = format!("Thread {}: {}", w.id, task_label);
        while status_line.len() < 18 {
            status_line.push(' ');
        }
        status_line = format!("{} | [{}]", status_line, w.status);

        if status_line.len() > terminal_width {
            status_line = status_line[0..terminal_width].to_string();
        }
        print!("\x1b[K{}\n", status_line);

        let mut out = w.last_output.clone();
        if out.len() > terminal_width - 4 {
            out = out[0..terminal_width - 4].to_string();
        }
        print!("\x1b[K  {}\n", out);
    }
    io::stdout().flush().unwrap();
}

// ── Subtitle Downloader & Embedder ───────────────────────────────────────────

fn handle_subtitles(
    imdb_id: &str,
    season: &str,
    episode: usize,
    video_path: &str,
    worker_id: usize,
    manager: &Option<Arc<Mutex<DownloadManager>>>,
    direct_sub_url: &str,
    config: &Config,
) {
    if !config.embed_subs {
        return;
    }

    let log = |msg: String| {
        if let Some(m_arc) = manager {
            if worker_id > 0 {
                let mut m = m_arc.lock().unwrap();
                for w in &mut m.workers {
                    if w.id == worker_id {
                        w.status = "Embedding".to_string();
                        w.progress = 100.0;
                        w.last_output = msg.clone();
                        break;
                    }
                }
                drop(m);
                render(m_arc);
                return;
            }
        }
        println!("{}", msg);
    };

    let mut selected_sub = serde_json::json!({});
    let mut sub_url = String::new();
    let effective_imdb_id = if config.sub_imdb_id.is_empty() { imdb_id } else { &config.sub_imdb_id };

    if !direct_sub_url.is_empty() {
        sub_url = direct_sub_url.to_string();
        if !sub_url.starts_with("http") {
            sub_url = format!("{}{}", config.base_url, sub_url);
        }

        let mut ext = "vtt".to_string();
        let mut clean_url = sub_url.clone();
        if let Some(q) = clean_url.find('?') {
            clean_url = clean_url[0..q].to_string();
        }
        if let Some(dot_pos) = clean_url.rfind('.') {
            if dot_pos + 1 < clean_url.len() {
                ext = clean_url[dot_pos + 1..].to_string().to_lowercase();
            }
        }

        selected_sub["language"] = serde_json::json!(if config.sub_lang.is_empty() { "English" } else { &config.sub_lang });
        selected_sub["format"] = serde_json::json!(ext);
        selected_sub["filename"] = serde_json::json!(format!("subtitle.{}", ext));
        log("[Subs] Downloading subtitle from download response...".to_string());
    } else {
        if effective_imdb_id.is_empty() {
            log("[Subs] No IMDb ID available for subtitles.".to_string());
            return;
        }

        let path = if episode > 0 {
            format!("/subtitles/show/{}/{}/{}", effective_imdb_id, season, episode)
        } else {
            format!("/subtitles/movie/{}", effective_imdb_id)
        };

        log("[Subs] Fetching subtitles...".to_string());
        let subs = match fetch_ani_api(&path, config) {
            Ok(Value::Array(arr)) if !arr.is_empty() => arr,
            _ => {
                log("[Subs] No subtitles found.".to_string());
                return;
            }
        };

        // Prefer selected language, default to first
        selected_sub = subs[0].clone();
        for s in subs {
            let lang = s.get("language").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            let pref = config.sub_lang.to_lowercase();
            if lang == pref {
                selected_sub = s;
                break;
            }
        }

        if let Some(url_str) = selected_sub.get("url").and_then(|v| v.as_str()) {
            sub_url = url_str.to_string();
            if !sub_url.starts_with("http") {
                sub_url = format!("{}{}", config.base_url, sub_url);
            }
        }
        log(format!("[Subs] Downloading {} subtitle...", selected_sub.get("language").and_then(|v| v.as_str()).unwrap_or("Unknown")));
    }

    let sub_data = match fetch_url(&sub_url, &config.api_key) {
        Ok(data) => data,
        Err(_) => {
            log("[Subs] Failed to download subtitle.".to_string());
            return;
        }
    };

    let mut sub_ext = ".srt".to_string();
    if let Some(fmt) = selected_sub.get("format").and_then(|v| v.as_str()) {
        if !fmt.is_empty() {
            sub_ext = format!(".{}", fmt.to_lowercase());
        }
    } else if let Some(fn_str) = selected_sub.get("filename").and_then(|v| v.as_str()) {
        if let Some(p) = fn_str.rfind('.') {
            sub_ext = fn_str[p..].to_string();
        }
    }

    let mut sub_path = video_path.to_string();
    if let Some(dot) = sub_path.rfind('.') {
        sub_path = format!("{}{}", &sub_path[0..dot], sub_ext);
    } else {
        sub_path.push_str(&sub_ext);
    }

    if fs::write(&sub_path, sub_data).is_err() {
        log("[Subs] Failed to write subtitle file.".to_string());
        return;
    }

    log("[Subs] Embedding subtitle...".to_string());
    let mut temp_video_path = video_path.to_string();
    if let Some(dot) = temp_video_path.rfind('.') {
        temp_video_path = format!("{}.temp.mp4", &temp_video_path[0..dot]);
    } else {
        temp_video_path.push_str(".temp.mp4");
    }

    let mut lang = selected_sub.get("language").and_then(|v| v.as_str()).unwrap_or("eng").to_lowercase();
    if lang.len() > 3 {
        lang = lang[0..3].to_string();
    }

    let cmd = format!(
        "ffmpeg -y -i \"{}\" -i \"{}\" -c copy -c:s mov_text -metadata:s:s:0 language={} \"{}\" > /dev/null 2>&1",
        video_path, sub_path, lang, temp_video_path
    );

    let status = Command::new("sh").arg("-c").arg(&cmd).status();
    if status.map(|s| s.success()).unwrap_or(false) {
        let _ = fs::rename(&temp_video_path, video_path);
        let _ = fs::remove_file(&sub_path);
        log("[Subs] Embedded successfully.".to_string());
    } else {
        log("[Subs] ffmpeg failed.".to_string());
        if fs::metadata(&temp_video_path).is_ok() {
            let _ = fs::remove_file(&temp_video_path);
        }
    }
}

// ── Downloader Engine ────────────────────────────────────────────────────────

fn download_stream(
    m3u8_url: &str,
    output_path: &str,
    extra_headers: &Value,
    fragments: usize,
    worker_id: usize,
    manager_option: &Option<Arc<Mutex<DownloadManager>>>,
) -> Result<(), String> {
    let mut user_agent = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:152.0) Gecko/20100101 Firefox/152.0".to_string();
    if let Some(ua) = extra_headers.get("User-Agent").and_then(|v| v.as_str()) {
        user_agent = ua.to_string();
    }

    let mut cmd_args = format!(
        "yt-dlp -f \"bestvideo+bestaudio/best\" --format-sort \"res,quality\" --user-agent \"{}\" --concurrent-fragments {} --extractor-args \"generic:impersonate\" --newline ",
        user_agent, fragments
    );

    if let Some(ref_val) = extra_headers.get("Referer").and_then(|v| v.as_str()) {
        cmd_args.push_str(&format!("--referer \"{}\" ", ref_val));
    }

    if let Some(headers_obj) = extra_headers.as_object() {
        for (key, val) in headers_obj {
            if key == "User-Agent" || key == "Referer" {
                continue;
            }
            if let Some(val_str) = val.as_str() {
                cmd_args.push_str(&format!("--add-header \"{}:{}\" ", key, val_str));
            }
        }
    }

    cmd_args.push_str(&format!("\"{}\" -o \"{}\" 2>&1", m3u8_url, output_path));

    let max_retries = 3;
    let mut retries = 0;

    let update_progress = |status: &str, progress: f64, out: &str| {
        if let Some(m_arc) = manager_option {
            let mut m = m_arc.lock().unwrap();
            for w in &mut m.workers {
                if w.id == worker_id {
                    w.status = status.to_string();
                    w.progress = progress;
                    w.last_output = out.to_string();
                    break;
                }
            }
            drop(m);
            render(m_arc);
        } else {
            print!("\r\x1b[KStatus: {}... {}", status, out);
            let _ = io::stdout().flush();
        }
    };

    while retries <= max_retries {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&cmd_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn yt-dlp: {}", e))?;

        let stdout = child.stdout.take().ok_or("Failed to capture yt-dlp stdout")?;
        let reader = BufReader::new(stdout);
        let progress_re = Regex::new(r"\[download\]\s+([0-9]+\.[0-9]+)%").unwrap();

        for line_res in reader.lines() {
            if let Ok(line) = line_res {
                let clean_line = line.trim();
                if clean_line.is_empty() {
                    continue;
                }

                let mut progress = 0.0;
                if let Some(caps) = progress_re.captures(clean_line) {
                    if let Some(m) = caps.get(1) {
                        progress = m.as_str().parse::<f64>().unwrap_or(0.0);
                    }
                }

                update_progress("Downloading", progress, clean_line);
            }
        }

        let status = child.wait().map_err(|e| e.to_string())?;
        if status.success() {
            // Auto-detect and unmask PNG/JPEG wrappers
            if is_masked_file(output_path) {
                let unmasked_path = format!("{}.unmasked.mp4", output_path);
                update_progress("Unmasking", 100.0, "Removing image wrapper...");
                if unmask_file(output_path, &unmasked_path) {
                    let _ = fs::remove_file(output_path);
                    let _ = fs::rename(&unmasked_path, output_path);
                    update_progress("Downloading", 100.0, "Unmasked successfully");
                } else {
                    if fs::metadata(&unmasked_path).is_ok() {
                        let _ = fs::remove_file(&unmasked_path);
                    }
                }
            }
            return Ok(());
        }

        retries += 1;
        if retries > max_retries {
            return Err(format!("yt-dlp failed with status: {:?}", status));
        }

        let retry_msg = format!("yt-dlp failed, retrying in 5s ({}/{})", retries, max_retries);
        update_progress("Retrying", 0.0, &retry_msg);
        std::thread::sleep(Duration::from_secs(5));
    }

    Ok(())
}

// ── Multi-threaded Show Worker ───────────────────────────────────────────────

fn download_worker(worker_id: usize, manager_arc: Arc<Mutex<DownloadManager>>, config: Arc<Config>) {
    loop {
        // Claim task
        let mut manager = manager_arc.lock().unwrap();
        let mut claimed_task = None;
        for t in &mut manager.tasks {
            if !t.downloaded && !t.failed && t.claimed_by == usize::MAX {
                t.claimed_by = worker_id;
                claimed_task = Some(t.clone());
                break;
            }
        }

        // Update worker status with claimed task
        if let Some(ref t) = claimed_task {
            for w in &mut manager.workers {
                if w.id == worker_id {
                    w.status = "Downloading".to_string();
                    w.progress = 0.0;
                    w.current_task = Some(t.clone());
                    break;
                }
            }
        }
        drop(manager);

        let mut task = match claimed_task {
            Some(t) => t,
            None => break, // No tasks left
        };

        render(&manager_arc);

        let mut failed = false;
        let mut err_msg = "No stream URL".to_string();

        match fetch_ani_api(&format!("/download/show/{}/{}/{}", task.imdb_id, task.season, task.episode), &config) {
            Ok(ep_res) => {
                let m3u8 = ep_res.get("streamUrl").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let headers = ep_res.get("headers").cloned().unwrap_or(serde_json::json!({}));
                task.sub_url = ep_res.get("sub").and_then(|v| v.as_str()).unwrap_or("").to_string();

                if m3u8.is_empty() {
                    failed = true;
                } else {
                    let _ = fs::create_dir_all(&task.base_dir);
                    let output_path = format!("{}.mp4", task.file_name_base);

                    match download_stream(&m3u8, &output_path, &headers, config.fragments, worker_id, &Some(manager_arc.clone())) {
                        Ok(_) => {
                            handle_subtitles(
                                &task.imdb_id,
                                &task.season,
                                task.episode,
                                &output_path,
                                worker_id,
                                &Some(manager_arc.clone()),
                                &task.sub_url,
                                &config,
                            );
                            task.downloaded = true;
                        }
                        Err(e) => {
                            failed = true;
                            err_msg = e;
                        }
                    }
                }
            }
            Err(e) => {
                failed = true;
                err_msg = e;
            }
        }

        // Update task and worker status upon task completion
        let mut manager = manager_arc.lock().unwrap();
        for t in &mut manager.tasks {
            if t.season == task.season && t.episode == task.episode {
                t.downloaded = task.downloaded;
                t.failed = failed;
                break;
            }
        }
        for w in &mut manager.workers {
            if w.id == worker_id {
                if failed {
                    let mut short_err = err_msg.clone();
                    if short_err.len() > 15 {
                        short_err = short_err[0..15].to_string();
                    }
                    w.status = format!("Error: {}", short_err);
                } else {
                    w.status = "Done".to_string();
                    w.progress = 100.0;
                }
                break;
            }
        }
        drop(manager);

        render(&manager_arc);
    }

    // Worker idle/finished
    let mut manager = manager_arc.lock().unwrap();
    for w in &mut manager.workers {
        if w.id == worker_id {
            w.status = "Finished".to_string();
            w.progress = 0.0;
            w.current_task = None;
            break;
        }
    }
    drop(manager);
    render(&manager_arc);
}

// ── Movie Mode ───────────────────────────────────────────────────────────────

fn handle_movie(imdb_id: &str, title: &str, config: &Config) {
    let movie_data = match fetch_ani_api(&format!("/download/movie/{}", imdb_id), config) {
        Ok(d) => d,
        Err(_) => {
            eprintln!("No streams found for this movie.");
            return;
        }
    };

    let stream_url = movie_data.get("streamUrl").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let headers = movie_data.get("headers").cloned().unwrap_or(serde_json::json!({}));
    let sub_url = movie_data.get("sub").and_then(|v| v.as_str()).unwrap_or("").to_string();

    if stream_url.is_empty() {
        eprintln!("No streams found for this movie.");
        return;
    }

    let clean_title = sanitize_filename(title);
    println!("\nFound Movie: {}", title);
    let output_path = format!("./{}.mp4", clean_title);
    println!("Downloading to {}...", output_path);

    if let Err(e) = download_stream(&stream_url, &output_path, &headers, config.fragments, 0, &None) {
        eprintln!("\nDownload failed: {}", e);
    } else {
        handle_subtitles(imdb_id, "", 0, &output_path, 0, &None, &sub_url, config);
        println!("\nDownload complete.");
    }
}

// ── Show Mode ────────────────────────────────────────────────────────────────

fn handle_show(imdb_id: &str, title: &str, eps_data: &Value, config: Arc<Config>) {
    if eps_data.is_object() && !eps_data.as_object().unwrap().is_empty() {
        println!("\nFound TV Show: {}", title);
        println!("Available Seasons:");

        let eps_obj = eps_data.as_object().unwrap();
        let mut seasons: Vec<String> = eps_obj.keys().cloned().collect();
        // Sort seasons naturally if numeric
        seasons.sort_by(|a, b| {
            let a_num = a.parse::<i32>().unwrap_or(0);
            let b_num = b.parse::<i32>().unwrap_or(0);
            a_num.cmp(&b_num)
        });

        for (i, s) in seasons.iter().enumerate() {
            let val = &eps_obj[s];
            let count = if val.is_array() {
                val.as_array().unwrap().len()
            } else {
                val.as_i64().unwrap_or(0) as usize
            };
            println!("  {}. Season {} ({} episodes)", i + 1, s, count);
        }

        println!("\nOptions:\n  1. Download one specific episode\n  2. Download one season\n  3. Download ALL episodes");
        print!("Choose an option (1-3): ");
        let _ = io::stdout().flush();

        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        let mode = input.trim().parse::<i32>().unwrap_or(0);
        let clean_title = sanitize_filename(title);

        if mode == 1 {
            print!("Enter Season Number: ");
            let _ = io::stdout().flush();
            let mut s_in = String::new();
            io::stdin().read_line(&mut s_in).unwrap();
            let season_num = s_in.trim();

            print!("Enter Episode Number: ");
            let _ = io::stdout().flush();
            let mut e_in = String::new();
            io::stdin().read_line(&mut e_in).unwrap();
            let ep_num = e_in.trim().parse::<usize>().unwrap_or(0);

            println!("\nDownloading S{}E{}...", season_num, ep_num);
            match fetch_ani_api(&format!("/download/show/{}/{}/{}", imdb_id, season_num, ep_num), &config) {
                Ok(ep_res) => {
                    let stream_url = ep_res.get("streamUrl").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let headers = ep_res.get("headers").cloned().unwrap_or(serde_json::json!({}));
                    let sub_url = ep_res.get("sub").and_then(|v| v.as_str()).unwrap_or("").to_string();

                    if !stream_url.is_empty() {
                        let base = format!("./{}-S{}-E{}", clean_title, season_num, ep_num);
                        let output_path = format!("{}.mp4", base);
                        if let Err(e) = download_stream(&stream_url, &output_path, &headers, config.fragments, 0, &None) {
                            eprintln!("Download failed: {}", e);
                        } else {
                            handle_subtitles(imdb_id, season_num, ep_num, &output_path, 0, &None, &sub_url, &config);
                            println!("\nDownload complete.");
                        }
                    } else {
                        eprintln!("No stream found via primary source.");
                    }
                }
                Err(e) => {
                    eprintln!("Primary source failed for that episode: {}", e);
                }
            }
        } else if mode == 2 {
            print!("Enter Season Number: ");
            let _ = io::stdout().flush();
            let mut s_in = String::new();
            io::stdin().read_line(&mut s_in).unwrap();
            let season_idx = s_in.trim().parse::<usize>().unwrap_or(0);

            if season_idx == 0 || season_idx > seasons.len() {
                eprintln!("Invalid season selection.");
                return;
            }
            let chosen_season = &seasons[season_idx - 1];

            let val = &eps_obj[chosen_season];
            let ep_count = if val.is_array() {
                val.as_array().unwrap().len()
            } else {
                val.as_i64().unwrap_or(0) as usize
            };

            let mut tasks = Vec::new();
            for ep in 1..=ep_count {
                tasks.push(Task {
                    season: chosen_season.clone(),
                    episode: ep,
                    base_dir: format!("./{}/Season_{}", clean_title, chosen_season),
                    file_name_base: format!("./{}/Season_{}/{}-S{}-E{}", clean_title, chosen_season, clean_title, chosen_season, ep),
                    imdb_id: imdb_id.to_string(),
                    sub_url: "".to_string(),
                    downloaded: false,
                    failed: false,
                    claimed_by: usize::MAX,
                });
            }

            let mut workers = Vec::new();
            for i in 0..config.threads {
                workers.push(WorkerStatus {
                    id: i + 1,
                    status: "Idle".to_string(),
                    progress: 0.0,
                    current_task: None,
                    last_output: "".to_string(),
                });
            }

            let manager = Arc::new(Mutex::new(DownloadManager {
                tasks,
                workers,
                is_bulk: false,
            }));

            println!("\nStarting bulk download of Season {} ({} episodes) with {} threads...", chosen_season, ep_count, config.threads);
            manager.lock().unwrap().is_bulk = true;
            for _ in 0..(config.threads * 2 + 2) {
                println!();
            }
            render(&manager);

            let mut thread_handles = Vec::new();
            for i in 0..config.threads {
                let m_clone = manager.clone();
                let c_clone = config.clone();
                thread_handles.push(std::thread::spawn(move || {
                    download_worker(i + 1, m_clone, c_clone);
                }));
            }

            for handle in thread_handles {
                let _ = handle.join();
            }

            let failed_count = manager.lock().unwrap().tasks.iter().filter(|t| t.failed).count();
            if failed_count > 0 {
                println!("\nNot all eps are downloaded and they need to run the command again");
            } else {
                println!("\nAll downloads completed.");
            }
        } else if mode == 3 {
            let mut tasks = Vec::new();
            for s in &seasons {
                let val = &eps_obj[s];
                let ep_count = if val.is_array() {
                    val.as_array().unwrap().len()
                } else {
                    val.as_i64().unwrap_or(0) as usize
                };

                for ep in 1..=ep_count {
                    tasks.push(Task {
                        season: s.clone(),
                        episode: ep,
                        base_dir: format!("./{}/Season_{}", clean_title, s),
                        file_name_base: format!("./{}/Season_{}/{}-S{}-E{}", clean_title, s, clean_title, s, ep),
                        imdb_id: imdb_id.to_string(),
                        sub_url: "".to_string(),
                        downloaded: false,
                        failed: false,
                        claimed_by: usize::MAX,
                    });
                }
            }

            let mut workers = Vec::new();
            for i in 0..config.threads {
                workers.push(WorkerStatus {
                    id: i + 1,
                    status: "Idle".to_string(),
                    progress: 0.0,
                    current_task: None,
                    last_output: "".to_string(),
                });
            }

            let manager = Arc::new(Mutex::new(DownloadManager {
                tasks,
                workers,
                is_bulk: false,
            }));

            println!("\nStarting bulk download ({} episodes) with {} threads...", manager.lock().unwrap().tasks.len(), config.threads);
            manager.lock().unwrap().is_bulk = true;
            for _ in 0..(config.threads * 2 + 2) {
                println!();
            }
            render(&manager);

            let mut thread_handles = Vec::new();
            for i in 0..config.threads {
                let m_clone = manager.clone();
                let c_clone = config.clone();
                thread_handles.push(std::thread::spawn(move || {
                    download_worker(i + 1, m_clone, c_clone);
                }));
            }

            for handle in thread_handles {
                let _ = handle.join();
            }

            let failed_count = manager.lock().unwrap().tasks.iter().filter(|t| t.failed).count();
            if failed_count > 0 {
                println!("\nNot all eps are downloaded and they need to run the command again");
            } else {
                println!("\nAll downloads completed.");
            }
        } else {
            eprintln!("Invalid option.");
        }
        return;
    }

    println!("\nFound TV Show: {}", title);
    println!("[Info] AniAPI did not return episode metadata. Downloading a single episode only.");
    
    print!("Enter Season Number: ");
    let _ = io::stdout().flush();
    let mut s_in = String::new();
    io::stdin().read_line(&mut s_in).unwrap();
    let season_idx = s_in.trim();

    print!("Enter Episode Number: ");
    let _ = io::stdout().flush();
    let mut e_in = String::new();
    io::stdin().read_line(&mut e_in).unwrap();
    let chosen_ep = e_in.trim().parse::<usize>().unwrap_or(0);

    let clean_title = sanitize_filename(title);

    match fetch_ani_api(&format!("/download/show/{}/{}/{}", imdb_id, season_idx, chosen_ep), &config) {
        Ok(ep_res) => {
            let stream_url = ep_res.get("streamUrl").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let headers = ep_res.get("headers").cloned().unwrap_or(serde_json::json!({}));
            let sub_url = ep_res.get("sub").and_then(|v| v.as_str()).unwrap_or("").to_string();

            if stream_url.is_empty() {
                eprintln!("No stream found for that episode.");
                return;
            }

            let base = format!("./{}-S{}-E{}", clean_title, season_idx, chosen_ep);
            let output_path = format!("{}.mp4", base);
            println!("\nDownloading S{}E{}...", season_idx, chosen_ep);

            if let Err(e) = download_stream(&stream_url, &output_path, &headers, config.fragments, 0, &None) {
                eprintln!("Download failed: {}", e);
            } else {
                handle_subtitles(imdb_id, season_idx, chosen_ep, &output_path, 0, &None, &sub_url, &config);
                println!("\nDownload complete.");
            }
        }
        Err(e) => {
            eprintln!("AniAPI episode download failed: {}", e);
        }
    }
}

// ── Dependency Check ──────────────────────────────────────────────────────────

fn check_dependencies() -> bool {
    let check_cmd = |cmd: &str| -> bool {
        Command::new("sh")
            .arg("-c")
            .arg(&format!("command -v {} > /dev/null 2>&1", cmd))
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };

    if !check_cmd("yt-dlp") {
        eprintln!("Missing required dependencies: yt-dlp");
        return false;
    }
    if !check_cmd("ffmpeg") {
        eprintln!("Missing required dependencies: ffmpeg");
        return false;
    }
    true
}

// ── Main ─────────────────────────────────────────────────────────────────────

fn main() {
    let args = Args::parse();

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

    if !check_dependencies() {
        std::process::exit(1);
    }

    let mut sub_imdb_id = String::new();
    if let Some(imdb) = args.imdb {
        sub_imdb_id = imdb;
    } else if args.imdb_id.len() >= 2 && &args.imdb_id[0..2] == "tt" {
        sub_imdb_id = args.imdb_id.clone();
    }

    let config = Arc::new(Config {
        threads: args.threads,
        fragments: args.concurrent_fragments,
        api_key,
        embed_subs: args.embed_subs,
        sub_lang: args.sub_lang,
        sub_imdb_id,
        base_url: args.base_url,
    });

    println!("Analyzing IMDB Media Signature...");

    let meta = fetch_imdb_metadata(&args.imdb_id, &config);
    let is_show = if let Some(ref mt) = meta.media_type {
        is_show_type(mt)
    } else if let Some(ref t) = meta.type_field {
        is_show_type(t)
    } else {
        false
    };

    println!("\nTitle: {} ({})", meta.title, if is_show { "show" } else { "movie" });

    if !is_show {
        handle_movie(&args.imdb_id, &meta.title, &config);
    } else {
        handle_show(&args.imdb_id, &meta.title, &meta.episodes.unwrap_or(serde_json::json!({})), config);
    }
}

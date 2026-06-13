use std::sync::{Arc, Mutex};
use std::fs;
use std::process::Command;
use serde_json::Value;

use crate::config::Config;
use crate::types::DownloadManager;
use crate::progress::render;
use crate::api::{fetch_url, fetch_ani_api};
use crate::ram::{SharedMemory, spawn_command_with_inherited_fds};

pub fn handle_subtitles(
    imdb_id: &str,
    season: &str,
    episode: usize,
    video_path: &str,
    worker_id: usize,
    manager: &Option<Arc<Mutex<DownloadManager>>>,
    direct_sub_url: &str,
    config: &Config,
) {
    if !config.embed_subs && !config.sub_only {
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

pub fn handle_subtitles_ram(
    imdb_id: &str,
    season: &str,
    episode: usize,
    shm_in: &SharedMemory,
    shm_out: &SharedMemory,
    worker_id: usize,
    manager: &Option<Arc<Mutex<DownloadManager>>>,
    direct_sub_url: &str,
    config: &Config,
) -> Result<(), String> {
    if !config.embed_subs && !config.sub_only {
        return Ok(());
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
            return Ok(());
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
                return Ok(());
            }
        };

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
            return Err("Failed to download subtitle".to_string());
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

    let temp_sub_path = format!("/tmp/imdb_sub_{}{}", std::process::id(), sub_ext);
    if fs::write(&temp_sub_path, sub_data).is_err() {
        log("[Subs] Failed to write temporary subtitle file.".to_string());
        return Err("Failed to write temporary subtitle".to_string());
    }

    log("[Subs] Embedding subtitle...".to_string());

    let mut lang = selected_sub.get("language").and_then(|v| v.as_str()).unwrap_or("eng").to_lowercase();
    if lang.len() > 3 {
        lang = lang[0..3].to_string();
    }

    let cmd = format!(
        "ffmpeg -y -i \"/dev/fd/{}\" -i \"{}\" -c copy -c:s mov_text -metadata:s:s:0 language={} \"/dev/fd/{}\" > /dev/null 2>&1",
        shm_in.fd, temp_sub_path, lang, shm_out.fd
    );

    let status = spawn_command_with_inherited_fds(&cmd, &[shm_in.fd, shm_out.fd], false)
        .and_then(|mut c| c.wait().map_err(|e| e.to_string()));

    let _ = fs::remove_file(&temp_sub_path);

    match status {
        Ok(s) if s.success() => {
            log("[Subs] Embedded successfully.".to_string());
            Ok(())
        }
        _ => {
            log("[Subs] ffmpeg failed.".to_string());
            Err("ffmpeg subtitle embedding failed".to_string())
        }
    }
}

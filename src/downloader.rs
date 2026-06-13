use std::sync::{Arc, Mutex};
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use regex::Regex;
use serde_json::Value;

use crate::config::Config;
use crate::types::{Task, WorkerStatus, DownloadManager};
use crate::progress::render;
use crate::api::{fetch_ani_api, sanitize_filename};
use crate::unmask::{is_masked_file, unmask_file};
use crate::subtitles::handle_subtitles;
use crate::ram::{spawn_command_with_inherited_fds, process_download_in_ram};

pub fn download_stream(
    m3u8_url: &str,
    output_path: &str,
    extra_headers: &Value,
    fragments: usize,
    worker_id: usize,
    manager_option: &Option<Arc<Mutex<DownloadManager>>>,
    shm_fd: Option<i32>,
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
        let mut child = if let Some(fd) = shm_fd {
            spawn_command_with_inherited_fds(&cmd_args, &[fd], true)
                .map_err(|e| format!("Failed to spawn yt-dlp: {}", e))?
        } else {
            Command::new("sh")
                .arg("-c")
                .arg(&cmd_args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("Failed to spawn yt-dlp: {}", e))?
        };

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

pub fn download_worker(worker_id: usize, manager_arc: Arc<Mutex<DownloadManager>>, config: Arc<Config>) {
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

        let output_path = format!("{}.mp4", task.file_name_base);

        if config.skip_existing && Path::new(&output_path).exists() {
            task.downloaded = true;
        } else {
            match fetch_ani_api(&format!("/download/show/{}/{}/{}", task.imdb_id, task.season, task.episode), &config) {
                Ok(ep_res) => {
                    let m3u8 = ep_res.get("streamUrl").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let headers = ep_res.get("headers").cloned().unwrap_or(serde_json::json!({}));
                    task.sub_url = ep_res.get("sub").and_then(|v| v.as_str()).unwrap_or("").to_string();

                    if config.use_ram_disk {
                        match process_download_in_ram(
                            &m3u8,
                            &output_path,
                            &headers,
                            config.fragments,
                            worker_id,
                            &Some(manager_arc.clone()),
                            &config,
                            &task,
                        ) {
                            Ok(_) => {
                                task.downloaded = true;
                            }
                            Err(e) => {
                                failed = true;
                                err_msg = e;
                            }
                        }
                    } else if config.sub_only {
                        if Path::new(&output_path).exists() {
                            println!("[Subs] Found file, embedding subtitles for S{}E{}...", task.season, task.episode);
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
                        } else {
                            eprintln!("Error: File is missing at {}", output_path);
                            failed = true;
                            err_msg = "File is missing".to_string();
                        }
                    } else if m3u8.is_empty() {
                        failed = true;
                    } else {
                        let _ = fs::create_dir_all(&task.base_dir);

                        match download_stream(&m3u8, &output_path, &headers, config.fragments, worker_id, &Some(manager_arc.clone()), None) {
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

pub fn handle_movie(imdb_id: &str, title: &str, config: &Config) {
    let clean_title = sanitize_filename(title);
    let output_path = format!("./{}.mp4", clean_title);

    if config.skip_existing && Path::new(&output_path).exists() {
        println!("File already exists at {}, skipping download.", output_path);
        return;
    }

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

    println!("\nFound Movie: {}", title);

    if config.use_ram_disk {
        let dummy_task = Task {
            season: "".to_string(),
            episode: 0,
            base_dir: ".".to_string(),
            file_name_base: clean_title.clone(),
            imdb_id: imdb_id.to_string(),
            sub_url,
            downloaded: false,
            failed: false,
            failure_printed: false,
            claimed_by: 0,
        };
        if let Err(e) = process_download_in_ram(&stream_url, &output_path, &headers, config.fragments, 0, &None, config, &dummy_task) {
            eprintln!("\nDownload failed: {}", e);
        } else {
            println!("\nDownload complete.");
        }
    } else {
        if config.sub_only {
            if Path::new(&output_path).exists() {
                println!("[Subs] Found file, embedding subtitles...");
                handle_subtitles(imdb_id, "", 0, &output_path, 0, &None, &sub_url, config);
            } else {
                eprintln!("Error: File is missing at {}", output_path);
            }
            return;
        }

        println!("Downloading to {}...", output_path);

        if let Err(e) = download_stream(&stream_url, &output_path, &headers, config.fragments, 0, &None, None) {
            eprintln!("\nDownload failed: {}", e);
        } else {
            handle_subtitles(imdb_id, "", 0, &output_path, 0, &None, &sub_url, config);
            println!("\nDownload complete.");
        }
    }
}

pub fn handle_show(imdb_id: &str, title: &str, eps_data: &Value, config: Arc<Config>) {
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

            let base = format!("./{}-S{}-E{}", clean_title, season_num, ep_num);
            let output_path = format!("{}.mp4", base);

            if config.skip_existing && Path::new(&output_path).exists() {
                println!("File already exists at {}, skipping download.", output_path);
                return;
            }

            match fetch_ani_api(&format!("/download/show/{}/{}/{}", imdb_id, season_num, ep_num), &config) {
                Ok(ep_res) => {
                    let stream_url = ep_res.get("streamUrl").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let headers = ep_res.get("headers").cloned().unwrap_or(serde_json::json!({}));
                    let sub_url = ep_res.get("sub").and_then(|v| v.as_str()).unwrap_or("").to_string();

                    if !stream_url.is_empty() {
                        if config.use_ram_disk {
                            let dummy_task = Task {
                                season: season_num.to_string(),
                                episode: ep_num,
                                base_dir: ".".to_string(),
                                file_name_base: clean_title.clone(),
                                imdb_id: imdb_id.to_string(),
                                sub_url: sub_url.clone(),
                                downloaded: false,
                                failed: false,
                                failure_printed: false,
                                claimed_by: 0,
                            };
                            if let Err(e) = process_download_in_ram(&stream_url, &output_path, &headers, config.fragments, 0, &None, &config, &dummy_task) {
                                eprintln!("Download failed: {}", e);
                            } else {
                                println!("\nDownload complete.");
                            }
                        } else if config.sub_only {
                            if Path::new(&output_path).exists() {
                                println!("[Subs] Found file, embedding subtitles for S{}E{}...", season_num, ep_num);
                                handle_subtitles(imdb_id, season_num, ep_num, &output_path, 0, &None, &sub_url, &config);
                            } else {
                                eprintln!("Error: File is missing at {}", output_path);
                            }
                        } else {
                            println!("\nDownloading S{}E{}...", season_num, ep_num);
                            if let Err(e) = download_stream(&stream_url, &output_path, &headers, config.fragments, 0, &None, None) {
                                eprintln!("Download failed: {}", e);
                            } else {
                                handle_subtitles(imdb_id, season_num, ep_num, &output_path, 0, &None, &sub_url, &config);
                                println!("\nDownload complete.");
                            }
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
                    failure_printed: false,
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

            if config.sub_only {
                println!("\nChecking and embedding subtitles for Season {} ({} episodes)...", chosen_season, ep_count);
                manager.lock().unwrap().is_bulk = false;
            } else {
                println!("\nStarting bulk download of Season {} ({} episodes) with {} threads...", chosen_season, ep_count, config.threads);
                manager.lock().unwrap().is_bulk = true;
                for _ in 0..(config.threads * 2 + 2) {
                    println!();
                }
                render(&manager);
            }

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
                if config.sub_only {
                    println!("\nSubtitles processing finished. {} episodes had errors or missing files.", failed_count);
                } else {
                    println!("\nNot all eps are downloaded and they need to run the command again");
                }
            } else {
                if config.sub_only {
                    println!("\nAll subtitles processed successfully.");
                } else {
                    println!("\nAll downloads completed.");
                }
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
                        failure_printed: false,
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

            if config.sub_only {
                println!("\nChecking and embedding subtitles for all episodes ({} episodes)...", manager.lock().unwrap().tasks.len());
                manager.lock().unwrap().is_bulk = false;
            } else {
                println!("\nStarting bulk download ({} episodes) with {} threads...", manager.lock().unwrap().tasks.len(), config.threads);
                manager.lock().unwrap().is_bulk = true;
                for _ in 0..(config.threads * 2 + 2) {
                    println!();
                }
                render(&manager);
            }

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
                if config.sub_only {
                    println!("\nSubtitles processing finished. {} episodes had errors or missing files.", failed_count);
                } else {
                    println!("\nNot all eps are downloaded and they need to run the command again");
                }
            } else {
                if config.sub_only {
                    println!("\nAll subtitles processed successfully.");
                } else {
                    println!("\nAll downloads completed.");
                }
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

    let base = format!("./{}-S{}-E{}", clean_title, season_idx, chosen_ep);
    let output_path = format!("{}.mp4", base);

    if config.skip_existing && Path::new(&output_path).exists() {
        println!("File already exists at {}, skipping download.", output_path);
        return;
    }

    match fetch_ani_api(&format!("/download/show/{}/{}/{}", imdb_id, season_idx, chosen_ep), &config) {
        Ok(ep_res) => {
            let stream_url = ep_res.get("streamUrl").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let headers = ep_res.get("headers").cloned().unwrap_or(serde_json::json!({}));
            let sub_url = ep_res.get("sub").and_then(|v| v.as_str()).unwrap_or("").to_string();

            if stream_url.is_empty() {
                eprintln!("No stream found for that episode.");
                return;
            }

            if config.use_ram_disk {
                let dummy_task = Task {
                    season: season_idx.to_string(),
                    episode: chosen_ep,
                    base_dir: ".".to_string(),
                    file_name_base: clean_title.clone(),
                    imdb_id: imdb_id.to_string(),
                    sub_url,
                    downloaded: false,
                    failed: false,
                    failure_printed: false,
                    claimed_by: 0,
                };
                if let Err(e) = process_download_in_ram(&stream_url, &output_path, &headers, config.fragments, 0, &None, &config, &dummy_task) {
                    eprintln!("Download failed: {}", e);
                } else {
                    println!("\nDownload complete.");
                }
            } else {
                if config.sub_only {
                    if Path::new(&output_path).exists() {
                        println!("[Subs] Found file, embedding subtitles for S{}E{}...", season_idx, chosen_ep);
                        handle_subtitles(imdb_id, season_idx, chosen_ep, &output_path, 0, &None, &sub_url, &config);
                    } else {
                        eprintln!("Error: File is missing at {}", output_path);
                    }
                    return;
                }

                println!("\nDownloading S{}E{}...", season_idx, chosen_ep);

                if let Err(e) = download_stream(&stream_url, &output_path, &headers, config.fragments, 0, &None, None) {
                    eprintln!("Download failed: {}", e);
                } else {
                    handle_subtitles(imdb_id, season_idx, chosen_ep, &output_path, 0, &None, &sub_url, &config);
                    println!("\nDownload complete.");
                }
            }
        }
        Err(e) => {
            eprintln!("AniAPI episode download failed: {}", e);
        }
    }
}

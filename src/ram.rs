use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::process::Command;

static MOUNTED_RAM_DISK: OnceLock<Mutex<Option<String>>> = OnceLock::new();

pub fn cleanup_ram_disk_global() {
    if let Some(cell) = MOUNTED_RAM_DISK.get() {
        if let Ok(mut opt) = cell.lock() {
            if let Some(mount_path) = opt.take() {
                println!("\nCleaning up RAM disk at {}...", mount_path);
                #[cfg(target_os = "macos")]
                {
                    let _ = Command::new("diskutil")
                        .arg("eject")
                        .arg(&mount_path)
                        .status();
                }
                #[cfg(target_os = "linux")]
                {
                    let _ = Command::new("umount")
                        .arg(&mount_path)
                        .status();
                    let _ = std::fs::remove_dir(&mount_path);
                }
            }
        }
    }
}

pub struct RamDiskGuard {
    pub mount_path: String,
}

impl Drop for RamDiskGuard {
    fn drop(&mut self) {
        cleanup_ram_disk_global();
    }
}


#[cfg(target_os = "macos")]
fn setup_macos_ram_disk(hash: &str) -> Result<String, String> {
    let vol_name = format!("tmp_{}", hash);
    let output = Command::new("hdid")
        .arg("-nomount")
        .arg("ram://67108864")
        .output()
        .map_err(|e| format!("Failed to run hdid: {}", e))?;
    
    if !output.status.success() {
        return Err(format!("hdid command failed: {}", String::from_utf8_lossy(&output.stderr)));
    }
    
    let dev_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if dev_path.is_empty() {
        return Err("hdid returned empty device path".to_string());
    }
    
    let erase_status = Command::new("diskutil")
        .arg("erasevolume")
        .arg("HFS+")
        .arg(&vol_name)
        .arg(&dev_path)
        .status()
        .map_err(|e| format!("Failed to run diskutil erasevolume: {}", e))?;
        
    if !erase_status.success() {
        return Err(format!("diskutil erasevolume failed for device {}", dev_path));
    }
    
    Ok(format!("/Volumes/{}", vol_name))
}

#[cfg(target_os = "linux")]
fn setup_linux_ram_disk(hash: &str) -> Result<String, String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let mount_path = std::path::Path::new(&home).join(format!("tmp_{}", hash));
    let mount_path_str = mount_path.to_string_lossy().to_string();
    
    let mkdir_status = Command::new("mkdir")
        .arg("-p")
        .arg(&mount_path_str)
        .status()
        .map_err(|e| format!("Failed to run mkdir: {}", e))?;
        
    if !mkdir_status.success() {
        return Err(format!("mkdir -p {} failed", mount_path_str));
    }
    
    let mount_status = Command::new("mount")
        .arg("-t")
        .arg("tmpfs")
        .arg("-o")
        .arg("size=8G")
        .arg("tmpfs")
        .arg(&mount_path_str)
        .status()
        .map_err(|e| format!("Failed to run mount: {}", e))?;
        
    if !mount_status.success() {
        return Err(format!("mount -t tmpfs failed for {}", mount_path_str));
    }
    
    Ok(mount_path_str)
}

pub fn setup_ram_disk(id: &str) -> Result<RamDiskGuard, String> {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(id.as_bytes());
    let hash = format!("{:x}", hasher.finalize());
    
    let mount_path = {
        #[cfg(target_os = "macos")]
        {
            setup_macos_ram_disk(&hash)?
        }
        #[cfg(target_os = "linux")]
        {
            setup_linux_ram_disk(&hash)?
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            return Err("Unsupported operating system for RAM disk.".to_string());
        }
    };

    if let Some(mut cell) = MOUNTED_RAM_DISK.get_or_init(|| Mutex::new(None)).lock().ok() {
        *cell = Some(mount_path.clone());
    }

    Ok(RamDiskGuard { mount_path })
}

pub fn process_download_in_ram(
    m3u8_url: &str,
    output_path: &str,
    extra_headers: &serde_json::Value,
    fragments: usize,
    worker_id: usize,
    manager_arc: &Option<Arc<Mutex<crate::types::DownloadManager>>>,
    config: &crate::config::Config,
    task: &crate::types::Task,
) -> Result<(), String> {
    let log_progress = |status: &str, out: &str| {
        if let Some(m_arc) = manager_arc {
            let mut m = m_arc.lock().unwrap();
            for w in &mut m.workers {
                if w.id == worker_id {
                    w.status = status.to_string();
                    w.last_output = out.to_string();
                    break;
                }
            }
            drop(m);
            crate::progress::render(m_arc);
        } else {
            println!("{}: {}", status, out);
        }
    };

    let ram_disk_dir = config.ram_disk_path.as_ref().ok_or("RAM disk path is not configured")?;
    let filename = Path::new(output_path)
        .file_name()
        .ok_or_else(|| "Invalid output path".to_string())?
        .to_str()
        .ok_or_else(|| "Invalid unicode in output path".to_string())?;
    
    let ram_output_path = Path::new(ram_disk_dir).join(filename);
    let ram_output_path_str = ram_output_path.to_string_lossy().to_string();

    log_progress("Downloading", "Starting download to RAM...");

    crate::downloader::download_stream(
        m3u8_url,
        &ram_output_path_str,
        extra_headers,
        fragments,
        worker_id,
        manager_arc,
    )?;

    let mut current_ram_path = ram_output_path_str;

    if crate::unmask::is_masked_file(&current_ram_path) {
        log_progress("Unmasking", "Removing image wrapper in RAM...");
        let unmasked_ram_path = format!("{}.unmasked.mp4", current_ram_path);
        if crate::unmask::unmask_file(&current_ram_path, &unmasked_ram_path) {
            let _ = fs::remove_file(&current_ram_path);
            current_ram_path = unmasked_ram_path;
        } else {
            return Err("Failed to unmask file".to_string());
        }
    }

    if config.embed_subs || config.sub_only {
        crate::subtitles::handle_subtitles(
            &task.imdb_id,
            &task.season,
            task.episode,
            &current_ram_path,
            worker_id,
            manager_arc,
            &task.sub_url,
            config,
        );
    }

    log_progress("Saving", "Writing final file to disk...");
    if let Some(parent) = Path::new(output_path).parent() {
        let _ = fs::create_dir_all(parent);
    }
    
    fs::copy(&current_ram_path, output_path)
        .map_err(|e| format!("Failed to copy file to disk: {}", e))?;

    let _ = fs::remove_file(&current_ram_path);

    log_progress("Done", "Completed download successfully.");
    Ok(())
}

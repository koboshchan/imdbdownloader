use std::ffi::CString;
use std::os::unix::io::FromRawFd;
use std::fs::{self, File};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::process::{Command, Stdio};

pub struct SharedMemory {
    pub name: String,
    pub fd: i32,
}

impl SharedMemory {
    pub fn new(suffix: &str) -> Result<Self, String> {
        let name = format!("/imdb_shm_{}_{}", std::process::id(), suffix);
        let c_name = CString::new(name.clone()).map_err(|e| e.to_string())?;
        
        let fd = unsafe {
            libc::shm_open(
                c_name.as_ptr(),
                libc::O_CREAT | libc::O_RDWR | libc::O_TRUNC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(format!("shm_open failed: {}", std::io::Error::last_os_error()));
        }
        Ok(Self { name, fd })
    }

    pub fn path(&self) -> String {
        format!("/dev/fd/{}", self.fd)
    }

    pub fn rewind(&self) -> Result<(), String> {
        let res = unsafe { libc::lseek(self.fd, 0, libc::SEEK_SET) };
        if res < 0 {
            return Err(format!("lseek failed: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
            if let Ok(c_name) = CString::new(self.name.clone()) {
                libc::shm_unlink(c_name.as_ptr());
            }
        }
    }
}

pub fn spawn_command_with_inherited_fds(
    cmd_args: &str,
    fds: &[i32],
    piped_output: bool,
) -> Result<std::process::Child, String> {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(cmd_args);
    
    if piped_output {
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
    } else {
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
    }

    let fds_vec = fds.to_vec();
    unsafe {
        cmd.pre_exec(move || {
            for &fd in &fds_vec {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags != -1 {
                    libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
                }
            }
            Ok(())
        });
    }

    cmd.spawn().map_err(|e| e.to_string())
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

    let shm_download = SharedMemory::new("download")?;
    log_progress("Downloading", "Starting download to RAM...");

    crate::downloader::download_stream(
        m3u8_url,
        &shm_download.path(),
        extra_headers,
        fragments,
        worker_id,
        manager_arc,
        Some(shm_download.fd),
    )?;

    let mut shm_current = shm_download;
    shm_current.rewind()?;

    if crate::unmask::is_masked_file(&shm_current.path()) {
        log_progress("Unmasking", "Removing image wrapper in RAM...");
        let shm_unmask = SharedMemory::new("unmasked")?;
        crate::unmask::unmask_file_ram(&shm_current, &shm_unmask)?;
        shm_current = shm_unmask;
    }

    shm_current.rewind()?;
    let mut active_shm = shm_current;

    if config.embed_subs || config.sub_only {
        let shm_sub = SharedMemory::new("subbed")?;
        if crate::subtitles::handle_subtitles_ram(
            &task.imdb_id,
            &task.season,
            task.episode,
            &active_shm,
            &shm_sub,
            worker_id,
            manager_arc,
            &task.sub_url,
            config,
        ).is_ok() {
            active_shm = shm_sub;
        }
    }

    log_progress("Saving", "Writing final file to disk...");
    active_shm.rewind()?;

    let mut shm_file = unsafe { File::from_raw_fd(libc::dup(active_shm.fd)) };
    
    if let Some(parent) = Path::new(output_path).parent() {
        let _ = fs::create_dir_all(parent);
    }
    
    let mut disk_file = File::create(output_path).map_err(|e| format!("Failed to create output file: {}", e))?;
    std::io::copy(&mut shm_file, &mut disk_file).map_err(|e| format!("Failed to copy file to disk: {}", e))?;

    log_progress("Done", "Completed download successfully.");
    Ok(())
}

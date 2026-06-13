use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::os::unix::io::FromRawFd;
use crate::ram::{SharedMemory, spawn_command_with_inherited_fds};

pub fn is_masked_file<P: AsRef<Path>>(path: P) -> bool {
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

pub fn unmask_file<P: AsRef<Path>>(input_path: P, output_path: P) -> bool {
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

    let segment_start = 0;
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

pub fn unmask_file_ram(shm_in: &SharedMemory, shm_out: &SharedMemory) -> Result<(), String> {
    shm_in.rewind()?;
    let mut data = Vec::new();
    let mut f_in = unsafe { File::from_raw_fd(libc::dup(shm_in.fd)) };
    f_in.read_to_end(&mut data).map_err(|e| e.to_string())?;

    let tmp_shm = SharedMemory::new("unmask_tmp")?;

    let png_magic = b"\x89PNG\r\n\x1a\n";
    let jpg_magic = b"\xFF\xD8\xFF";

    let mut all_pos = Vec::new();

    let mut i = 0;
    while i + 8 <= data.len() {
        if &data[i..i+8] == png_magic {
            all_pos.push((i, 8));
        }
        i += 1;
    }

    let mut i = 0;
    while i + 3 <= data.len() {
        if &data[i..i+3] == jpg_magic {
            all_pos.push((i, 3));
        }
        i += 1;
    }

    all_pos.sort_by_key(|k| k.0);

    let segment_start = 0;
    let mut written = 0;

    let mut f_tmp = unsafe { File::from_raw_fd(libc::dup(tmp_shm.fd)) };

    for (magic_pos, magic_len) in all_pos {
        if magic_pos > segment_start {
            let _ = f_tmp.write_all(&data[segment_start..magic_pos]);
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
            let _ = f_tmp.write_all(&data[v_start..]);
            written += 1;
        }
        break;
    }

    let _ = f_tmp.flush();
    drop(f_tmp);

    if written == 0 {
        shm_in.rewind()?;
        shm_out.rewind()?;
        let mut f_out = unsafe { File::from_raw_fd(libc::dup(shm_out.fd)) };
        shm_in.rewind()?;
        let mut f_in_orig = unsafe { File::from_raw_fd(libc::dup(shm_in.fd)) };
        std::io::copy(&mut f_in_orig, &mut f_out).map_err(|e| e.to_string())?;
        return Ok(());
    }

    let cmd = format!(
        "ffmpeg -y -i \"/dev/fd/{}\" -c copy -bsf:a aac_adtstoasc -movflags +faststart \"/dev/fd/{}\"",
        tmp_shm.fd,
        shm_out.fd
    );

    let mut child = spawn_command_with_inherited_fds(&cmd, &[tmp_shm.fd, shm_out.fd], false)?;
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("ffmpeg remux failed".to_string());
    }

    Ok(())
}

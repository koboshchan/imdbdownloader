use std::sync::{Arc, Mutex};
use std::io::{self, Write};
use crate::types::DownloadManager;

pub fn get_terminal_width() -> usize {
    if let Some((w, _h)) = term_size::dimensions() {
        w
    } else {
        80
    }
}

pub fn render(manager_arc: &Arc<Mutex<DownloadManager>>) {
    let mut manager = manager_arc.lock().unwrap();
    if !manager.is_bulk {
        return;
    }

    let mut completed = 0;
    let mut failed = 0;
    let mut failed_to_print = Vec::new();
    for t in &mut manager.tasks {
        if t.downloaded {
            completed += 1;
        }
        if t.failed {
            failed += 1;
            if !t.failure_printed {
                t.failure_printed = true;
                failed_to_print.push(format!("S{}E{} Failed to download.", t.season, t.episode));
            }
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

    // Print failed messages
    for msg in &failed_to_print {
        print!("\x1b[K{}\n", msg);
    }

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

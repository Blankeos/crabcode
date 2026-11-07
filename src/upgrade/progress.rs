use std::io::{self, IsTerminal, Write};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;

pub(super) fn run(work: impl FnOnce() -> Result<()>) -> Result<()> {
    let interactive =
        io::stderr().is_terminal() && std::env::var("TERM").is_ok_and(|term| term != "dumb");
    if !interactive {
        eprintln!("Working...");
        return work();
    }

    let color = std::env::var_os("NO_COLOR").is_none();
    std::thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel();
        // Closing the channel on unwind also stops and joins the renderer.
        let stop = StopOnDrop(sender);
        let renderer = scope.spawn(move || {
            let mut tick = 0;
            loop {
                let columns = ratatui::crossterm::terminal::size()
                    .map(|(columns, _)| usize::from(columns))
                    .unwrap_or(80);
                let mut stderr = io::stderr().lock();
                let _ = write!(stderr, "\r\x1b[2K{}", working_row(columns, tick, color));
                let _ = stderr.flush();
                drop(stderr);
                match receiver.recv_timeout(Duration::from_millis(120)) {
                    Err(mpsc::RecvTimeoutError::Timeout) => tick += 1,
                    _ => break,
                }
            }
            let _ = write!(io::stderr(), "\r\x1b[2K");
            let _ = io::stderr().flush();
        });
        let result = work();
        drop(stop);
        let _ = renderer.join();
        result
    })
}

struct StopOnDrop(mpsc::Sender<()>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

fn working_row(columns: usize, tick: usize, color: bool) -> String {
    if columns < 12 {
        return "Working".chars().take(columns.saturating_sub(1)).collect();
    }
    let width = columns.saturating_sub(10).min(50);
    let start = tick % width;
    let bar: String = (0..width)
        .map(|index| if index == start { '█' } else { '.' })
        .collect();
    if color {
        format!("\x1b[33m{bar}\x1b[0m Working")
    } else {
        format!("{bar} Working")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_progress_has_no_fabricated_percentage() {
        let row = working_row(80, 17, false);
        assert_eq!(row.chars().count(), 58);
        assert_eq!(row.matches('█').count(), 1);
        assert!(row.ends_with(" Working"));
        assert!(!row.contains('%'));
        assert!(!row.contains('\x1b'));
        assert_ne!(row, working_row(80, 18, false));
    }

    #[test]
    fn working_progress_fits_narrow_terminals() {
        for columns in 0..100 {
            let row = working_row(columns, usize::MAX, false);
            assert!(row.chars().count() <= columns.saturating_sub(1));
        }
    }

    #[test]
    fn color_only_wraps_the_bar() {
        let row = working_row(80, 0, true);
        assert!(row.starts_with("\x1b[33m█"));
        assert!(row.ends_with("\x1b[0m Working"));
    }
}

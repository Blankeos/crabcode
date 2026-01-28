use anyhow::Result;
use chrono::Local;
use std::fs::OpenOptions;
use std::io::Write;

#[allow(unused_must_use)]
pub fn log(message: &str) -> Result<()> {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
    let log_line = format!("[{}] {}\n", timestamp, message);

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open("app.log")?;

    file.write_all(log_line.as_bytes())?;
    Ok(())
}

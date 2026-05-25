use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

const PASTED_IMAGE_PREFIX: &str = "crabcode-clipboard-";
const PASTED_IMAGE_SUFFIX: &str = ".png";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageCategory {
    PastedImages,
    DataDb,
    ModelsDevCache,
}

#[derive(Debug, Clone)]
pub struct StorageRow {
    pub category: StorageCategory,
    pub label: String,
    pub detail: String,
    pub bytes: u64,
    pub item_count: usize,
    pub open_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct StorageReport {
    pub rows: Vec<StorageRow>,
    pub total_bytes: u64,
    pub checked_at: SystemTime,
}

pub fn collect_storage_report() -> StorageReport {
    let rows = vec![
        collect_pasted_images_in_dir(&std::env::temp_dir()),
        collect_file_row(
            StorageCategory::DataDb,
            "Data.db",
            "sessions, preferences, prompt history",
            crate::persistence::get_data_dir().join("data.db"),
        ),
        collect_file_row(
            StorageCategory::ModelsDevCache,
            "Models.dev Cache",
            "models_dev_cache.json",
            crate::persistence::get_cache_dir().join("models_dev_cache.json"),
        ),
    ];
    let total_bytes = rows.iter().map(|row| row.bytes).sum();

    StorageReport {
        rows,
        total_bytes,
        checked_at: SystemTime::now(),
    }
}

fn collect_pasted_images_in_dir(dir: &Path) -> StorageRow {
    let mut bytes = 0u64;
    let mut item_count = 0usize;

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_pasted_image_file(&path) {
                continue;
            }
            if let Ok(metadata) = entry.metadata() {
                if metadata.is_file() {
                    bytes = bytes.saturating_add(metadata.len());
                    item_count = item_count.saturating_add(1);
                }
            }
        }
    }

    StorageRow {
        category: StorageCategory::PastedImages,
        label: "Pasted Images".to_string(),
        detail: format!(
            "{} PNG {}",
            item_count,
            if item_count == 1 { "file" } else { "files" }
        ),
        bytes,
        item_count,
        open_path: dir.is_dir().then(|| dir.to_path_buf()),
    }
}

fn collect_file_row(
    category: StorageCategory,
    label: &str,
    detail: &str,
    path: PathBuf,
) -> StorageRow {
    let bytes = path
        .metadata()
        .ok()
        .filter(|metadata| metadata.is_file())
        .map(|metadata| metadata.len())
        .unwrap_or(0);

    StorageRow {
        category,
        label: label.to_string(),
        detail: detail.to_string(),
        bytes,
        item_count: usize::from(bytes > 0),
        open_path: path.parent().map(Path::to_path_buf),
    }
}

fn is_pasted_image_file(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    file_name.starts_with(PASTED_IMAGE_PREFIX) && file_name.ends_with(PASTED_IMAGE_SUFFIX)
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let bytes_f = bytes as f64;
    if bytes == 0 {
        "0 B".to_string()
    } else if bytes_f < KB {
        format!("{} B", bytes)
    } else if bytes_f < MB {
        format!("{:.1} KB", bytes_f / KB)
    } else if bytes_f < GB {
        format!("{:.1} MB", bytes_f / MB)
    } else {
        format!("{:.2} GB", bytes_f / GB)
    }
}

pub fn open_folder(path: &Path) -> Result<()> {
    if !path.is_dir() {
        return Err(anyhow::anyhow!("folder does not exist: {}", path.display()));
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(path)
            .spawn()
            .with_context(|| format!("failed to open {}", path.display()))?;
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        Command::new("explorer")
            .arg(path)
            .spawn()
            .with_context(|| format!("failed to open {}", path.display()))?;
        return Ok(());
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        Command::new("xdg-open")
            .arg(path)
            .spawn()
            .with_context(|| format!("failed to open {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_uses_readable_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(2 * 1024 * 1024), "2.0 MB");
    }

    #[test]
    fn pasted_images_scan_counts_matching_png_files_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("crabcode-clipboard-a.png"), [1u8; 4]).unwrap();
        std::fs::write(dir.path().join("crabcode-clipboard-b.png"), [1u8; 6]).unwrap();
        std::fs::write(dir.path().join("crabcode-clipboard-c.jpg"), [1u8; 8]).unwrap();
        std::fs::write(dir.path().join("other.png"), [1u8; 10]).unwrap();

        let row = collect_pasted_images_in_dir(dir.path());

        assert_eq!(row.item_count, 2);
        assert_eq!(row.bytes, 10);
        assert_eq!(row.open_path.as_deref(), Some(dir.path()));
    }
}

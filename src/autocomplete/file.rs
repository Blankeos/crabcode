use std::fs;
use std::path::PathBuf;

pub struct FileAuto;

impl FileAuto {
    pub fn new() -> Self {
        Self
    }

    pub fn get_suggestions(&self, input: &str) -> Vec<String> {
        if input.is_empty() {
            return Vec::new();
        }

        let path = PathBuf::from(input);
        let parent_dir = if input.ends_with('/') || path.has_root() {
            path.clone()
        } else {
            match path.parent() {
                Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
                _ => PathBuf::from("."),
            }
        };

        let prefix = if input.ends_with('/') || path.has_root() {
            String::new()
        } else {
            match path.file_name() {
                Some(name) => name.to_string_lossy().to_string(),
                None => String::new(),
            }
        };

        if let Ok(entries) = fs::read_dir(&parent_dir) {
            entries
                .filter_map(|entry| entry.ok())
                .map(|entry| {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if entry.path().is_dir() {
                        format!("{}/", name)
                    } else {
                        name
                    }
                })
                .filter(|name| name.to_lowercase().starts_with(&prefix.to_lowercase()))
                .collect()
        } else {
            Vec::new()
        }
    }

    pub fn expand_path(&self, input: &str) -> Option<String> {
        if input.is_empty() {
            return None;
        }

        let suggestions = self.get_suggestions(input);
        if suggestions.len() == 1 {
            let suggestion = &suggestions[0];
            let path = PathBuf::from(input);
            let parent_dir = if input.ends_with('/') || path.has_root() {
                path
            } else {
                match path.parent() {
                    Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
                    _ => PathBuf::from("."),
                }
            };

            let full_path = if parent_dir.as_os_str().is_empty() {
                suggestion.clone()
            } else {
                format!("{}/{}", parent_dir.display(), suggestion)
            };

            Some(full_path)
        } else {
            None
        }
    }
}

impl Default for FileAuto {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_auto_creation() {
        let _auto = FileAuto::new();
    }

    #[test]
    fn test_file_auto_default() {
        let _auto = FileAuto;
    }

    #[test]
    fn test_get_suggestions_empty() {
        let auto = FileAuto::new();
        let suggestions = auto.get_suggestions("");
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_get_suggestions_no_match() {
        let auto = FileAuto::new();
        let suggestions = auto.get_suggestions("xyz123abc");
        assert!(suggestions.is_empty());
    }
}

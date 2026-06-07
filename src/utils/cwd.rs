use anyhow::{anyhow, Context, Result};
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

pub fn current_dir() -> Result<PathBuf> {
    let resolved = resolve_current_dir(
        std::env::current_dir(),
        std::env::var_os("PWD"),
        fallback_dir(),
    )?;

    if resolved.should_chdir {
        std::env::set_current_dir(&resolved.path).with_context(|| {
            format!(
                "Failed to recover from unavailable current directory by changing to {}",
                resolved.path.display()
            )
        })?;
    }

    if let Some(warning) = resolved.warning {
        crate::startup_diag!("Warning: {}", warning);
    }

    Ok(resolved.path)
}

pub fn current_dir_or_dot() -> PathBuf {
    current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn fallback_dir() -> Option<PathBuf> {
    dirs::home_dir().filter(|path| path.is_dir()).or_else(|| {
        let temp_dir = std::env::temp_dir();
        temp_dir.is_dir().then_some(temp_dir)
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CurrentDirResolution {
    path: PathBuf,
    warning: Option<String>,
    should_chdir: bool,
}

fn resolve_current_dir(
    getcwd: io::Result<PathBuf>,
    pwd: Option<OsString>,
    fallback: Option<PathBuf>,
) -> Result<CurrentDirResolution> {
    match getcwd {
        Ok(path) => Ok(CurrentDirResolution {
            path,
            warning: None,
            should_chdir: false,
        }),
        Err(err) => {
            if let Some(raw_pwd) = pwd {
                if !raw_pwd.is_empty() {
                    let pwd = PathBuf::from(raw_pwd);
                    if pwd.is_dir() {
                        return Ok(CurrentDirResolution {
                            warning: Some(format!(
                                "Recovered from unavailable current directory ({err}) by changing to PWD: {}",
                                pwd.display()
                            )),
                            path: pwd,
                            should_chdir: true,
                        });
                    }

                    if let Some(fallback) = fallback {
                        return Ok(CurrentDirResolution {
                            warning: Some(format!(
                                "The previous current directory is unavailable ({err}), and PWD points to a directory that does not exist or cannot be accessed: {}. Continuing from {}.",
                                pwd.display(),
                                fallback.display()
                            )),
                            path: fallback,
                            should_chdir: true,
                        });
                    }

                    return Err(anyhow!(
                        "Failed to determine current directory. The process current directory is unavailable ({err}), PWD points to a directory that does not exist or cannot be accessed: {}, and no fallback directory was available. Run `cd <existing-project-directory>` and start crabcode again.",
                        pwd.display()
                    ));
                }
            }

            if let Some(fallback) = fallback {
                return Ok(CurrentDirResolution {
                    warning: Some(format!(
                        "The previous current directory is unavailable ({err}). Continuing from {}.",
                        fallback.display()
                    )),
                    path: fallback,
                    should_chdir: true,
                });
            }

            Err(anyhow!(
                "Failed to determine current directory. The process current directory is unavailable ({err}), and no fallback directory was available. Run `cd <existing-project-directory>` and start crabcode again."
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_current_dir;
    use std::ffi::OsString;
    use std::io;

    #[test]
    fn current_dir_falls_back_to_valid_pwd() {
        let pwd = std::env::current_dir().unwrap();
        let cwd = resolve_current_dir(
            Err(io::Error::new(io::ErrorKind::NotFound, "missing cwd")),
            Some(OsString::from(&pwd)),
            None,
        )
        .unwrap();

        assert_eq!(
            cwd,
            super::CurrentDirResolution {
                path: pwd,
                warning: Some(format!(
                    "Recovered from unavailable current directory (missing cwd) by changing to PWD: {}",
                    std::env::current_dir().unwrap().display()
                )),
                should_chdir: true,
            }
        );
    }

    #[test]
    fn current_dir_recovers_from_deleted_pwd_with_fallback() {
        let missing =
            std::env::temp_dir().join(format!("crabcode-missing-cwd-{}", std::process::id()));
        let fallback = std::env::temp_dir();
        let cwd = resolve_current_dir(
            Err(io::Error::new(io::ErrorKind::NotFound, "missing cwd")),
            Some(OsString::from(&missing)),
            Some(fallback.clone()),
        )
        .unwrap();

        assert_eq!(cwd.path, fallback);
        assert_eq!(cwd.should_chdir, true);
        let warning = cwd.warning.unwrap();
        assert!(warning.contains("PWD points to a directory that does not exist"));
        assert!(warning.contains(&missing.to_string_lossy().to_string()));
    }

    #[test]
    fn current_dir_recovers_from_missing_pwd_with_fallback() {
        let fallback = std::env::temp_dir();
        let cwd = resolve_current_dir(
            Err(io::Error::new(io::ErrorKind::NotFound, "missing cwd")),
            None,
            Some(fallback.clone()),
        )
        .unwrap();

        assert_eq!(cwd.path, fallback);
        assert_eq!(cwd.should_chdir, true);
        assert!(cwd
            .warning
            .unwrap()
            .contains("The previous current directory is unavailable"));
    }

    #[test]
    fn current_dir_reports_when_no_fallback_exists() {
        let err = resolve_current_dir(
            Err(io::Error::new(io::ErrorKind::NotFound, "missing cwd")),
            None,
            None,
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("no fallback directory was available"));
    }
}

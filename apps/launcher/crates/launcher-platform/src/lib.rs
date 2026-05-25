use launcher_core::{
    LAUNCHER_STATE_SCHEMA_VERSION, LauncherPathLayout, LauncherStateSnapshot, PayloadEntry,
    PendingPromotionPlan, StatePointer, plan_pending_promotion,
};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LauncherPlatformError {
    #[error("required environment variable is missing: {0}")]
    MissingEnv(&'static str),
    #[error("launcher lock is already held: {0}")]
    LockAlreadyHeld(String),
    #[error("unsupported launcher state schema at {path}: expected {expected}, got {actual}")]
    UnsupportedStateSchema {
        actual: u32,
        expected: u32,
        path: String,
    },
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub struct LauncherLock {
    path: PathBuf,
}

impl LauncherLock {
    pub fn acquire(path: impl AsRef<Path>) -> Result<Self, LauncherPlatformError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    LauncherPlatformError::LockAlreadyHeld(path.display().to_string())
                } else {
                    LauncherPlatformError::Io(error)
                }
            })?;
        writeln!(file, "pid={}", std::process::id())?;
        Ok(Self { path })
    }
}

impl Drop for LauncherLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn default_data_root() -> Result<PathBuf, LauncherPlatformError> {
    if cfg!(target_os = "windows") {
        env_path("APPDATA")
    } else if cfg!(target_os = "macos") {
        Ok(env_path("HOME")?.join("Library").join("Application Support"))
    } else if let Some(value) = env::var_os("XDG_DATA_HOME") {
        Ok(PathBuf::from(value))
    } else {
        Ok(env_path("HOME")?.join(".local").join("share"))
    }
}

pub fn ensure_launcher_layout(paths: &LauncherPathLayout) -> Result<(), LauncherPlatformError> {
    for directory in [
        &paths.downloads_root,
        &paths.installer_observations_root,
        &paths.launcher_observations_root,
        &paths.state_root,
        &paths.staging_root,
        &paths.update_logs_root,
        &paths.updater_observations_root,
        &paths.versions_root,
    ] {
        fs::create_dir_all(directory)?;
    }
    Ok(())
}

pub fn read_launcher_state(paths: &LauncherPathLayout) -> Result<LauncherStateSnapshot, LauncherPlatformError> {
    Ok(LauncherStateSnapshot {
        current: read_state_pointer(&paths.current_state_path)?,
        pending: read_state_pointer(&paths.pending_state_path)?,
        previous: read_state_pointer(&paths.previous_state_path)?,
    })
}

pub fn apply_pending_state_promotion(
    paths: &LauncherPathLayout,
) -> Result<PendingPromotionPlan, LauncherPlatformError> {
    ensure_launcher_layout(paths)?;
    let _lock = LauncherLock::acquire(&paths.state_lock_path)?;
    let snapshot = read_launcher_state(paths)?;
    let plan = plan_pending_promotion(&snapshot);

    if plan.promote {
        if let Some(current) = &plan.current {
            write_state_pointer(&paths.current_state_path, current)?;
        }
        if let Some(previous) = &plan.previous {
            write_state_pointer(&paths.previous_state_path, previous)?;
        } else {
            remove_file_if_exists(&paths.previous_state_path)?;
        }
        if plan.remove_pending {
            remove_file_if_exists(&paths.pending_state_path)?;
        }
    }

    Ok(plan)
}

pub fn read_state_pointer(path: impl AsRef<Path>) -> Result<Option<StatePointer>, LauncherPlatformError> {
    let path = path.as_ref();
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let pointer: StatePointer = serde_json::from_reader(file)?;
    if pointer.schema_version != LAUNCHER_STATE_SCHEMA_VERSION {
        return Err(LauncherPlatformError::UnsupportedStateSchema {
            actual: pointer.schema_version,
            expected: LAUNCHER_STATE_SCHEMA_VERSION,
            path: path.display().to_string(),
        });
    }
    Ok(Some(pointer))
}

pub fn write_state_pointer(
    path: impl AsRef<Path>,
    pointer: &StatePointer,
) -> Result<(), LauncherPlatformError> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = File::create(path)?;
    serde_json::to_writer_pretty(&mut file, pointer)?;
    file.write_all(b"\n")?;
    Ok(())
}

pub fn spawn_payload(entry: &PayloadEntry, payload_root: impl AsRef<Path>) -> Result<Child, LauncherPlatformError> {
    let payload_root = payload_root.as_ref();
    let executable = resolve_payload_path(payload_root, &entry.executable);
    let mut command = Command::new(executable);
    command.args(&entry.args);
    command.envs(&entry.env);
    command.current_dir(match &entry.cwd {
        Some(cwd) => resolve_payload_path(payload_root, cwd),
        None => payload_root.to_path_buf(),
    });
    Ok(command.spawn()?)
}

pub fn write_observation(path: impl AsRef<Path>, payload: &str) -> Result<(), LauncherPlatformError> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = File::create(path)?;
    file.write_all(payload.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

fn env_path(name: &'static str) -> Result<PathBuf, LauncherPlatformError> {
    env::var_os(name)
        .map(PathBuf::from)
        .ok_or(LauncherPlatformError::MissingEnv(name))
}

fn resolve_payload_path(root: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn remove_file_if_exists(path: &Path) -> Result<(), LauncherPlatformError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

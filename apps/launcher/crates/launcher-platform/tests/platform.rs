use launcher_core::{LauncherIdentity, LauncherPathLayout, Namespace, ReleaseChannel, StatePointer};
use launcher_platform::{
    LauncherLock, LauncherPlatformError, apply_pending_state_promotion, ensure_launcher_layout,
    read_launcher_state, write_state_pointer,
};
use std::fs;
use std::path::PathBuf;

fn temp_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "open-design-launcher-test-{}-{}",
        name,
        std::process::id()
    ))
}

fn pointer(version: &str, timestamp: &str) -> StatePointer {
    StatePointer::new(
        version,
        format!("versions/{version}/payload"),
        format!("versions/{version}/manifest.json"),
        timestamp,
    )
    .unwrap()
}

#[test]
fn lock_stays_exclusive() {
    let root = temp_root("lock");
    let lock_path = root.join("state").join("lock");
    let lock = LauncherLock::acquire(&lock_path).unwrap();

    assert!(LauncherLock::acquire(&lock_path).is_err());
    drop(lock);
    assert!(LauncherLock::acquire(&lock_path).is_ok());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn layout_creates_dirs() {
    let root = temp_root("layout");
    let identity = LauncherIdentity::new(ReleaseChannel::Preview, Namespace::new("preview-local").unwrap());
    let paths = LauncherPathLayout::from_data_root(&root, &identity);

    ensure_launcher_layout(&paths).unwrap();

    assert!(paths.state_root.is_dir());
    assert!(paths.versions_root.is_dir());
    assert!(paths.staging_root.is_dir());
    assert!(paths.launcher_observations_root.is_dir());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn pending_promotion_persists() {
    let root = temp_root("pending-promotion");
    let _ = fs::remove_dir_all(&root);
    let identity = LauncherIdentity::new(ReleaseChannel::Beta, Namespace::new("release-beta-win").unwrap());
    let paths = LauncherPathLayout::from_data_root(&root, &identity);
    let current = pointer("0.8.0-beta.6", "2026-05-22T00:00:00Z");
    let pending = pointer("0.8.0-beta.7", "2026-05-22T00:01:00Z");
    write_state_pointer(&paths.current_state_path, &current).unwrap();
    write_state_pointer(&paths.pending_state_path, &pending).unwrap();

    let plan = apply_pending_state_promotion(&paths).unwrap();

    assert!(plan.promote);
    assert!(plan.remove_pending);
    let snapshot = read_launcher_state(&paths).unwrap();
    assert_eq!(snapshot.current, Some(pending));
    assert_eq!(snapshot.previous, Some(current));
    assert_eq!(snapshot.pending, None);
    assert!(!paths.pending_state_path.exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn state_schema_is_checked() {
    let root = temp_root("state-schema");
    let _ = fs::remove_dir_all(&root);
    let identity = LauncherIdentity::new(ReleaseChannel::Beta, Namespace::new("release-beta-win").unwrap());
    let paths = LauncherPathLayout::from_data_root(&root, &identity);
    fs::create_dir_all(&paths.state_root).unwrap();
    fs::write(
        &paths.current_state_path,
        r#"{
  "schemaVersion": 999,
  "version": "0.8.0-beta.6",
  "payloadRoot": "versions/0.8.0-beta.6/payload",
  "manifestPath": "versions/0.8.0-beta.6/manifest.json",
  "updatedAt": "2026-05-22T00:00:00Z"
}"#,
    )
    .unwrap();

    assert!(matches!(
        read_launcher_state(&paths),
        Err(LauncherPlatformError::UnsupportedStateSchema { actual: 999, .. })
    ));

    let _ = fs::remove_dir_all(root);
}

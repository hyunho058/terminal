//! Acceptance test TT2 for R2's GWTs (top-settings-menu: settings file resolution,
//! seeding, and launch error handling).
//!
//! Tests:
//! - R2.1: Seed default configuration when config.toml is missing
//! - R2.2: Existing configuration preserved without overwrite
//! - R2.4: Safe error handling on launch or file creation failure

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use app::ensure_config_file_exists_at;

/// RAII temporary directory under `std::env::temp_dir()`.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!("mac-term-tt2-{tag}-{}-{}", std::process::id(), id));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("failed to create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn r2_1_seed_default_configuration_when_missing() {
    // Given: A target file path in a temporary directory where no file exists,
    // including non-existent parent subdirectories.
    let temp = TempDir::new("missing");
    let target_path = temp.path().join("sub_dir").join("config.toml");
    assert!(!target_path.exists());

    // When: ensure_config_file_exists_at(&target_path) is called.
    let result = ensure_config_file_exists_at(&target_path);

    // Then: Parent directories are created, default TOML content
    // (containing at least "[font]" and "[theme]") is written to path, and Ok(path) is returned.
    let returned_path = result.expect("should create default config file successfully");
    assert_eq!(returned_path, target_path);
    assert!(target_path.is_file(), "config file should have been created on disk");

    let content = fs::read_to_string(&target_path).expect("config file should be readable");
    assert!(
        content.contains("[font]"),
        "seeded config must contain '[font]' section, but got:\n{content}"
    );
    assert!(
        content.contains("[theme]"),
        "seeded config must contain '[theme]' section, but got:\n{content}"
    );
}

#[test]
fn r2_2_existing_configuration_preserved_without_overwrite() {
    // Given: An existing config.toml exists at path with user modifications.
    let temp = TempDir::new("existing");
    let target_path = temp.path().join("config.toml");
    let custom_content = "# custom user comment\n[font]\nsize_pt = 22.0\n";
    fs::write(&target_path, custom_content).expect("failed to write existing config");

    // When: ensure_config_file_exists_at(&target_path) is called.
    let result = ensure_config_file_exists_at(&target_path);

    // Then: The file on disk is untouched and Ok(path) is returned with the existing file path.
    let returned_path = result.expect("should return Ok for existing config file");
    assert_eq!(returned_path, target_path);

    let content = fs::read_to_string(&target_path).expect("config file should still be readable");
    assert_eq!(
        content, custom_content,
        "existing config content must remain unmodified"
    );
}

#[test]
fn r2_4_safe_error_handling_when_parent_is_file() {
    // Given: An invalid directory path where writing fails (parent component is a file).
    let temp = TempDir::new("invalid_parent");
    let blocker = temp.path().join("blocker_file");
    fs::write(&blocker, "blocking regular file").expect("failed to write blocker file");

    // Attempting to create parent directories under a regular file fails.
    let target_path = blocker.join("nested").join("config.toml");

    // When: ensure_config_file_exists_at(&target_path) handles the error.
    let result = ensure_config_file_exists_at(&target_path);

    // Then: Returns an Err instead of panicking.
    assert!(
        result.is_err(),
        "ensure_config_file_exists_at should return Err when parent path is invalid"
    );
}

#[test]
fn r2_4_safe_error_handling_when_dir_is_readonly() {
    use std::os::unix::fs::PermissionsExt;

    // Given: A read-only directory path where file creation fails.
    let temp = TempDir::new("readonly");
    let readonly_dir = temp.path().join("ro_dir");
    fs::create_dir_all(&readonly_dir).expect("failed to create directory");

    // Make directory read-only (0o444).
    let mut perms = fs::metadata(&readonly_dir).unwrap().permissions();
    perms.set_mode(0o444);
    fs::set_permissions(&readonly_dir, perms).expect("failed to set read-only permissions");

    let target_path = readonly_dir.join("config.toml");

    // When: ensure_config_file_exists_at(&target_path) handles the error.
    let result = ensure_config_file_exists_at(&target_path);

    // Restore permissions so TempDir drop can clean it up.
    let mut restore_perms = fs::metadata(&readonly_dir).unwrap().permissions();
    restore_perms.set_mode(0o755);
    let _ = fs::set_permissions(&readonly_dir, restore_perms);

    // Then: Returns an Err instead of panicking.
    assert!(
        result.is_err(),
        "ensure_config_file_exists_at should return Err when writing to read-only directory fails"
    );
}

//! Composite device fingerprint (no IP).
//!
//! `device_id = sha256(system_id | disk_volume_serial | local_random_salt)[..16]` hex.
//! The random salt is generated once and cached locally, so the id is stable per
//! install yet not purely derivable from hardware (raises forgery/collision bar).
//! Honest limit: the cached salt is local-readable → not cryptographic device
//! binding. A per-device keypair + challenge signature would be the stronger option.

use std::fs;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

/// Stable per-install device id. Never displayed in the UI.
pub fn device_id() -> String {
    let sys = machine_uid::get().unwrap_or_default();
    let disk = volume_serial();
    let salt = load_or_create_salt();
    let mut h = Sha256::new();
    h.update(format!("{sys}|{disk}|{salt}").as_bytes());
    let digest = h.finalize();
    hex::encode(&digest[..16]) // 32 hex chars
}

fn salt_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "opsctl")
        .map(|d| d.data_local_dir().join("device.json"))
}

/// Read the cached salt, or generate + persist a new one on first run.
fn load_or_create_salt() -> String {
    let Some(path) = salt_path() else {
        return "no-salt".into();
    };
    if let Ok(bytes) = fs::read(&path) {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            if let Some(s) = v.get("salt").and_then(|s| s.as_str()) {
                return s.to_string();
            }
        }
    }
    let salt = uuid::Uuid::new_v4().to_string();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&path, serde_json::json!({ "salt": salt }).to_string());
    salt
}

/// System-drive volume serial number (Windows). Empty on other platforms /
/// failure — the system id + salt still produce a stable id.
#[cfg(windows)]
fn volume_serial() -> String {
    use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW;
    let root: Vec<u16> = "C:\\".encode_utf16().chain(std::iter::once(0)).collect();
    let mut serial: u32 = 0;
    let ok = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            std::ptr::null_mut(),
            0,
            &mut serial,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        )
    };
    if ok != 0 {
        format!("{serial:08x}")
    } else {
        String::new()
    }
}

#[cfg(not(windows))]
fn volume_serial() -> String {
    String::new()
}

//! Content pack sync: fetch the manifest from the user's PC over the tailnet and download changed files.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
pub struct Manifest {
    pub files: Vec<ManifestFile>,
}

#[derive(Deserialize)]
pub struct ManifestFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

/// Where the pack lives on the device. On iOS this is the app's Documents folder (visible in the Files app).
pub fn pack_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join("Documents").join("pack")
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn up_to_date(local: &Path, file: &ManifestFile) -> bool {
    fs::read(local).is_ok_and(|bytes| bytes.len() as u64 == file.size && sha256_hex(&bytes) == file.sha256)
}

fn get(url: &str) -> Result<Vec<u8>, String> {
    let mut response = ureq::get(url).call().map_err(|e| format!("{url}: {e}"))?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{url}: {e}"))?;
    Ok(bytes)
}

/// Bring `dir` in line with the server's manifest. `report` receives human-readable progress lines.
pub fn sync(server: &str, dir: &Path, report: &dyn Fn(String)) -> Result<Manifest, String> {
    report(format!("Contacting {server} ..."));
    let manifest: Manifest = serde_json::from_slice(&get(&format!("{server}/manifest.json"))?)
        .map_err(|e| format!("manifest: {e}"))?;

    let stale: Vec<&ManifestFile> = manifest.files.iter().filter(|f| !up_to_date(&dir.join(&f.path), f)).collect();
    let total: u64 = stale.iter().map(|f| f.size).sum();
    let mut done = 0u64;
    for (i, file) in stale.iter().enumerate() {
        report(format!(
            "Downloading {}/{}: {} ({:.1} / {:.1} MB)",
            i + 1,
            stale.len(),
            file.path,
            done as f64 / 1e6,
            total as f64 / 1e6
        ));
        let bytes = get(&format!("{server}/{}", file.path))?;
        if sha256_hex(&bytes) != file.sha256 {
            return Err(format!("{}: checksum mismatch", file.path));
        }
        let local = dir.join(&file.path);
        fs::create_dir_all(local.parent().unwrap()).map_err(|e| e.to_string())?;
        let partial = local.with_extension("partial");
        fs::write(&partial, &bytes).map_err(|e| e.to_string())?;
        fs::rename(&partial, &local).map_err(|e| e.to_string())?;
        done += file.size;
    }
    Ok(manifest)
}

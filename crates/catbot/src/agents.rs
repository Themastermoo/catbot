//! Agent programs Catbot downloads itself (stage 12.4.3): Antigravity's ACP server from Google
//! (and Muse Code from Meta, 12.4.5), as the ACP registry lists them, kept in
//! `<catbot home>/agents/<id>/<version>`.
//!
//! The download is checked against the registry's SHA-256 before anything in it is used, unpacked
//! into a folder of its own (no entry may climb out of it), and only then put in place, so a
//! half-done download is never mistaken for an installed program.

use std::io::Write;
use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use sha2::{Digest, Sha256};

use crate::engines::{self, Engine};

/// Where a download comes from: the registry's, or `CATBOT_DOWNLOAD_<ID>="<url> <sha256>"` (tests).
fn source(engine: &Engine) -> Option<(String, String, &'static str, bool)> {
    let asset = engines::asset(engine)?;
    let variable = format!("CATBOT_DOWNLOAD_{}", engine.id.to_uppercase().replace('-', "_"));
    if let Ok(set) = std::env::var(&variable)
        && let Some((url, sha)) = set.trim().split_once(' ')
    {
        return Some((url.to_string(), sha.trim().to_string(), asset.cmd, asset.zip));
    }
    Some((asset.url.to_string(), asset.sha256.to_string(), asset.cmd, asset.zip))
}

/// Downloads and unpacks an engine's program; `progress` hears the bytes so far and the size.
/// Gives back the program's path.
pub async fn install(engine: &Engine, progress: impl Fn(u64, Option<u64>)) -> Result<PathBuf, String> {
    let folder = engines::download_folder(engine).ok_or_else(|| format!("{} can't be downloaded here", engine.name))?;
    let (url, sha, cmd, zipped) = source(engine).ok_or_else(|| format!("{} has no download for this computer ({})", engine.name, engines::platform()))?;
    let parent = folder.parent().ok_or("no folder for it")?.to_path_buf();
    std::fs::create_dir_all(&parent).map_err(|e| format!("couldn't make {}: {e}", parent.display()))?;
    let file = parent.join(format!("{}.download", engines::platform()));
    fetch(&url, &file, &sha, &progress).await?;
    let fresh = parent.join(format!("{}.new", engine.download.map(|d| d.version).unwrap_or("new")));
    let _ = std::fs::remove_dir_all(&fresh);
    let (from, into, program) = (file.clone(), fresh.clone(), cmd.to_string());
    tokio::task::spawn_blocking(move || if zipped { unzip(&from, &into) } else { place(&from, &into, &program) }).await.map_err(|e| e.to_string())??;
    let _ = std::fs::remove_file(&file);
    let program = fresh.join(cmd);
    if !program.is_file() {
        let _ = std::fs::remove_dir_all(&fresh);
        return Err(format!("the download has no {cmd} in it"));
    }
    make_runnable(&program);
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::rename(&fresh, &folder).map_err(|e| format!("couldn't put it in place: {e}"))?;
    Ok(folder.join(cmd))
}

async fn fetch(url: &str, into: &Path, sha: &str, progress: &impl Fn(u64, Option<u64>)) -> Result<(), String> {
    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(30)).build().map_err(|e| e.to_string())?;
    let answer = client.get(url).send().await.map_err(|e| format!("couldn't download it: {e}"))?;
    if !answer.status().is_success() {
        return Err(format!("the download answered {}", answer.status()));
    }
    let total = answer.content_length();
    let mut out = std::fs::File::create(into).map_err(|e| format!("couldn't save it: {e}"))?;
    let mut hash = Sha256::new();
    let mut got = 0u64;
    let mut last = 0u64;
    let mut stream = answer.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("the download broke off: {e}"))?;
        hash.update(&chunk);
        out.write_all(&chunk).map_err(|e| format!("couldn't save it: {e}"))?;
        got += chunk.len() as u64;
        if got - last > 4 * 1024 * 1024 {
            last = got;
            progress(got, total);
        }
    }
    out.flush().map_err(|e| e.to_string())?;
    progress(got, total);
    let found = format!("{:x}", hash.finalize());
    if !found.eq_ignore_ascii_case(sha) {
        let _ = std::fs::remove_file(into);
        return Err("the download isn't the file it should be (its checksum doesn't match), so it wasn't used".into());
    }
    Ok(())
}

/// Unpacks a zip, refusing any entry that would land outside `into`.
fn unzip(file: &Path, into: &Path) -> Result<(), String> {
    let reader = std::fs::File::open(file).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| format!("the download isn't a zip: {e}"))?;
    std::fs::create_dir_all(into).map_err(|e| e.to_string())?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let Some(name) = entry.enclosed_name() else { return Err(format!("the zip has an unsafe name: {}", entry.name())) };
        let path = into.join(name);
        if entry.is_dir() {
            std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = std::fs::File::create(&path).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode & 0o755));
        }
    }
    Ok(())
}

/// A download that is the program itself.
fn place(file: &Path, into: &Path, cmd: &str) -> Result<(), String> {
    std::fs::create_dir_all(into).map_err(|e| e.to_string())?;
    std::fs::copy(file, into.join(cmd)).map(|_| ()).map_err(|e| e.to_string())
}

fn make_runnable(program: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(program, std::fs::Permissions::from_mode(0o755));
    }
    #[cfg(not(unix))]
    let _ = program;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zip_cant_climb_out() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bad.zip");
        {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&file).unwrap());
            zip.start_file("../evil.txt", zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(b"x").unwrap();
            zip.finish().unwrap();
        }
        assert!(unzip(&file, &dir.path().join("out")).unwrap_err().contains("unsafe"));
        assert!(!dir.path().join("evil.txt").exists());
    }
}

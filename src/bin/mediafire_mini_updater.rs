#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};
use std::os::windows::process::CommandExt;

fn log(folder: &Path, message: &str) {
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(folder.join("MediaFireMiniUpdater.log")) {
        let _ = writeln!(f, "{message}");
    }
}

fn backup_name(folder: &Path) -> PathBuf {
    let first = folder.join("MediaFireMini.exe.bak");
    if !first.exists() { return first; }
    for i in 2..10_000 {
        let p = folder.join(format!("MediaFireMini.exe.bak-{i}"));
        if !p.exists() { return p; }
    }
    folder.join("MediaFireMini.exe.bak-final")
}

fn verify_sha(path: &Path, expected: &str) -> Result<(), String> {
    if expected.len() != 64 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("Invalid SHA256 digest".into());
    }
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut block = [0u8; 65536];
    loop {
        let len = file.read(&mut block).map_err(|e| e.to_string())?;
        if len == 0 { break; }
        hasher.update(&block[..len]);
    }
    if format!("{:x}", hasher.finalize()) != expected.to_ascii_lowercase() {
        return Err("SHA256 mismatch after staging".into());
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 5 { return Err("Expected PID, target, stage and SHA256".into()); }
    let _parent_pid = args[1].to_string_lossy().parse::<u32>().map_err(|e| e.to_string())?;
    let target = PathBuf::from(&args[2]);
    let staged = PathBuf::from(&args[3]);
    let hash = args[4].to_string_lossy().to_string();
    let folder = target.parent().ok_or("Target directory is invalid")?;
    if target.file_name().and_then(|x| x.to_str()) != Some("MediaFireMini.exe")
        || staged.file_name().and_then(|x| x.to_str()) != Some("MediaFireMini.new.exe")
        || staged.parent() != Some(folder) {
        return Err("Unsafe updater target rejected".into());
    }
    verify_sha(&staged, &hash)?;
    log(folder, "SHA256 verified; waiting for GUI to exit.");
    let backup = backup_name(folder);
    let mut renamed = false;
    for _ in 0..120 {
        if fs::rename(&target, &backup).is_ok() { renamed = true; break; }
        thread::sleep(Duration::from_millis(500));
    }
    if !renamed { return Err("Cannot rename running GUI (60 s timeout)".into()); }
    if let Err(e) = fs::rename(&staged, &target) {
        let _ = fs::rename(&backup, &target);
        return Err(format!("Replacement failed; rollback attempted: {e}"));
    }
    match Command::new(&target)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .creation_flags(0x08000000).spawn()
    {
        Ok(_) => { log(folder, "GUI successfully updated and restarted; backup kept."); Ok(()) },
        Err(e) => {
            let failed = folder.join("MediaFireMini.failed.exe");
            let _ = fs::rename(&target, &failed);
            let restored = fs::rename(&backup, &target);
            let _ = fs::remove_file(&failed);
            if restored.is_ok() { let _ = Command::new(&target).creation_flags(0x08000000).spawn(); }
            Err(format!("Restart failed ({e}), rollback status: {restored:?}"))
        }
    }
}

fn main() {
    if let Err(e) = run() {
        let folder = std::env::current_exe().ok()
            .and_then(|p| p.parent().map(|x| x.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        log(&folder, &format!("ERROR: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn digest_check() {
        let p = std::env::temp_dir().join(format!("mini-updater-test-{}", std::process::id()));
        fs::write(&p, b"abc").unwrap();
        assert!(verify_sha(&p, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad").is_ok());
        assert!(verify_sha(&p, &"0".repeat(64)).is_err());
        fs::remove_file(p).unwrap();
    }
}

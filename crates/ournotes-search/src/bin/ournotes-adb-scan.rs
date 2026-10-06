//! Pull candidate Our Notes save files through an already-authorized adb shell.
//!
//! This mirrors StarMoe-box's Shizuku scan: package `files/`, hexadecimal account
//! directories, bounded files, and newest-copy metadata. Decryption stays out of
//! this transport helper; the Android app owns the build-time save key.

use serde_json::json;
use std::{env, fs, path::PathBuf, process::Command, sync::Arc, thread};

const MAX_FILE: u64 = 16 << 20;

fn adb(args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("adb").args(args).output().map_err(|e| format!("adb: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(out.stdout)
}

fn is_account_dir(s: &str) -> bool {
    let n = s.len();
    (16..=128).contains(&n) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut package = None;
    let mut output = PathBuf::from("adb-saves");
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--package" => package = args.next(),
            "--out" => output = PathBuf::from(args.next().ok_or("--out requires a path")?),
            "--help" => {
                println!("usage: ournotes-adb-scan --package PACKAGE [--out DIR]");
                return Ok(());
            }
            _ => return Err(format!("unknown option {arg}").into()),
        }
    }
    let package = package.ok_or("--package is required")?;
    let root = format!("/sdcard/Android/data/{package}/files");
    fs::create_dir_all(&output)?;
    let top = String::from_utf8(adb(&["shell", "ls", "-1p", &root])?)?;
    let dirs: Vec<_> = top
        .lines()
        .map(str::trim)
        .filter(|s| s.ends_with('/') && is_account_dir(s.trim_end_matches('/')))
        .map(|s| s.trim_end_matches('/').to_owned())
        .collect();
    let output = Arc::new(output);
    thread::scope(|scope| {
        for dir in dirs {
            let package = package.clone();
            let root = root.clone();
            let output = Arc::clone(&output);
            scope.spawn(move || {
                let path = format!("{root}/{dir}");
                let listing = match adb(&["shell", "ls", "-1p", &path]) { Ok(v) => String::from_utf8_lossy(&v).into_owned(), Err(_) => return };
                for name in listing.lines().map(str::trim).filter(|s| !s.is_empty() && !s.ends_with('/')) {
                    let file = format!("{path}/{name}");
                    let stat = adb(&["shell", "stat", "-c", "%s %Y", &file]).ok().and_then(|v| String::from_utf8(v).ok());
                    let parts: Vec<_> = stat.as_deref().unwrap_or("").split_whitespace().collect();
                    let size = parts.first().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                    if size == 0 || size > MAX_FILE { continue; }
                    let bytes = match adb(&["exec-out", "cat", &file]) { Ok(v) if v.len() as u64 <= MAX_FILE => v, _ => continue };
                    let name_on_disk = format!("{}_{}", dir, name);
                    if fs::write(output.join(&name_on_disk), &bytes).is_ok() {
                        let record = json!({"package":package,"path":file,"size":size,"mtime":parts.get(1).and_then(|s| s.parse::<i64>().ok()),"file":name_on_disk});
                        let _ = fs::write(output.join(format!("{name_on_disk}.json")), serde_json::to_vec_pretty(&record).unwrap());
                    }
                }
            });
        }
    });
    println!("saved candidates in {}", output.display());
    Ok(())
}

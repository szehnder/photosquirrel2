//! Video poster frames through an optional external `ffmpeg` (ADR-0017), with a hard timeout and kill.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use thiserror::Error;

/// Default poster-frame timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Error)]
pub enum PosterError {
    #[error("could not start the poster-frame tool: {0}")]
    Spawn(std::io::Error),
    #[error("poster-frame tool timed out after {0:?}")]
    Timeout(Duration),
    #[error("poster-frame tool failed ({0})")]
    Failed(String),
}

/// Grab one frame at 1 s, scaled to 384 px on the long edge, as JPEG bytes on stdout. The source is only
/// read by the tool; nothing is written next to it.
pub fn poster_frame(tool: &Path, source: &Path, timeout: Duration) -> Result<Vec<u8>, PosterError> {
    let mut child = Command::new(tool)
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-ss",
            "1",
            "-i",
        ])
        .arg(source)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale='if(gt(iw,ih),384,-2)':'if(gt(iw,ih),-2,384)'",
            "-f",
            "image2",
            "-c:v",
            "mjpeg",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(PosterError::Spawn)?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| PosterError::Failed("no stdout".into()))?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.by_ref().take(16 * 1024 * 1024).read_to_end(&mut buf);
        buf
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(PosterError::Timeout(timeout));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(PosterError::Failed(e.to_string())),
        }
    };
    let out = reader.join().unwrap_or_default();
    if !status.success() {
        return Err(PosterError::Failed(format!("exit status {status}")));
    }
    if !out.starts_with(&[0xFF, 0xD8]) {
        return Err(PosterError::Failed("output is not a JPEG".into()));
    }
    Ok(out)
}

/// `ffmpeg` found on `PATH`, if any (for callers resolving the default tool).
pub fn ffmpeg_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let names: &[&str] = if cfg!(windows) {
        &["ffmpeg.exe"]
    } else {
        &["ffmpeg"]
    };
    std::env::split_paths(&path)
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|p| p.is_file())
}

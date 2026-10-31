//! Video poster frames through an optional external `ffmpeg` (ADR-0017, ADR-0114), with a hard timeout, a stop
//! check and a kill.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use thiserror::Error;

/// Default poster-frame timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// The seek of a poster frame when the clip's duration is unknown or long enough (ADR-0114).
pub const DEFAULT_SEEK: Duration = Duration::from_secs(1);

/// Threads each poster-frame call may use (`-threads`, ADR-0114).
pub const TOOL_THREADS: u32 = 2;

/// How often the wait loop checks the child, the timeout and the stop check.
const POLL: Duration = Duration::from_millis(10);

/// Bytes of the tool's error output kept for the message.
const STDERR_LIMIT: u64 = 8 * 1024;

#[derive(Debug, Error)]
pub enum PosterError {
    /// The tool could not be started (missing, or without the execute permission): no tool, says nothing about
    /// the file (ADR-0114).
    #[error("could not start the poster-frame tool: {0}")]
    Spawn(std::io::Error),
    #[error("poster-frame tool timed out after {0:?}")]
    Timeout(Duration),
    #[error("poster-frame tool failed ({0})")]
    Failed(String),
    /// The tool succeeded and wrote nothing: no frame at the seek (a clip shorter than it).
    #[error("the poster-frame tool gave no frame")]
    NoFrame,
    /// The run was cancelled: the tool was killed. Says nothing about the file.
    #[error("cancelled")]
    Cancelled,
}

/// The seek of a clip of `duration_ms`: `min(1 s, duration / 2)`, 1 s when the duration is unknown (ADR-0114).
pub fn seek_for(duration_ms: Option<u64>) -> Duration {
    match duration_ms {
        Some(ms) => DEFAULT_SEEK.min(Duration::from_millis(ms / 2)),
        None => DEFAULT_SEEK,
    }
}

/// Grab one frame at `seek`, scaled to 384 px on the long edge, as JPEG bytes on stdout. The source is only read
/// by the tool; nothing is written next to it. The child is killed after `timeout`, or as soon as `stop` returns
/// true (checked every 10 ms).
pub fn poster_frame(
    tool: &Path,
    source: &Path,
    seek: Duration,
    timeout: Duration,
    stop: &dyn Fn() -> bool,
) -> Result<Vec<u8>, PosterError> {
    if stop() {
        return Err(PosterError::Cancelled);
    }
    let threads = TOOL_THREADS.to_string();
    let seek = format!("{:.3}", seek.as_secs_f64());
    let mut child = Command::new(tool)
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-threads",
            &threads,
            "-ss",
            &seek,
            "-i",
        ])
        .arg(source)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale='if(gt(iw,ih),384,-2)':'if(gt(iw,ih),-2,384)'",
            "-threads",
            &threads,
            "-f",
            "image2",
            "-c:v",
            "mjpeg",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
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
    let stderr = child.stderr.take().map(|mut e| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = e.by_ref().take(STDERR_LIMIT).read_to_end(&mut buf);
            // Drain the rest so the tool never blocks on a full pipe.
            let _ = std::io::copy(&mut e, &mut std::io::sink());
            buf
        })
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if stop() => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(PosterError::Cancelled);
            }
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(PosterError::Timeout(timeout));
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(PosterError::Failed(e.to_string()));
            }
        }
    };
    let out = reader.join().unwrap_or_default();
    let err = stderr
        .and_then(|h| h.join().ok())
        .map(|b| last_line(&b))
        .unwrap_or_default();
    if !status.success() {
        // `ExitStatus` displays as "exit status: 1" (or "signal: 9").
        return Err(PosterError::Failed(if err.is_empty() {
            status.to_string()
        } else {
            format!("{status}: {err}")
        }));
    }
    if out.is_empty() {
        return Err(PosterError::NoFrame);
    }
    if !out.starts_with(&[0xFF, 0xD8]) {
        return Err(PosterError::Failed("output is not a JPEG".into()));
    }
    Ok(out)
}

/// The last non-empty line of the tool's error output, without control characters, at most 200 characters.
fn last_line(b: &[u8]) -> String {
    let text = String::from_utf8_lossy(b);
    let line = text
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .unwrap_or_default();
    line.chars().filter(|c| !c.is_control()).take(200).collect()
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
        .find(|p| is_executable(p))
}

/// A file this process may run: a regular file, with an execute permission on Unix.
pub fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// The poster-frame tool available now (ADR-0114): the configured path when it is an executable file, else
/// `ffmpeg` on `PATH`, else none. Cheap (a few stats); callers resolve it again when each run starts.
pub fn resolve_tool(configured: Option<&Path>) -> Option<PathBuf> {
    configured
        .filter(|p| is_executable(p))
        .map(Path::to_path_buf)
        .or_else(ffmpeg_on_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seek_is_at_most_half_the_clip() {
        assert_eq!(seek_for(None), Duration::from_secs(1));
        assert_eq!(seek_for(Some(12_500)), Duration::from_secs(1));
        assert_eq!(seek_for(Some(2_000)), Duration::from_secs(1));
        assert_eq!(seek_for(Some(500)), Duration::from_millis(250));
        assert_eq!(seek_for(Some(0)), Duration::ZERO);
    }

    #[test]
    fn the_last_error_line_is_kept() {
        assert_eq!(
            last_line(b"first\n  x.mov: Invalid data found\n\n"),
            "x.mov: Invalid data found"
        );
        assert_eq!(last_line(b""), "");
    }

    #[test]
    fn a_configured_path_that_is_not_a_file_is_not_a_tool() {
        let dir = std::env::temp_dir();
        // A directory is never the tool; the result is then whatever `PATH` has.
        assert_eq!(resolve_tool(Some(&dir)), ffmpeg_on_path());
        assert_eq!(
            resolve_tool(Some(Path::new("/nonexistent/ffmpeg"))),
            ffmpeg_on_path()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_configured_file_without_the_execute_permission_is_not_a_tool() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = dir.path().join("ffmpeg");
        std::fs::write(&tool, "#!/bin/sh\n").expect("write");
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        assert!(!is_executable(&tool));
        assert_eq!(resolve_tool(Some(&tool)), ffmpeg_on_path());
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert_eq!(resolve_tool(Some(&tool)), Some(tool));
    }
}

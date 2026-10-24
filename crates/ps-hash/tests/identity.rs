//! Content identity scenarios (openspec/changes/content-hashing/specs/content-identity).
#![allow(clippy::unwrap_used)]

mod common;

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Duration, SystemTime};

use common::CountingReader;
use ps_hash::{identity_from_buffer, identity_from_reader, read_for_analysis};
use ps_model::ContentIdKind;
use ps_model::limits::{ANALYSIS_BUFFER_MAX_BYTES, FULL_IDENTITY_MAX_BYTES, SAMPLE_WINDOW_BYTES};
use rand::{RngCore, SeedableRng};
use rand_chacha::ChaCha8Rng;

const MB: u64 = 1024 * 1024;

fn random_bytes(seed: u64, n: usize) -> Vec<u8> {
    let mut v = vec![0u8; n];
    ChaCha8Rng::seed_from_u64(seed).fill_bytes(&mut v);
    v
}

/// A sparse file of `len` bytes with random 64 KB patches every `every` bytes (holes read as zeros).
fn sparse_file(path: &Path, len: u64, seed: u64, every: u64) {
    let mut f = File::create(path).unwrap();
    f.set_len(len).unwrap();
    let mut off = 0;
    let mut i = 0;
    while off < len {
        let n = (64 * 1024).min(len - off) as usize;
        f.seek(SeekFrom::Start(off)).unwrap();
        f.write_all(&random_bytes(seed + i, n)).unwrap();
        off += every;
        i += 1;
    }
    // The very end too, so the last window is not all holes.
    let tail = (64 * 1024).min(len);
    f.seek(SeekFrom::Start(len - tail)).unwrap();
    f.write_all(&random_bytes(seed ^ 0xdead, tail as usize))
        .unwrap();
}

fn reference_blake3(path: &Path) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update_reader(File::open(path).unwrap()).unwrap();
    *h.finalize().as_bytes()
}

/// Sampled identity computed independently: size, then the three 4 MB windows.
fn reference_sampled(path: &Path) -> [u8; 32] {
    let mut f = File::open(path).unwrap();
    let len = f.metadata().unwrap().len();
    let mut h = blake3::Hasher::new();
    h.update(&len.to_le_bytes());
    let w = SAMPLE_WINDOW_BYTES;
    for off in [0, len / 2 - w / 2, len - w] {
        let mut buf = vec![0u8; w as usize];
        f.seek(SeekFrom::Start(off)).unwrap();
        f.read_exact(&mut buf).unwrap();
        h.update(&buf);
    }
    *h.finalize().as_bytes()
}

fn hash_path(path: &Path) -> ps_model::ContentId {
    let f = File::open(path).unwrap();
    let len = f.metadata().unwrap().len();
    identity_from_reader(f, len).unwrap()
}

#[test]
fn scenario_content_identity_full_hash_matches_reference() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.bin");
    let bytes = random_bytes(5, (5 * MB) as usize);
    fs::write(&path, &bytes).unwrap();

    let reference = *blake3::hash(&bytes).as_bytes();
    let from_reader = hash_path(&path);
    let from_buffer = identity_from_buffer(&bytes);
    assert_eq!(from_reader.digest, reference);
    assert_eq!(from_reader.kind, ContentIdKind::Full);
    assert_eq!(from_buffer, from_reader);
    let hex = from_reader.to_hex();
    assert_eq!(hex.len(), 64);
    assert!(
        hex.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert_eq!(hex, blake3::hash(&bytes).to_hex().as_str());
}

#[test]
fn scenario_content_identity_boundary_size_hashed_fully() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("boundary.bin");
    sparse_file(&path, FULL_IDENTITY_MAX_BYTES, 11, 16 * MB);
    let id = hash_path(&path);
    assert_eq!(FULL_IDENTITY_MAX_BYTES, 268_435_456);
    assert_eq!(id.kind, ContentIdKind::Full);
    assert_eq!(id.digest, reference_blake3(&path));

    // One byte more is sampled.
    OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(FULL_IDENTITY_MAX_BYTES + 1)
        .unwrap();
    assert_eq!(hash_path(&path).kind, ContentIdKind::Sampled);
}

#[test]
fn scenario_content_identity_renamed_copy_has_same_identity() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("IMG_0001.JPG");
    let b = dir.path().join("copy of holiday.jpg");
    fs::write(&a, random_bytes(3, 300_000)).unwrap();
    fs::copy(&a, &b).unwrap();
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    File::options()
        .write(true)
        .open(&b)
        .unwrap()
        .set_modified(old)
        .unwrap();
    assert_ne!(
        fs::metadata(&a).unwrap().modified().unwrap(),
        fs::metadata(&b).unwrap().modified().unwrap()
    );
    assert_eq!(hash_path(&a), hash_path(&b));
}

#[test]
fn scenario_content_identity_large_video_sampled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("video.mov");
    let len = 300 * MB;
    sparse_file(&path, len, 21, 3 * MB);
    let mut reader = CountingReader::new(File::open(&path).unwrap());
    let id = identity_from_reader(&mut reader, len).unwrap();
    assert_eq!(id.kind, ContentIdKind::Sampled);
    assert!(
        reader.bytes_read <= 12_582_912,
        "read {} bytes",
        reader.bytes_read
    );
    assert_eq!(id.digest, reference_sampled(&path));
}

#[test]
fn scenario_content_identity_middle_edit_changes_sampled_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("video.mp4");
    let len = 300 * MB;
    sparse_file(&path, len, 31, 5 * MB);
    let before = hash_path(&path);

    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    f.seek(SeekFrom::Start(len / 2)).unwrap();
    let mut b = [0u8; 1];
    f.read_exact(&mut b).unwrap();
    f.seek(SeekFrom::Start(len / 2)).unwrap();
    f.write_all(&[b[0] ^ 0xff]).unwrap();
    drop(f);

    let after = hash_path(&path);
    assert_eq!(after.kind, ContentIdKind::Sampled);
    assert_ne!(before, after);
}

#[test]
fn scenario_content_identity_size_is_part_of_sampled_identity() {
    let dir = tempfile::tempdir().unwrap();
    // Sparse files of zeros: every window is identical, only the size differs.
    let (a, b) = (dir.path().join("a.mkv"), dir.path().join("b.mkv"));
    File::create(&a).unwrap().set_len(300 * MB).unwrap();
    File::create(&b).unwrap().set_len(301 * MB).unwrap();
    let (ia, ib) = (hash_path(&a), hash_path(&b));
    assert_eq!(ia.kind, ContentIdKind::Sampled);
    assert_eq!(ib.kind, ContentIdKind::Sampled);
    assert_ne!(ia, ib);
}

#[test]
fn scenario_content_identity_one_read_per_file() {
    // A real JPEG padded to 12 MB with trailing data (decoders stop at EOI, as with camera trailers).
    let photo = common::photo(12, 1600, 1200);
    let mut bytes = common::encode_jpeg(&photo, 95);
    let jpeg_len = bytes.len();
    bytes.extend(random_bytes(12, (12 * MB) as usize - jpeg_len));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("IMG_1200.JPG");
    fs::write(&path, &bytes).unwrap();

    let mut reader = CountingReader::new(File::open(&path).unwrap());
    let len = fs::metadata(&path).unwrap().len();
    let read = read_for_analysis(&mut reader, len).unwrap();
    // Decode and perceptual-hash from the same buffer; nothing else touches the file.
    let buffer = read.bytes.as_deref().unwrap();
    let decoded = common::decode_jpeg(buffer);
    assert_eq!((decoded.width(), decoded.height()), (1600, 1200));
    let _ = common::hash_rgb(&decoded, 1);

    assert_eq!(reader.bytes_read, 12 * MB);
    assert_eq!(read.identity.digest, *blake3::hash(&bytes).as_bytes());
    assert_eq!(read.identity.kind, ContentIdKind::Full);
}

#[test]
fn scenario_content_identity_large_file_streamed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.tif");
    let len = 100 * MB;
    sparse_file(&path, len, 41, MB / 2);
    let mut reader = CountingReader::new(File::open(&path).unwrap());
    let read = read_for_analysis(&mut reader, len).unwrap();
    assert!(read.bytes.is_none(), "no whole-file buffer above 64 MB");
    assert!(len > ANALYSIS_BUFFER_MAX_BYTES);
    assert!(
        reader.max_request as u64 <= MB,
        "largest read {}",
        reader.max_request
    );
    assert_eq!(reader.bytes_read, len);
    assert_eq!(read.identity.kind, ContentIdKind::Full);
    assert_eq!(read.identity.digest, reference_blake3(&path));
}

#[test]
fn scenario_content_identity_hash_is_stable_across_runs() {
    const CHILD: &str = "PS_HASH_STABILITY_THREADS";
    const FIXTURE: &str = "PS_HASH_STABILITY_FIXTURE";
    if let Ok(threads) = std::env::var(CHILD) {
        // Child process: hash the fixture on `threads` threads at once; all must agree.
        let path = std::path::PathBuf::from(std::env::var(FIXTURE).unwrap());
        let threads: usize = threads.parse().unwrap();
        let ids: Vec<String> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..threads)
                .map(|_| s.spawn(|| hash_path(&path).to_hex()))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert!(ids.windows(2).all(|w| w[0] == w[1]));
        println!("identity={}", ids[0]);
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stable.bin");
    fs::write(&path, random_bytes(77, (3 * MB) as usize + 12_345)).unwrap();
    let run = |threads: usize| {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "scenario_content_identity_hash_is_stable_across_runs",
                "--nocapture",
                "--test-threads=1",
                "--format=pretty",
            ])
            .env(CHILD, threads.to_string())
            .env(FIXTURE, &path)
            .env("RAYON_NUM_THREADS", threads.to_string())
            .output()
            .unwrap();
        assert!(out.status.success(), "child failed: {out:?}");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .find_map(|l| {
                l.split_once("identity=")
                    .map(|(_, id)| id.trim().to_owned())
            })
            .unwrap()
    };
    let one = run(1);
    let many = run(8);
    assert_eq!(one, many);
    assert_eq!(one, hash_path(&path).to_hex());
}

/// tiff-support (ADR-0086): an identity fed during another sequential read equals the one-shot identity.
#[test]
fn scenario_content_identity_incremental_identity_equals_one_shot_identity() {
    let dir = tempfile::tempdir().unwrap();
    let small = dir.path().join("small.bin");
    fs::write(&small, random_bytes(21, 10 * MB as usize)).unwrap();
    let large = dir.path().join("large.bin");
    sparse_file(&large, 300 * MB + 12_345, 22, 7 * MB);
    let mut rng = ChaCha8Rng::seed_from_u64(23);
    for (path, kind) in [
        (&small, ContentIdKind::Full),
        (&large, ContentIdKind::Sampled),
    ] {
        let mut f = File::open(path).unwrap();
        let len = f.metadata().unwrap().len();
        let mut s = ps_hash::IdentityStream::new(len);
        assert_eq!(s.kind(), kind);
        let (mut pos, mut read) = (0u64, 0u64);
        let mut buf = vec![0u8; MB as usize];
        while pos < len {
            let piece = (1 + rng.next_u64() % MB).min(len - pos);
            if s.needs_any(pos, pos + piece) {
                f.seek(SeekFrom::Start(pos)).unwrap();
                f.read_exact(&mut buf[..piece as usize]).unwrap();
                s.update(pos, &buf[..piece as usize]).unwrap();
                read += piece;
            }
            pos += piece;
        }
        assert_eq!(s.next_needed(), None);
        assert_eq!(s.finish().unwrap(), hash_path(path), "{}", path.display());
        match kind {
            ContentIdKind::Full => assert_eq!(read, len),
            ContentIdKind::Sampled => {
                assert!(read <= 3 * SAMPLE_WINDOW_BYTES + 6 * MB, "read {read}")
            }
        }
    }
    // A needed byte skipped is an error, and an unfinished identity does not finish.
    let mut s = ps_hash::IdentityStream::new(10);
    assert!(s.update(3, &[0; 4]).is_err());
    s.update(0, &[0; 4]).unwrap();
    assert_eq!(s.next_needed(), Some(4));
    assert!(s.finish().is_err());
}

//! Hostile and extreme TIFFs (tiff-support gate review fixes, ADR-0088): decode work caps, cancellation,
//! overflow-free arithmetic, odd bit depths, previews of large images with alpha, and bounded metadata blocks.

use std::cell::Cell;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::rc::Rc;

use ps_media::decode::{DecodeError, decode_tiff_reader};
use ps_media::tiff_decode::{self, TIFF_MAX_WORK_BYTES, TiffDecodeStats};
use ps_media::{RgbImage, decode_for_analysis, probe_buffer};
use ps_model::Dimensions;

/// A little-endian TIFF built by hand: header, `data` at offset 8, then the out-of-line arrays and IFD0.
struct Raw {
    width: u32,
    height: u32,
    bits: u16,
    samples: u16,
    photometric: u16,
    compression: u16,
    /// `Some((tile_width, tile_length))` for tiles, else one strip of `rows_per_strip` rows per chunk.
    tiles: Option<(u32, u32)>,
    rows_per_strip: u32,
    /// Chunk offsets relative to the start of `data`.
    offsets: Vec<u32>,
    counts: Vec<u32>,
    data: Vec<u8>,
    /// Extra entries `(tag, type, count, value)` (inline values only).
    extra: Vec<(u16, u16, u32, u32)>,
}

impl Raw {
    fn grey(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            bits: 8,
            samples: 1,
            photometric: 1,
            compression: 1,
            tiles: None,
            rows_per_strip: height,
            offsets: vec![0],
            counts: vec![width.saturating_mul(height)],
            data: Vec::new(),
            extra: Vec::new(),
        }
    }

    fn build(&self) -> Vec<u8> {
        chain(&[self])
    }

    /// Append this directory's data, arrays and IFD to `out`; returns the position of its next-IFD field.
    fn append(&self, out: &mut Vec<u8>, prev_next: usize) -> usize {
        let base = out.len() as u32;
        out.extend_from_slice(&self.data);
        let align = |out: &mut Vec<u8>| {
            if out.len() % 2 == 1 {
                out.push(0);
            }
        };
        let array = |out: &mut Vec<u8>, v: &[u32]| -> u32 {
            if v.len() == 1 {
                return v[0];
            }
            align(out);
            let at = out.len() as u32;
            for x in v {
                out.extend_from_slice(&x.to_le_bytes());
            }
            at
        };
        let offsets: Vec<u32> = self.offsets.iter().map(|o| o + base).collect();
        let bits_at = if self.samples > 2 {
            align(out);
            let at = out.len() as u32;
            for _ in 0..self.samples {
                out.extend_from_slice(&self.bits.to_le_bytes());
            }
            at
        } else {
            u32::from(self.bits)
        };
        let off_at = array(out, &offsets);
        let cnt_at = array(out, &self.counts);
        let mut e: Vec<(u16, u16, u32, u32)> = vec![
            (0x0100, 4, 1, self.width),
            (0x0101, 4, 1, self.height),
            (0x0102, 3, u32::from(self.samples), bits_at),
            (0x0103, 3, 1, u32::from(self.compression)),
            (0x0106, 3, 1, u32::from(self.photometric)),
            (0x0115, 3, 1, u32::from(self.samples)),
        ];
        let n = self.offsets.len() as u32;
        match self.tiles {
            Some((tw, th)) => {
                e.push((0x0142, 4, 1, tw));
                e.push((0x0143, 4, 1, th));
                e.push((0x0144, 4, n, off_at));
                e.push((0x0145, 4, n, cnt_at));
            }
            None => {
                e.push((0x0111, 4, n, off_at));
                e.push((0x0116, 4, 1, self.rows_per_strip));
                e.push((0x0117, 4, n, cnt_at));
            }
        }
        e.extend_from_slice(&self.extra);
        e.sort_by_key(|x| x.0);
        align(out);
        let ifd = out.len() as u32;
        out[prev_next..prev_next + 4].copy_from_slice(&ifd.to_le_bytes());
        out.extend_from_slice(&(e.len() as u16).to_le_bytes());
        for (tag, typ, count, value) in e {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&typ.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            if typ == 3 && count <= 2 && (tag != 0x0102 || self.samples == 1) {
                out.extend_from_slice(&(value as u16).to_le_bytes());
                out.extend_from_slice(&[0, 0]);
            } else {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
        let next = out.len();
        out.extend_from_slice(&0u32.to_le_bytes());
        next
    }
}

/// A TIFF whose main chain holds `dirs` in order.
fn chain(dirs: &[&Raw]) -> Vec<u8> {
    let mut out = b"II\x2A\x00\0\0\0\0".to_vec();
    let mut next = 4;
    for d in dirs {
        next = d.append(&mut out, next);
    }
    out
}

/// The reviewer's bomb: 81920x81920 8-bit grey in 400 uncompressed 4096x4096 tiles that all share one 16 MB
/// data block (a 16.8 MB file claiming 6.7 gigapixels).
fn shared_tile_bomb() -> Vec<u8> {
    let tile = 4096u32;
    Raw {
        tiles: Some((tile, tile)),
        offsets: vec![0; 400],
        counts: vec![tile * tile; 400],
        data: vec![0; (tile * tile) as usize],
        ..Raw::grey(81920, 81920)
    }
    .build()
}

/// Bytes read through a reader.
struct Counting<R> {
    inner: R,
    bytes: Rc<Cell<u64>>,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes.set(self.bytes.get() + n as u64);
        Ok(n)
    }
}

impl<R: Seek> Seek for Counting<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// Decode `t` with a stop check that counts its calls (one per strip or tile) and stops at call `stop_at`.
fn decode_counted(
    t: &[u8],
    min_long: u32,
    stop_at: Option<u32>,
) -> (Result<RgbImage, DecodeError>, TiffDecodeStats, u32, u64) {
    let calls = Cell::new(0u32);
    let bytes = Rc::new(Cell::new(0u64));
    let reader = Counting {
        inner: Cursor::new(t),
        bytes: Rc::clone(&bytes),
    };
    let stop = || {
        calls.set(calls.get() + 1);
        stop_at.is_some_and(|n| calls.get() >= n)
    };
    let (r, stats) = tiff_decode::decode_with(reader, min_long, &stop);
    (r, stats, calls.get(), bytes.get())
}

#[test]
fn scenario_thumbnails_tiff_decode_work_capped() {
    let t = shared_tile_bomb();
    let (r, stats, chunks, bytes) = decode_counted(&t, 512, None);
    let err = r.expect_err("refused");
    assert!(matches!(err, DecodeError::TooLarge(_)), "{err:?}");
    assert!(err.to_string().contains("81920x81920"), "{err}");
    // Refused from the header: no strip or tile decoded, nothing but the directory read.
    assert_eq!(chunks, 0);
    assert_eq!(stats.chunks, 0);
    assert!(bytes < 64 * 1024, "{bytes} bytes read");
    assert!(stats.work_bytes > TIFF_MAX_WORK_BYTES);
    assert_eq!(stats.image_dims, (81920, 81920));
    // In memory too (the `image` fallback refuses it as well).
    let p = probe_buffer(&t, None);
    assert!(decode_for_analysis(&t, &p, &ps_media::Decoders::none()).is_err());
    // A block shared by many chunks counts once per chunk: 200 tiles of 4096x4096 x 1 byte (3.1 GB decoded)
    // pass both caps (the decode starts, and stops at the first check here); the same 200 tiles each claiming
    // a 32 MB compressed block (6.3 GB to read) are refused before any is read.
    let shared = |count: u32| {
        let tile = 4096u32;
        Raw {
            tiles: Some((tile, tile)),
            offsets: vec![0; 200],
            counts: vec![count; 200],
            data: vec![0; count as usize],
            ..Raw::grey(tile * 20, tile * 10)
        }
        .build()
    };
    let (r, _, chunks, _) = decode_counted(&shared(4096 * 4096), 512, Some(1));
    assert!(matches!(r, Err(DecodeError::Cancelled)), "{r:?}");
    assert_eq!(chunks, 1);
    let (r, stats, chunks, _) = decode_counted(&shared(32 << 20), 512, Some(1));
    let err = r.expect_err("refused");
    assert!(err.to_string().contains("compressed"), "{err}");
    assert!(stats.work_bytes <= TIFF_MAX_WORK_BYTES);
    assert_eq!((chunks, stats.chunks), (0, 0));
}

#[test]
fn scenario_thumbnails_reduced_resolution_level_stands_in_for_a_huge_first_image() {
    let tile = 4096u32;
    let huge = Raw {
        tiles: Some((tile, tile)),
        offsets: vec![0; 400],
        counts: vec![tile * tile; 400],
        data: vec![0; (tile * tile) as usize],
        ..Raw::grey(81920, 81920)
    };
    let level = Raw {
        data: vec![150; 640 * 640],
        extra: vec![(0x00FE, 4, 1, 1)],
        ..Raw::grey(640, 640)
    };
    let t = chain(&[&huge, &level]);
    // A 2048 px preview: no level reaches 2048 px, and the first image is above the cap; the 640 px level
    // decodes instead of nothing.
    let (r, stats, _, _) = decode_counted(&t, 2048, None);
    let img = r.expect("decode");
    assert_eq!((stats.level, stats.level_dims), (1, (640, 640)));
    assert_eq!((img.width, img.height), (640, 640));
    assert!(img.data.iter().all(|v| *v == 150));
}

#[test]
fn scenario_thumbnails_tiff_decode_stops_when_cancelled() {
    // 4096x4096 grey in 256 strips of 16 rows.
    let (w, h) = (4096u32, 4096u32);
    let t = Raw {
        rows_per_strip: 16,
        offsets: (0..256).map(|i| i * w * 16).collect(),
        counts: vec![w * 16; 256],
        data: vec![99; (w * h) as usize],
        ..Raw::grey(w, h)
    }
    .build();
    let (r, stats, calls, _) = decode_counted(&t, 512, None);
    r.expect("decode");
    assert_eq!((calls, stats.chunks), (256, 256));
    // The stop check fires at the 11th strip: ten were decoded, none after it.
    let (r, stats, calls, _) = decode_counted(&t, 512, Some(11));
    assert!(matches!(r, Err(DecodeError::Cancelled)), "{r:?}");
    assert_eq!((calls, stats.chunks), (11, 10));
    // A file in memory is not decoded again by the fallback after a cancel.
    let p = probe_buffer(&t, None);
    let n = Cell::new(0u32);
    let stop = || {
        n.set(n.get() + 1);
        n.get() > 3
    };
    let r = ps_media::analyze_still_with(&t, &p, &ps_media::Decoders::none(), &stop);
    assert!(matches!(r, Err(DecodeError::Cancelled)), "{:?}", r.err());
    assert_eq!(n.get(), 4);
}

#[test]
fn extreme_aspect_tiled_image_decodes() {
    // 33,554,432 x 1 in 32 tiles of 1,048,576 x 16 sharing one block of value 200.
    let (tw, th) = (1_048_576u32, 16u32);
    let t = Raw {
        tiles: Some((tw, th)),
        offsets: vec![0; 32],
        counts: vec![tw * th; 32],
        data: vec![200; (tw * th) as usize],
        ..Raw::grey(33_554_432, 1)
    }
    .build();
    let p = probe_buffer(&t, None);
    let (d, _) = decode_tiff_reader(Cursor::new(&t), &p, 512).expect("decode");
    assert!(
        d.image.width >= 512 && d.image.height == 1,
        "{:?}",
        d.image.dims()
    );
    assert!(d.image.data.iter().all(|v| *v == 200), "mean kept");
}

#[test]
fn huge_box_factor_keeps_the_mean() {
    // 5000x4000 of value 200 decoded to a 1 px long edge: 20 M samples per output pixel.
    let t = Raw {
        data: vec![200; 5000 * 4000],
        ..Raw::grey(5000, 4000)
    }
    .build();
    let p = probe_buffer(&t, None);
    let (d, _) = decode_tiff_reader(Cursor::new(&t), &p, 1).expect("decode");
    assert!(
        d.image.data.iter().all(|v| *v == 200),
        "{:?}",
        &d.image.data[..3]
    );
}

#[test]
fn tile_count_overflow_refused() {
    let t = Raw {
        tiles: Some((16, 16)),
        offsets: vec![0],
        counts: vec![256],
        data: vec![0; 256],
        ..Raw::grey(u32::MAX - 15, u32::MAX - 15)
    }
    .build();
    let p = probe_buffer(&t, None);
    assert!(decode_tiff_reader(Cursor::new(&t), &p, 512).is_err());
}

/// Pack `values` (each `bits` wide) MSB first, rows padded to a byte.
fn pack(values: &[u8], bits: u32, row: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for r in values.chunks(row) {
        let mut acc = 0u32;
        let mut n = 0;
        for v in r {
            acc = (acc << bits) | u32::from(*v);
            n += bits;
            while n >= 8 {
                out.push((acc >> (n - 8)) as u8);
                n -= 8;
                acc &= (1 << n) - 1;
            }
        }
        if n > 0 {
            out.push((acc << (8 - n)) as u8);
        }
    }
    out
}

#[test]
fn odd_bit_depths_decode_or_are_refused() {
    for bits in 1..=7u32 {
        let max = (1u32 << bits) - 1;
        let (w, h) = (13u32, 4u32);
        let values: Vec<u8> = (0..w * h).map(|i| (i % (max + 1)) as u8).collect();
        let data = pack(&values, bits, w as usize);
        let t = Raw {
            bits: bits as u16,
            counts: vec![data.len() as u32],
            data,
            ..Raw::grey(w, h)
        }
        .build();
        let p = probe_buffer(&t, None);
        match decode_tiff_reader(Cursor::new(&t), &p, w) {
            Ok((d, stats)) => {
                println!("{bits}-bit samples decoded");
                assert_eq!(stats.factor, 1, "{bits} bits");
                for (i, v) in values.iter().enumerate() {
                    let want = ((f64::from(*v) / f64::from(max)) * 255.0 + 0.5) as u8;
                    assert_eq!(d.image.data[i * 3], want, "{bits} bits, sample {i}");
                }
            }
            Err(e) => assert!(
                matches!(e, DecodeError::Unsupported(_)),
                "{bits} bits: {e:?}"
            ),
        }
    }
}

#[test]
fn scenario_thumbnails_large_preview_of_a_five_channel_tiff_decodes() {
    // 4095x4095 CMYK + alpha: the 2048 px preview at factor 1 would need a 335 MB accumulator.
    let (w, h) = (4095u32, 4095u32);
    let t = Raw {
        samples: 5,
        photometric: 5,
        rows_per_strip: 16,
        offsets: (0..h.div_ceil(16)).map(|i| i * w * 16 * 5).collect(),
        counts: (0..h.div_ceil(16))
            .map(|i| w * 5 * 16.min(h - i * 16))
            .collect(),
        data: vec![0; (w * h * 5) as usize],
        extra: vec![(0x0152, 3, 1, 2)],
        ..Raw::grey(w, h)
    }
    .build();
    let p = probe_buffer(&t, None);
    let (d, stats) = decode_tiff_reader(Cursor::new(&t), &p, 2048).expect("decode");
    assert_eq!(stats.factor, 2);
    assert_eq!(d.image.dims(), Dimensions::new(2047, 2047));
}

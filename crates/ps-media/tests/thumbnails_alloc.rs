//! DCT-scaled decode allocates no full-resolution buffer (peak-allocation tracking needs its own binary).
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use ps_fixtures::media as fx;
use ps_model::Dimensions;

struct Tracking;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards to the system allocator; only counts bytes.
unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            let now = LIVE.fetch_add(l.size(), Ordering::Relaxed) + l.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static GLOBAL: Tracking = Tracking;

#[test]
fn scenario_thumbnails_dct_scaled_decode_chosen() {
    let j = fx::jpeg(6000, 4000, 1, 85);
    let p = ps_media::probe_bytes(&j, j.len() as u64, None);
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let d = ps_media::decode_for_analysis(&j, &p, &ps_media::Decoders::none()).expect("decode");
    let peak = PEAK.load(Ordering::Relaxed) - base;
    assert_eq!(d.image.dims(), Dimensions::new(750, 500));
    assert_eq!(d.scale_denom, 8);
    assert_eq!(d.image.data.len(), 750 * 500 * 3);
    let full = 6000 * 4000 * 3;
    assert!(
        peak < full / 4,
        "peak {peak} bytes during decode (full-res RGB would be {full})"
    );
    eprintln!("peak decode allocation: {peak} bytes");
}

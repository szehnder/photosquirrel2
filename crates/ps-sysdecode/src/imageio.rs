//! macOS: Apple ImageIO (ADR-0089, design D2). `CGImageSourceCreateWithData` over a `CFData` that borrows the
//! buffer, then `CGImageSourceCreateThumbnailAtIndex` with the transform applied (so `applies_transforms()` is
//! true, confirmed by the self-test) and a maximum pixel size of `max(min_long, 512)`; the `CGImage` is drawn
//! into an sRGB bitmap context (RGBX, 8 bits) and copied to RGB. ImageIO decodes HEIC on every macOS since
//! 10.13, on arm64 and x86_64. Every CoreFoundation, CoreGraphics and ImageIO function used is a plain C
//! function declared here with its documented signature.

#![allow(unsafe_code)]

use std::ffi::c_void;

use ps_media::StillDecoder;
use ps_media::decode::{DecodeError, RgbImage};

use crate::{MAX_PIXELS, PlatformProbe};

type CFTypeRef = *const c_void;
type CFIndex = isize;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

/// `CFDictionaryKeyCallBacks` / `CFDictionaryValueCallBacks`: only their addresses are passed.
#[repr(C)]
struct CallBacks {
    _private: [u8; 0],
}

const K_CF_NUMBER_SINT32: CFIndex = 3;
/// `kCGImageAlphaNoneSkipLast` with the default byte order: R, G, B, X in memory.
const K_CG_IMAGE_ALPHA_NONE_SKIP_LAST: u32 = 5;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFAllocatorNull: CFTypeRef;
    static kCFBooleanTrue: CFTypeRef;
    static kCFTypeDictionaryKeyCallBacks: CallBacks;
    static kCFTypeDictionaryValueCallBacks: CallBacks;
    fn CFDataCreateWithBytesNoCopy(
        allocator: CFTypeRef,
        bytes: *const u8,
        length: CFIndex,
        deallocator: CFTypeRef,
    ) -> CFTypeRef;
    fn CFDictionaryCreate(
        allocator: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        count: CFIndex,
        key_callbacks: *const CallBacks,
        value_callbacks: *const CallBacks,
    ) -> CFTypeRef;
    fn CFNumberCreate(allocator: CFTypeRef, kind: CFIndex, value: *const c_void) -> CFTypeRef;
    fn CFRelease(cf: CFTypeRef);
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGColorSpaceSRGB: CFTypeRef;
    fn CGImageGetWidth(image: CFTypeRef) -> usize;
    fn CGImageGetHeight(image: CFTypeRef) -> usize;
    fn CGColorSpaceCreateWithName(name: CFTypeRef) -> CFTypeRef;
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: CFTypeRef,
        bitmap_info: u32,
    ) -> CFTypeRef;
    fn CGContextDrawImage(c: CFTypeRef, rect: CGRect, image: CFTypeRef);
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    static kCGImageSourceCreateThumbnailFromImageAlways: CFTypeRef;
    static kCGImageSourceCreateThumbnailWithTransform: CFTypeRef;
    static kCGImageSourceThumbnailMaxPixelSize: CFTypeRef;
    static kCGImageSourceShouldCacheImmediately: CFTypeRef;
    fn CGImageSourceCreateWithData(data: CFTypeRef, options: CFTypeRef) -> CFTypeRef;
    fn CGImageSourceGetCount(source: CFTypeRef) -> usize;
    fn CGImageSourceCreateThumbnailAtIndex(
        source: CFTypeRef,
        index: usize,
        options: CFTypeRef,
    ) -> CFTypeRef;
}

/// Releases a CoreFoundation object (any CF or CG type) on every exit path.
struct Cf(CFTypeRef);

impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the object came from a Create function (we own one reference) and is released once.
            unsafe { CFRelease(self.0) };
        }
    }
}

/// ImageIO as a [`StillDecoder`].
pub struct ImageIo;

impl StillDecoder for ImageIo {
    fn name(&self) -> &str {
        "ImageIO"
    }

    fn decode(&self, bytes: &[u8], min_long: u32) -> Result<RgbImage, DecodeError> {
        decode(bytes, min_long.max(512)).map_err(DecodeError::Failed)
    }

    fn applies_transforms(&self) -> bool {
        true
    }
}

fn decode(bytes: &[u8], max_pixel_size: u32) -> Result<RgbImage, String> {
    // SAFETY (whole block): `bytes` outlives every object created here (the CFData borrows it with the null
    // deallocator and is released before return); every Create result is checked for NULL and owned by a `Cf`
    // guard; the bitmap context writes exactly `w * h * 4` bytes into `buf`, which it was created over.
    unsafe {
        let data = Cf(CFDataCreateWithBytesNoCopy(
            std::ptr::null(),
            bytes.as_ptr(),
            bytes.len() as CFIndex,
            kCFAllocatorNull,
        ));
        if data.0.is_null() {
            return Err("CFDataCreateWithBytesNoCopy failed".into());
        }
        let source = Cf(CGImageSourceCreateWithData(data.0, std::ptr::null()));
        if source.0.is_null() || CGImageSourceGetCount(source.0) == 0 {
            return Err("ImageIO cannot read this file".into());
        }
        let size = max_pixel_size as i32;
        let number = Cf(CFNumberCreate(
            std::ptr::null(),
            K_CF_NUMBER_SINT32,
            (&size as *const i32).cast(),
        ));
        let keys = [
            kCGImageSourceCreateThumbnailFromImageAlways,
            kCGImageSourceCreateThumbnailWithTransform,
            kCGImageSourceThumbnailMaxPixelSize,
            kCGImageSourceShouldCacheImmediately,
        ];
        let values = [kCFBooleanTrue, kCFBooleanTrue, number.0, kCFBooleanTrue];
        let options = Cf(CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            keys.len() as CFIndex,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        ));
        if options.0.is_null() || number.0.is_null() {
            return Err("CFDictionaryCreate failed".into());
        }
        let image = Cf(CGImageSourceCreateThumbnailAtIndex(source.0, 0, options.0));
        if image.0.is_null() {
            return Err("ImageIO could not decode the image".into());
        }
        let (w, h) = (CGImageGetWidth(image.0), CGImageGetHeight(image.0));
        if w == 0 || h == 0 || (w as u64) * (h as u64) > MAX_PIXELS {
            return Err(format!("unsupported image size {w}x{h}"));
        }
        let space = Cf(CGColorSpaceCreateWithName(kCGColorSpaceSRGB));
        if space.0.is_null() {
            return Err("no sRGB colour space".into());
        }
        let mut buf = vec![0u8; w * h * 4];
        let ctx = Cf(CGBitmapContextCreate(
            buf.as_mut_ptr().cast(),
            w,
            h,
            8,
            w * 4,
            space.0,
            K_CG_IMAGE_ALPHA_NONE_SKIP_LAST,
        ));
        if ctx.0.is_null() {
            return Err("CGBitmapContextCreate failed".into());
        }
        CGContextDrawImage(
            ctx.0,
            CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize {
                    width: w as f64,
                    height: h as f64,
                },
            },
            image.0,
        );
        drop(ctx);
        let mut rgb = Vec::with_capacity(w * h * 3);
        for px in buf.as_chunks::<4>().0 {
            rgb.extend_from_slice(&px[..3]);
        }
        Ok(RgbImage {
            width: w as u32,
            height: h as u32,
            data: rgb,
        })
    }
}

/// ImageIO is always present on macOS 10.13 and later (the app's floor is macOS 11, ADR-0030).
pub fn probe() -> PlatformProbe {
    PlatformProbe {
        backend_name: "ImageIO".into(),
        version: None,
        decoder: Some(std::sync::Arc::new(ImageIo)),
        finding: None,
    }
}

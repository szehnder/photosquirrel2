//! The system libheif, loaded at run time with `libloading` (ADR-0089, design D3): `libheif.so.1` (the SONAME
//! since 1.0), else `libheif.so`, or the `PHOTOSQUIRREL_LIBHEIF` path. Symbols are resolved once into a table
//! of hand-written C signatures (no bindgen, no `-sys` crate); the library is never unloaded. Version 1.12 or
//! newer and an HEVC decoder are required. Decoding uses only structures that are the same in every version
//! since 1.12 (`heif_error` by value, `NULL` decoding options: transformations applied).

#![allow(unsafe_code)]

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use ps_media::StillDecoder;
use ps_media::decode::{DecodeError, RgbImage};

use crate::guidance::{Finding, LIBHEIF_MIN};
use crate::{MAX_PIXELS, PlatformProbe};

/// What detection and decoding need from libheif, so tests can supply a fake table.
pub trait HeifLib: Send + Sync {
    /// (major, minor, maintenance).
    fn version(&self) -> (u32, u32, u32);
    /// `heif_have_decoder_for_format(heif_compression_HEVC)`.
    fn has_hevc(&self) -> bool;
    /// Decode the primary image to interleaved 8-bit RGB, transformations applied.
    fn decode(&self, bytes: &[u8]) -> Result<RgbImage, String>;
}

/// `1.17.6`.
pub fn version_text((a, b, c): (u32, u32, u32)) -> String {
    format!("{a}.{b}.{c}")
}

/// The checks of design D3 on a loaded library: version, then HEVC. `Ok` hands the decoder to the self-test.
pub fn probe_lib(lib: Result<Arc<dyn HeifLib>, String>) -> PlatformProbe {
    let lib = match lib {
        Ok(l) => l,
        Err(e) => {
            log::info!("libheif not loaded: {e}");
            return PlatformProbe {
                backend_name: "libheif".into(),
                version: None,
                decoder: None,
                finding: Some(Finding::NoLibrary),
            };
        }
    };
    let v = lib.version();
    let version = version_text(v);
    if (v.0, v.1) < LIBHEIF_MIN {
        return PlatformProbe {
            backend_name: "libheif".into(),
            version: Some(version.clone()),
            decoder: None,
            finding: Some(Finding::TooOld { version }),
        };
    }
    if !lib.has_hevc() {
        return PlatformProbe {
            backend_name: "libheif".into(),
            version: Some(version.clone()),
            decoder: None,
            finding: Some(Finding::NoHevc { version }),
        };
    }
    PlatformProbe {
        backend_name: format!("libheif {version}"),
        version: Some(version),
        decoder: Some(Arc::new(LibheifDecoder {
            name: "libheif".into(),
            lib,
        })),
        finding: None,
    }
}

/// Load the system libheif (or `path`) and probe it.
pub fn probe(path: Option<&Path>) -> PlatformProbe {
    probe_lib(load(path).map(|l| l as Arc<dyn HeifLib>))
}

/// A [`StillDecoder`] over a libheif table.
pub struct LibheifDecoder {
    name: String,
    lib: Arc<dyn HeifLib>,
}

impl StillDecoder for LibheifDecoder {
    fn name(&self) -> &str {
        &self.name
    }

    fn decode(&self, bytes: &[u8], _min_long: u32) -> Result<RgbImage, DecodeError> {
        self.lib.decode(bytes).map_err(DecodeError::Failed)
    }

    fn applies_transforms(&self) -> bool {
        true
    }
}

// ---- the real library ----

#[repr(C)]
#[derive(Clone, Copy)]
struct HeifError {
    code: c_int,
    subcode: c_int,
    message: *const c_char,
}

impl HeifError {
    fn check(self, what: &str) -> Result<(), String> {
        if self.code == 0 {
            return Ok(());
        }
        let msg = if self.message.is_null() {
            String::new()
        } else {
            // SAFETY: libheif documents `message` as a valid NUL-terminated string (static or owned by the
            // context, which is alive here).
            unsafe { CStr::from_ptr(self.message) }
                .to_string_lossy()
                .into_owned()
        };
        Err(format!("{what}: {msg} ({}/{})", self.code, self.subcode))
    }
}

type FnVersion = unsafe extern "C" fn() -> u32;
type FnInit = unsafe extern "C" fn(*mut c_void) -> HeifError;
type FnCtxAlloc = unsafe extern "C" fn() -> *mut c_void;
type FnReadMem =
    unsafe extern "C" fn(*mut c_void, *const c_void, usize, *const c_void) -> HeifError;
type FnSetLimit = unsafe extern "C" fn(*mut c_void, c_int);
type FnPrimary = unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> HeifError;
type FnHandleDim = unsafe extern "C" fn(*const c_void) -> c_int;
type FnRelease = unsafe extern "C" fn(*const c_void);
type FnDecode =
    unsafe extern "C" fn(*const c_void, *mut *mut c_void, c_int, c_int, *const c_void) -> HeifError;
type FnPlane = unsafe extern "C" fn(*const c_void, c_int, *mut c_int) -> *const u8;
type FnImageDim = unsafe extern "C" fn(*const c_void, c_int) -> c_int;
type FnHaveDecoder = unsafe extern "C" fn(c_int) -> c_int;

const HEIF_COMPRESSION_HEVC: c_int = 1;
const HEIF_COLORSPACE_RGB: c_int = 1;
const HEIF_CHROMA_INTERLEAVED_RGB: c_int = 10;
const HEIF_CHANNEL_INTERLEAVED: c_int = 10;

/// The resolved function table. The library stays loaded for the life of the process.
pub struct SystemLibheif {
    _lib: libloading::Library,
    version: FnVersion,
    ctx_alloc: FnCtxAlloc,
    ctx_free: FnRelease,
    read_mem: FnReadMem,
    set_limit: FnSetLimit,
    primary: FnPrimary,
    handle_width: FnHandleDim,
    handle_height: FnHandleDim,
    handle_release: FnRelease,
    decode_image: FnDecode,
    plane: FnPlane,
    image_width: FnImageDim,
    image_height: FnImageDim,
    image_release: FnRelease,
    have_decoder: FnHaveDecoder,
}

// SAFETY: the table holds plain function pointers into a library that is never unloaded; libheif functions
// are safe to call from several threads as long as each context is used by one thread at a time, which
// `decode` guarantees (one context per call).
unsafe impl Send for SystemLibheif {}
// SAFETY: as above; nothing in the table is mutated after loading.
unsafe impl Sync for SystemLibheif {}

/// Loaded libraries by path (successes only), never unloaded.
fn cache() -> &'static Mutex<HashMap<PathBuf, Arc<SystemLibheif>>> {
    static C: OnceLock<Mutex<HashMap<PathBuf, Arc<SystemLibheif>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Open `path`, else `libheif.so.1`, else `libheif.so`, resolve the table and call `heif_init` when present
/// (1.13+; it loads the decoder plugins with which current distributions ship HEVC).
pub fn load(path: Option<&Path>) -> Result<Arc<SystemLibheif>, String> {
    let names: Vec<PathBuf> = match path {
        Some(p) => vec![p.to_path_buf()],
        None => vec!["libheif.so.1".into(), "libheif.so".into()],
    };
    let mut errors = Vec::new();
    for n in names {
        if let Some(l) = cache().lock().unwrap_or_else(|e| e.into_inner()).get(&n) {
            return Ok(l.clone());
        }
        match open(&n) {
            Ok(l) => {
                let l = Arc::new(l);
                cache()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(n, l.clone());
                return Ok(l);
            }
            Err(e) => errors.push(format!("{}: {e}", n.display())),
        }
    }
    Err(errors.join("; "))
}

fn open(path: &Path) -> Result<SystemLibheif, String> {
    // SAFETY: loading libheif runs its initialisers, which only set up libheif's own state; the library is a
    // well-known system component, and the path is the system's or the user's explicit override.
    let lib = unsafe { libloading::Library::new(path) }.map_err(|e| e.to_string())?;
    macro_rules! sym {
        ($name:literal, $ty:ty) => {{
            // SAFETY: the symbol is looked up by its libheif name and given its C signature from the libheif
            // 1.12 headers (checked in task 5.5); the pointer stays valid while `lib` is loaded (always).
            let s: libloading::Symbol<'_, $ty> = unsafe { lib.get(concat!($name, "\0").as_bytes()) }
                .map_err(|e| format!("{}: {e}", $name))?;
            *s
        }};
    }
    let table = SystemLibheif {
        version: sym!("heif_get_version_number", FnVersion),
        ctx_alloc: sym!("heif_context_alloc", FnCtxAlloc),
        ctx_free: sym!("heif_context_free", FnRelease),
        read_mem: sym!("heif_context_read_from_memory_without_copy", FnReadMem),
        set_limit: sym!("heif_context_set_maximum_image_size_limit", FnSetLimit),
        primary: sym!("heif_context_get_primary_image_handle", FnPrimary),
        handle_width: sym!("heif_image_handle_get_width", FnHandleDim),
        handle_height: sym!("heif_image_handle_get_height", FnHandleDim),
        handle_release: sym!("heif_image_handle_release", FnRelease),
        decode_image: sym!("heif_decode_image", FnDecode),
        plane: sym!("heif_image_get_plane_readonly", FnPlane),
        image_width: sym!("heif_image_get_width", FnImageDim),
        image_height: sym!("heif_image_get_height", FnImageDim),
        image_release: sym!("heif_image_release", FnRelease),
        have_decoder: sym!("heif_have_decoder_for_format", FnHaveDecoder),
        _lib: lib,
    };
    // SAFETY: `heif_init` (1.13+) takes optional init parameters (NULL: defaults) and is reference counted;
    // it is called once per loaded library and never paired with `heif_deinit` (the library stays loaded).
    unsafe {
        if let Ok(init) = table._lib.get::<FnInit>(b"heif_init\0")
            && let Err(e) = (*init)(std::ptr::null_mut()).check("heif_init")
        {
            log::warn!("{e}");
        }
    }
    Ok(table)
}

/// The argument of `heif_context_set_maximum_image_size_limit` for libheif `version`. Up to 1.17 it is a limit
/// on each side (`width > limit || height > limit`, some paths `>=`), so the square root of the area cap would
/// refuse panoramas wider than 16,384 px: there it gets the area cap itself, which no side of an image within
/// the area can reach, and our own check of the primary image's size enforces the area before decoding. From
/// 1.18 it is the square root of a maximum area (`limit * limit` as 64-bit), so it gets the root of the cap.
pub fn size_limit_arg(version: (u32, u32, u32)) -> c_int {
    if (version.0, version.1) >= (1, 18) {
        (MAX_PIXELS as f64).sqrt() as c_int
    } else {
        c_int::try_from(MAX_PIXELS).unwrap_or(c_int::MAX)
    }
}

/// Frees a context, handle or image on every exit path.
struct Guard(*const c_void, unsafe extern "C" fn(*const c_void));

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from the matching libheif allocator and is released exactly once.
            unsafe { (self.1)(self.0) };
        }
    }
}

impl HeifLib for SystemLibheif {
    fn version(&self) -> (u32, u32, u32) {
        // SAFETY: no arguments; returns a packed number.
        let v = unsafe { (self.version)() };
        (v >> 24, (v >> 16) & 0xFF, (v >> 8) & 0xFF)
    }

    fn has_hevc(&self) -> bool {
        // SAFETY: a plain enum argument.
        unsafe { (self.have_decoder)(HEIF_COMPRESSION_HEVC) != 0 }
    }

    fn decode(&self, bytes: &[u8]) -> Result<RgbImage, String> {
        // SAFETY (whole block): every pointer passed comes from libheif in this call or is `bytes`, which
        // outlives the context (read "without copy", freed before return); outputs are checked for NULL;
        // the plane is read only within height * stride with stride >= width * 3, as libheif guarantees for
        // interleaved RGB.
        unsafe {
            let ctx = (self.ctx_alloc)();
            if ctx.is_null() {
                return Err("heif_context_alloc failed".into());
            }
            let _ctx = Guard(ctx, self.ctx_free);
            // libheif's own limit, by version; the area cap is enforced below from the handle's size.
            (self.set_limit)(ctx, size_limit_arg(self.version()));
            (self.read_mem)(ctx, bytes.as_ptr().cast(), bytes.len(), std::ptr::null())
                .check("read")?;
            let mut handle = std::ptr::null_mut();
            (self.primary)(ctx, &mut handle).check("primary image")?;
            if handle.is_null() {
                return Err("no primary image".into());
            }
            let _handle = Guard(handle, self.handle_release);
            // The area cap, from the primary image's declared size, before anything is decoded.
            let (w, h) = ((self.handle_width)(handle), (self.handle_height)(handle));
            if w <= 0 || h <= 0 || (w as u64) * (h as u64) > MAX_PIXELS {
                return Err(format!(
                    "unsupported image size {w}x{h} (over the decode cap)"
                ));
            }
            let mut img = std::ptr::null_mut();
            (self.decode_image)(
                handle,
                &mut img,
                HEIF_COLORSPACE_RGB,
                HEIF_CHROMA_INTERLEAVED_RGB,
                std::ptr::null(),
            )
            .check("decode")?;
            if img.is_null() {
                return Err("decode returned no image".into());
            }
            let _img = Guard(img, self.image_release);
            let iw = (self.image_width)(img, HEIF_CHANNEL_INTERLEAVED);
            let ih = (self.image_height)(img, HEIF_CHANNEL_INTERLEAVED);
            let mut stride: c_int = 0;
            let plane = (self.plane)(img, HEIF_CHANNEL_INTERLEAVED, &mut stride);
            if plane.is_null() || iw <= 0 || ih <= 0 || stride < iw * 3 {
                return Err("no interleaved RGB plane".into());
            }
            if (iw as u64) * (ih as u64) > MAX_PIXELS {
                return Err(format!("unsupported image size {iw}x{ih}"));
            }
            let (iw, ih, stride) = (iw as usize, ih as usize, stride as usize);
            let mut data = Vec::with_capacity(iw * ih * 3);
            for y in 0..ih {
                let row = std::slice::from_raw_parts(plane.add(y * stride), iw * 3);
                data.extend_from_slice(row);
            }
            Ok(RgbImage {
                width: iw as u32,
                height: ih as u32,
                data,
            })
        }
    }
}

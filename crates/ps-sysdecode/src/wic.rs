//! Windows: WIC with the HEIF Image Extensions and an HEVC decoder (ADR-0089, design D2). Per thread, COM is
//! initialised multithreaded once (an apartment set up otherwise by the host is used as found) and one
//! `IWICImagingFactory` is created, both held by one thread-local ([`ComThread`]). A memory stream over the bytes, `CreateDecoderFromStream`, frame 0, a
//! scaler when the frame is larger than needed, a format converter to 24-bit RGB, `CopyPixels`.
//!
//! The platform checks explain a failure: the Windows build (HEIF Image Extensions need 10 version 1809,
//! build 17763), a WIC decoder for the HEIF container (HEIF Image Extensions), and a Media Foundation video
//! decoder for HEVC (HEVC Video Extensions, the device-manufacturer edition or a GPU vendor's). Media
//! Foundation is loaded at run time: on an N edition without the Media Feature Pack `mfplat.dll` is absent,
//! and a static import would stop the programs from starting.

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::ffi::c_void;

use ps_media::StillDecoder;
use ps_media::decode::{DecodeError, RgbImage};
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatHeif, GUID_WICPixelFormat24bppRGB,
    IWICBitmapDecoderInfo, IWICBitmapSource, IWICImagingFactory, WICBitmapDitherTypeNone,
    WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom, WICComponentEnumerateDefault,
    WICDecodeMetadataCacheOnDemand, WICDecoder,
};
use windows::Win32::Media::MediaFoundation::{
    MFMediaType_Video, MFT_CATEGORY_VIDEO_DECODER, MFT_ENUM_FLAG_ALL, MFT_REGISTER_TYPE_INFO,
    MFVideoFormat_HEVC,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::core::{GUID, HRESULT, IUnknown, Interface, PCSTR, w};

use crate::{MAX_PIXELS, PlatformProbe};

/// COM and the WIC factory of one thread, in one thread-local so their order at thread exit is ours, not the
/// unspecified order of separate thread-locals.
///
/// At thread exit Rust runs thread-local destructors from the loader's thread-detach callback, with the loader
/// lock held. Releasing the factory there is a reference-count decrement in the in-process WIC component
/// (`windowscodecs.dll` stays loaded), so it is done first; `CoUninitialize` is deliberately not called there:
/// it may unload COM servers and wait on other threads under the loader lock (a deadlock), and the system frees
/// a thread's COM state when the thread ends anyway. A thread that initialised COM itself keeps it until it
/// ends.
struct ComThread {
    factory: Option<IWICImagingFactory>,
}

impl Drop for ComThread {
    fn drop(&mut self) {
        // Released before anything else; no CoUninitialize (see the type's docs).
        drop(self.factory.take());
    }
}

thread_local! {
    static COM: RefCell<Option<ComThread>> = const { RefCell::new(None) };
}

/// Initialise COM on this thread once (S_FALSE and RPC_E_CHANGED_MODE tolerated).
fn ensure_com() -> Result<(), String> {
    COM.with(|c| {
        if c.borrow().is_some() {
            return Ok(());
        }
        // SAFETY: reserved pointer NULL; called on the current thread.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            return Err(format!("CoInitializeEx failed ({hr:?})"));
        }
        // RPC_E_CHANGED_MODE: the thread already has another apartment; it is used as found.
        *c.borrow_mut() = Some(ComThread { factory: None });
        Ok(())
    })
}

fn factory() -> Result<IWICImagingFactory, String> {
    ensure_com()?;
    COM.with(|c| {
        let mut c = c.borrow_mut();
        let state = c.as_mut().ok_or("COM is not initialised on this thread")?;
        if let Some(x) = state.factory.as_ref() {
            return Ok(x.clone());
        }
        // SAFETY: COM is initialised on this thread; the CLSID and interface are WIC's.
        let x: IWICImagingFactory =
            unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
                .map_err(|e| format!("WIC factory: {e}"))?;
        state.factory = Some(x.clone());
        Ok(x)
    })
}

/// WIC as a [`StillDecoder`].
pub struct Wic;

impl StillDecoder for Wic {
    fn name(&self) -> &str {
        "WIC"
    }

    fn decode(&self, bytes: &[u8], min_long: u32) -> Result<RgbImage, DecodeError> {
        decode(bytes, min_long.max(512)).map_err(DecodeError::Failed)
    }

    fn applies_transforms(&self) -> bool {
        // Not documented for the HEIF decoder: the self-test's rotated sample decides (D5).
        false
    }
}

fn decode(bytes: &[u8], min_long: u32) -> Result<RgbImage, String> {
    let f = factory()?;
    // SAFETY (whole block): every interface comes from WIC on this thread's apartment and is released by
    // its `Drop`; the memory stream copies `bytes`; `CopyPixels` writes at most `buf.len()` bytes with the
    // given stride.
    unsafe {
        let stream = SHCreateMemStream(Some(bytes)).ok_or("SHCreateMemStream failed")?;
        let decoder = f
            .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
            .map_err(|e| format!("no WIC decoder for this file: {e}"))?;
        let frame = decoder.GetFrame(0).map_err(|e| format!("frame: {e}"))?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).map_err(|e| e.to_string())?;
        if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
            return Err(format!("unsupported image size {w}x{h}"));
        }
        let mut source: IWICBitmapSource = frame.cast().map_err(|e| e.to_string())?;
        let long = w.max(h);
        if long > min_long {
            let (tw, th) = (
                (u64::from(w) * u64::from(min_long) / u64::from(long)).max(1) as u32,
                (u64::from(h) * u64::from(min_long) / u64::from(long)).max(1) as u32,
            );
            let scaler = f.CreateBitmapScaler().map_err(|e| e.to_string())?;
            scaler
                .Initialize(&source, tw, th, WICBitmapInterpolationModeFant)
                .map_err(|e| format!("scaler: {e}"))?;
            source = scaler.cast().map_err(|e| e.to_string())?;
            (w, h) = (tw, th);
        }
        let conv = f.CreateFormatConverter().map_err(|e| e.to_string())?;
        conv.Initialize(
            &source,
            &GUID_WICPixelFormat24bppRGB,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )
        .map_err(|e| format!("format converter: {e}"))?;
        let stride = w as usize * 3;
        let mut buf = vec![0u8; stride * h as usize];
        conv.CopyPixels(std::ptr::null(), stride as u32, &mut buf)
            .map_err(|e| format!("decode: {e}"))?;
        Ok(RgbImage {
            width: w,
            height: h,
            data: buf,
        })
    }
}

/// A WIC decoder for the HEIF container is registered (HEIF Image Extensions).
fn heif_decoder_registered() -> Result<bool, String> {
    let f = factory()?;
    // SAFETY: enumerating WIC components on this thread's apartment; every returned interface is released by
    // its `Drop`.
    unsafe {
        let e = f
            .CreateComponentEnumerator(WICDecoder.0 as u32, WICComponentEnumerateDefault.0 as u32)
            .map_err(|e| e.to_string())?;
        loop {
            let mut item: [Option<IUnknown>; 1] = [None];
            let mut fetched = 0u32;
            let hr = e.Next(&mut item, Some(&mut fetched));
            if hr.is_err() || fetched == 0 {
                return Ok(false);
            }
            if let Some(u) = item[0].take()
                && let Ok(info) = u.cast::<IWICBitmapDecoderInfo>()
                && info
                    .GetContainerFormat()
                    .is_ok_and(|g| g == GUID_ContainerFormatHeif)
            {
                return Ok(true);
            }
        }
    }
}

type MftEnumEx = unsafe extern "system" fn(
    GUID,
    u32,
    *const MFT_REGISTER_TYPE_INFO,
    *const MFT_REGISTER_TYPE_INFO,
    *mut *mut *mut c_void,
    *mut u32,
) -> HRESULT;

/// Media Foundation's HEVC video decoders: `None` when Media Foundation itself is absent (an N edition
/// without the Media Feature Pack), else how many are registered.
fn hevc_decoders() -> Option<u32> {
    // SAFETY: `mfplat.dll` is a system library; `MFTEnumEx` is looked up by name and called with its
    // documented signature; the returned array and its activation objects are released here.
    unsafe {
        let lib = LoadLibraryW(w!("mfplat.dll")).ok()?;
        let proc = GetProcAddress(lib, PCSTR(c"MFTEnumEx".as_ptr().cast()))?;
        let enum_ex: MftEnumEx = std::mem::transmute(proc);
        let input = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_HEVC,
        };
        let mut array: *mut *mut c_void = std::ptr::null_mut();
        let mut count = 0u32;
        let hr = enum_ex(
            MFT_CATEGORY_VIDEO_DECODER,
            MFT_ENUM_FLAG_ALL.0 as u32,
            &input,
            std::ptr::null(),
            &mut array,
            &mut count,
        );
        if hr.is_err() {
            return Some(0);
        }
        if !array.is_null() {
            for i in 0..count as usize {
                let p = *array.add(i);
                if !p.is_null() {
                    drop(IUnknown::from_raw(p));
                }
            }
            CoTaskMemFree(Some(array as *const c_void));
        }
        Some(count)
    }
}

/// The Windows build number and whether Media Foundation is missing.
pub fn windows_version() -> (u32, bool) {
    use windows::Wdk::System::SystemServices::RtlGetVersion;
    use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
    let mut v = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    // SAFETY: `v` is a correctly sized OSVERSIONINFOW.
    let ok = unsafe { RtlGetVersion(&mut v) }.is_ok();
    let build = if ok { v.dwBuildNumber } else { 0 };
    // SAFETY: probing for a system library by name.
    let no_mf = unsafe { LoadLibraryW(w!("mfplat.dll")) }.is_err();
    (build, no_mf)
}

pub fn probe() -> PlatformProbe {
    let (build, no_mf) = windows_version();
    let heif = heif_decoder_registered().unwrap_or(false);
    let hevc = hevc_decoders();
    let finding = crate::guidance::windows_finding(build, no_mf, heif, hevc);
    PlatformProbe {
        backend_name: "WIC".into(),
        version: (build != 0).then(|| format!("Windows build {build}")),
        // The decoder is offered whenever WIC itself works: the self-test is the final word, and a failure
        // reports the finding above.
        decoder: factory()
            .ok()
            .map(|_| std::sync::Arc::new(Wic) as std::sync::Arc<dyn StillDecoder>),
        finding,
    }
}

//! Renders a [`Plan`] in parallel (rayon), writes every file through a [`CorpusWriter`], picks the expected
//! keepers (ADR-0014) and returns the [`Truth`] (also written as `truth.json`).

use std::cmp::Reverse;
use std::io;

use ps_model::PhotoClass;
use rayon::prelude::*;

use crate::camera::{Body, ExifSpec, exif_tiff};
use crate::expected::{Facts, expected_class};
use crate::jpeg::{
    PngKind, app1_exif, camera_jpeg, editor_jpeg, encode_jpeg, encode_png, strip_segments,
};
use crate::plan::{CacheKind, DerivKind, From, JunkKind, Plan, Shot, Task, plan_with};
use crate::preset::{Params, Preset};
use crate::rng::derive;
use crate::synth::{Perturb, Rgb, render};
use crate::truth::{Category, TRUTH_FILE, TRUTH_VERSION, Truth, TruthFile};
use crate::writer::CorpusWriter;

/// Generation failure (only the writer can fail).
#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    #[error("writing {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: io::Error,
    },
}

/// Generate the corpus for `seed` and `preset` into `writer`; also writes `truth.json`.
pub fn generate(
    seed: u64,
    preset: Preset,
    writer: &dyn CorpusWriter,
) -> Result<Truth, GenerateError> {
    generate_with(seed, preset, preset.params(), writer)
}

/// Like [`generate`] with explicit parameters.
pub fn generate_with(
    seed: u64,
    preset: Preset,
    params: Params,
    writer: &dyn CorpusWriter,
) -> Result<Truth, GenerateError> {
    let plan = plan_with(seed, preset, params);
    let truth = render_plan(&plan, writer)?;
    let json = truth.to_json();
    write(writer, TRUTH_FILE, &json, 0)?;
    Ok(truth)
}

/// Internal per-file record: the truth entry plus ADR-0014 ranking inputs.
struct Rec {
    file: TruthFile,
    /// 0 RAW, 1 camera original (maker notes), 2 lossless, 3 other lossy.
    tier: u8,
    completeness: u8,
    quality: u8,
}

impl Rec {
    fn depth(&self) -> usize {
        self.file.path.matches('/').count()
    }
}

fn write(w: &dyn CorpusWriter, path: &str, bytes: &[u8], mtime: i64) -> Result<(), GenerateError> {
    w.write(path, bytes, mtime)
        .map_err(|source| GenerateError::Write {
            path: path.to_string(),
            source,
        })
}

fn drive_of(path: &str) -> String {
    path.split('/').next().unwrap_or_default().to_string()
}

/// Shared builder for records.
struct Out<'w> {
    w: &'w dyn CorpusWriter,
    recs: Vec<Rec>,
}

struct Meta<'a> {
    category: Category,
    group: &'a str,
    format: &'a str,
    width: u32,
    height: u32,
    camera_exif: bool,
    palette_or_alpha: bool,
    video: Option<(u64, bool)>,
    tier: u8,
    completeness: u8,
    quality: u8,
    derived_from: Option<&'a str>,
}

impl Out<'_> {
    fn put(
        &mut self,
        path: &str,
        bytes: &[u8],
        mtime: i64,
        m: Meta<'_>,
    ) -> Result<(), GenerateError> {
        write(self.w, path, bytes, mtime)?;
        let graphic_format = matches!(m.format, "png" | "gif" | "ico" | "svg");
        let class = expected_class(&Facts {
            path,
            camera_exif: m.camera_exif,
            palette_or_alpha: m.palette_or_alpha,
            graphic_format,
            width: m.width,
            height: m.height,
            video_ms: m.video.map(|v| v.0),
            video_meta: m.video.is_some_and(|v| v.1),
        });
        self.recs.push(Rec {
            file: TruthFile {
                path: path.to_string(),
                drive: drive_of(path),
                category: m.category,
                expected_class: class,
                group: m.group.to_string(),
                keeper: false,
                keeper_set: false,
                format: m.format.to_string(),
                width: m.width,
                height: m.height,
                size: bytes.len() as u64,
                mtime,
                content: blake3::hash(bytes).to_hex().to_string(),
                derived_from: m.derived_from.map(str::to_string),
                alternate_groups: Vec::new(),
            },
            tier: m.tier,
            completeness: m.completeness,
            quality: m.quality,
        });
        Ok(())
    }
}

/// Render every task and assemble the truth (files sorted by path).
pub fn render_plan(plan: &Plan, writer: &dyn CorpusWriter) -> Result<Truth, GenerateError> {
    let per_task: Vec<Vec<Rec>> = plan
        .tasks
        .par_iter()
        .map(|t| {
            let mut out = Out {
                w: writer,
                recs: Vec::new(),
            };
            match t {
                Task::Shot(s) => render_shot(s, &plan.bodies[s.body], &mut out)?,
                Task::Scan(s) => {
                    let img = render(
                        &s.scene,
                        &Perturb::identity(s.grain_seed),
                        s.display.0,
                        s.display.1,
                    );
                    let img = sepia(&img);
                    let bytes = encode_jpeg(&img, 90);
                    let (w, h) = s.display;
                    let meta = |cat, from| Meta {
                        category: cat,
                        group: &s.group,
                        format: "jpeg",
                        width: w,
                        height: h,
                        camera_exif: false,
                        palette_or_alpha: false,
                        video: None,
                        tier: 3,
                        completeness: 0,
                        quality: 90,
                        derived_from: from,
                    };
                    out.put(&s.path, &bytes, s.mtime, meta(Category::Scan, None))?;
                    for (p, mt) in &s.copies {
                        out.put(p, &bytes, *mt, meta(Category::ExactCopy, Some(&s.path)))?;
                    }
                }
                Task::Video(v) => {
                    let bytes = crate::video::write_video(&v.spec);
                    let fmt = if v.spec.quicktime { "mov" } else { "mp4" };
                    let has_meta = v.spec.make.is_some();
                    let meta = |cat, from| Meta {
                        category: cat,
                        group: &v.group,
                        format: fmt,
                        width: v.spec.width,
                        height: v.spec.height,
                        camera_exif: false,
                        palette_or_alpha: false,
                        video: Some((v.spec.duration_ms, has_meta || !v.junk)),
                        tier: 3,
                        completeness: u8::from(has_meta) * 2,
                        quality: 0,
                        derived_from: from,
                    };
                    let cat = if v.junk {
                        Category::JunkVideo
                    } else {
                        Category::Video
                    };
                    out.put(&v.path, &bytes, v.mtime, meta(cat, None))?;
                    for (p, mt) in &v.copies {
                        out.put(p, &bytes, *mt, meta(Category::ExactCopy, Some(&v.path)))?;
                    }
                }
                Task::Junk(j) => {
                    let mut rng = derive(j.seed, "junk", 0);
                    let (w, h) = j.size;
                    let (bytes, cat, fmt, pal) = match j.kind {
                        JunkKind::Icon => (
                            crate::junk::ico(&mut rng, &[16, 32, 48, 256]),
                            Category::Icon,
                            "ico",
                            true,
                        ),
                        JunkKind::UiPng => (
                            crate::junk::ui_png(&mut rng, w, h),
                            Category::UiGraphic,
                            "png",
                            true,
                        ),
                        JunkKind::UiGif => (
                            crate::junk::ui_gif(&mut rng, w, h),
                            Category::UiGraphic,
                            "gif",
                            true,
                        ),
                        JunkKind::TinyWeb => (
                            crate::junk::tiny_web(&mut rng, w, h).1,
                            Category::TinyWeb,
                            "jpeg",
                            false,
                        ),
                    };
                    out.put(
                        &j.path,
                        &bytes,
                        j.mtime,
                        Meta {
                            category: cat,
                            group: &j.group,
                            format: fmt,
                            width: w,
                            height: h,
                            camera_exif: false,
                            palette_or_alpha: pal,
                            video: None,
                            tier: 3,
                            completeness: 0,
                            quality: 0,
                            derived_from: None,
                        },
                    )?;
                }
            }
            choose_keepers(&mut out.recs);
            Ok(out.recs)
        })
        .collect::<Result<_, GenerateError>>()?;
    let mut files: Vec<TruthFile> = per_task.into_iter().flatten().map(|r| r.file).collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    for f in &mut files {
        if let Some(alts) = plan.alternate_groups.get(&f.path) {
            f.alternate_groups.clone_from(alts);
        }
    }
    Ok(Truth {
        truth_version: TRUTH_VERSION,
        generator: format!("ps-fixtures {}", env!("CARGO_PKG_VERSION")),
        seed: plan.seed,
        preset: plan.preset.as_str().to_string(),
        drives: crate::plan::DRIVES.iter().map(|d| d.to_string()).collect(),
        files,
        must_not_merge: plan.must_not_merge.clone(),
    })
}

/// ADR-0014 ranking per group: tier, pixels, completeness, quality, size, oldest mtime, shortest path depth,
/// then path. Cache copies only win if nothing else is there; junk/graphic classes never keep. A RAW keeper
/// forms a keeper set with the JPEG of the same stem in the same folder.
fn choose_keepers(recs: &mut [Rec]) {
    let mut groups: Vec<&str> = recs.iter().map(|r| r.file.group.as_str()).collect();
    groups.sort_unstable();
    groups.dedup();
    let groups: Vec<String> = groups.into_iter().map(str::to_string).collect();
    for g in groups {
        let members: Vec<usize> = (0..recs.len())
            .filter(|&i| recs[i].file.group == g)
            .collect();
        if members.iter().all(|&i| recs[i].file.category.is_junk()) {
            continue;
        }
        let eligible = |i: &usize| {
            !matches!(
                recs[*i].file.expected_class,
                PhotoClass::Junk | PhotoClass::Graphic
            )
        };
        let non_cache: Vec<usize> = members
            .iter()
            .copied()
            .filter(eligible)
            .filter(|&i| recs[i].file.expected_class != PhotoClass::ThumbnailCache)
            .collect();
        let pool = if non_cache.is_empty() {
            members.iter().copied().filter(eligible).collect()
        } else {
            non_cache
        };
        let Some(&best) = pool.iter().min_by_key(|&&i| {
            let r = &recs[i];
            (
                r.tier,
                Reverse(u64::from(r.file.width) * u64::from(r.file.height)),
                Reverse(r.completeness),
                Reverse(r.quality),
                Reverse(r.file.size),
                r.file.mtime,
                r.depth(),
                r.file.path.clone(),
            )
        }) else {
            continue;
        };
        recs[best].file.keeper = true;
        if recs[best].file.format.starts_with("raw:") {
            let raw_path = recs[best].file.path.clone();
            let stem = raw_path
                .rsplit_once('.')
                .map(|(s, _)| s)
                .unwrap_or(&raw_path)
                .to_string();
            if let Some(&jpeg) = members.iter().find(|&&i| {
                recs[i].file.format == "jpeg"
                    && recs[i].file.path.rsplit_once('.').map(|(s, _)| s) == Some(stem.as_str())
            }) {
                recs[best].file.keeper_set = true;
                recs[jpeg].file.keeper_set = true;
            }
        }
    }
}

fn sepia(img: &Rgb) -> Rgb {
    let mut out = img.clone();
    for p in out.data.as_chunks_mut::<3>().0 {
        let l = (u32::from(p[0]) * 77 + u32::from(p[1]) * 150 + u32::from(p[2]) * 29) >> 8;
        p[0] = (l * 240 / 255 + 15).min(255) as u8;
        p[1] = (l * 215 / 255 + 10).min(255) as u8;
        p[2] = (l * 170 / 255 + 5).min(255) as u8;
    }
    out
}

const EDITORS: &[&str] = &[
    "Adobe Photoshop Lightroom Classic 9.2 (Windows)",
    "GIMP 2.10.30",
    "Picasa",
    "Apple Photos 6.0",
];

/// The converted-burst files of one frame (ADR-0105): the CR2-like original next to the camera JPEG, the
/// converted DNG (rendered again by the "converter", without sub-second `00` and with the maker note in
/// `DNGPrivateData`) and a copy of the camera JPEG next to it.
fn render_converted(
    s: &Shot,
    c: &crate::plan::ConvertedFrame,
    spec: &ExifSpec<'_>,
    encoded: &[u8],
    original: &[u8],
    completeness: u8,
    out: &mut Out<'_>,
) -> Result<(), GenerateError> {
    let (dw, dh) = s.display;
    let group = s.group.as_str();
    let raw_meta = |cat, format, w, h, from| Meta {
        category: cat,
        group,
        format,
        width: w,
        height: h,
        camera_exif: true,
        palette_or_alpha: false,
        video: None,
        tier: 0,
        completeness,
        quality: 100,
        derived_from: from,
    };
    let preview = strip_segments(encoded, &[0xE0]);
    let stored = (spec.width, spec.height);
    let cr2 = crate::dng::cr2_like(spec, &preview, stored.0, stored.1);
    out.put(
        &c.raw_path,
        &cr2,
        s.mtime,
        raw_meta(Category::RawOriginal, "raw:cr2", dw, dh, None),
    )?;
    // The converter's own rendering of the frame, and a sensor a little larger than the JPEG.
    let rendered = render(&s.scene, &c.render, dw, dh).to_stored(s.orientation);
    let dng_preview = strip_segments(&encode_jpeg(&rendered, 90), &[0xE0]);
    let sensor = (
        (stored.0 + stored.0 / 64) & !1,
        (stored.1 + stored.1 / 64) & !1,
    );
    let raw_name = c
        .raw_path
        .rsplit('/')
        .next()
        .unwrap_or(&c.raw_path)
        .to_string();
    let note = crate::camera::spec_maker_note(spec).unwrap_or_default();
    // Adobe drops a sub-second of `00` (two digits); other sub-seconds stay.
    let dng_spec = ExifSpec {
        maker_note: None,
        subsec: s.capture.millis / 10 != 0,
        thumbnail: None,
        ..spec.clone()
    };
    let dng = crate::dng::converted_dng(
        &dng_spec,
        &dng_preview,
        rendered.width,
        rendered.height,
        &raw_name,
        &note,
        sensor,
    );
    let (sw, sh) = if s.orientation >= 5 {
        (sensor.1, sensor.0)
    } else {
        sensor
    };
    out.put(
        &c.dng_path,
        &dng,
        c.dng_mtime,
        raw_meta(
            Category::ConvertedRaw,
            "raw:dng",
            sw,
            sh,
            Some(c.raw_path.as_str()),
        ),
    )?;
    let mut copy = raw_meta(
        Category::ExactCopy,
        "jpeg",
        dw,
        dh,
        Some(s.path.as_str()),
    );
    copy.tier = 1;
    copy.quality = s.quality;
    out.put(&c.jpeg_copy_path, original, c.dng_mtime, copy)
}

fn render_shot(s: &Shot, body: &Body, out: &mut Out<'_>) -> Result<(), GenerateError> {
    let (dw, dh) = s.display;
    let display = render(&s.scene, &s.perturb, dw, dh);
    let stored = display.to_stored(s.orientation);
    let thumb = strip_segments(&encode_jpeg(&stored.fit_long_edge(160), 70), &[0xE0]);
    let completeness = 3 + u8::from(s.gps.is_some());
    let spec = ExifSpec {
        body,
        capture: &s.capture,
        exposure: &s.exposure,
        gps: s.gps,
        orientation: s.orientation,
        width: stored.width,
        height: stored.height,
        maker_note: Some(s.maker_seed),
        software: None,
        thumbnail: Some(thumb),
        subsec: true,
        file_number: s.converted.as_ref().map(|c| c.file_number),
    };
    let encoded = encode_jpeg(&stored, s.quality);
    let original = camera_jpeg(&encoded, &app1_exif(&exif_tiff(&spec)));
    let group = s.group.as_str();
    let jpeg_meta = |cat, w, h, exif: bool, tier, q, from| Meta {
        category: cat,
        group,
        format: "jpeg",
        width: w,
        height: h,
        camera_exif: exif,
        palette_or_alpha: false,
        video: None,
        tier,
        completeness: if exif { completeness } else { 0 },
        quality: q,
        derived_from: from,
    };
    out.put(
        &s.path,
        &original,
        s.mtime,
        jpeg_meta(Category::Original, dw, dh, true, 1, s.quality, None),
    )?;

    let dng = s.raw_path.as_ref().map(|rp| {
        let preview = strip_segments(&encoded, &[0xE0]);
        let bytes = crate::dng::dng(&spec, &preview, stored.width, stored.height);
        (rp.clone(), bytes)
    });
    let raw_meta = |cat, from| Meta {
        category: cat,
        group,
        format: "raw:dng",
        width: dw,
        height: dh,
        camera_exif: true,
        palette_or_alpha: false,
        video: None,
        tier: 0,
        completeness,
        quality: 100,
        derived_from: from,
    };
    if let Some((rp, bytes)) = &dng {
        out.put(rp, bytes, s.mtime, raw_meta(Category::RawOriginal, None))?;
    }

    if let Some(c) = &s.converted {
        render_converted(s, c, &spec, &encoded, &original, completeness, out)?;
    }

    // Editor-style EXIF (no maker note, orientation baked, new dimensions).
    let editor_exif = |w: u32, h: u32, k: usize| {
        app1_exif(&exif_tiff(&ExifSpec {
            orientation: 1,
            width: w,
            height: h,
            maker_note: None,
            software: Some(EDITORS[k % EDITORS.len()]),
            thumbnail: None,
            ..spec.clone()
        }))
    };

    // Bytes of each derivative, for exact copies of derivatives.
    let mut produced: Vec<(Vec<u8>, Category, u32, u32, bool, u8, u8, &'static str)> = Vec::new();
    for (k, d) in s.derivs.iter().enumerate() {
        let entry = match (d.kind, d.from) {
            (DerivKind::Exact | DerivKind::Renamed, From::Deriv(i)) => {
                let (b, _, w, h, exif, tier, q, fmt) = &produced[i];
                let src = &s.derivs[i].path;
                let mut m = jpeg_meta(
                    Category::ExactCopy,
                    *w,
                    *h,
                    *exif,
                    *tier,
                    *q,
                    Some(src.as_str()),
                );
                m.format = fmt;
                if d.kind == DerivKind::Renamed {
                    m.category = Category::RenamedCopy;
                }
                out.put(&d.path, b, d.mtime, m)?;
                (
                    b.clone(),
                    Category::ExactCopy,
                    *w,
                    *h,
                    *exif,
                    *tier,
                    *q,
                    *fmt,
                )
            }
            (DerivKind::Exact | DerivKind::Renamed, From::Original) => {
                let cat = if d.kind == DerivKind::Exact {
                    Category::ExactCopy
                } else {
                    Category::RenamedCopy
                };
                out.put(
                    &d.path,
                    &original,
                    d.mtime,
                    jpeg_meta(cat, dw, dh, true, 1, s.quality, Some(&s.path)),
                )?;
                (Vec::new(), cat, dw, dh, true, 1, s.quality, "jpeg")
            }
            (DerivKind::ExactRaw, _) => {
                if let Some((rp, bytes)) = &dng {
                    out.put(
                        &d.path,
                        bytes,
                        d.mtime,
                        raw_meta(Category::ExactCopy, Some(rp)),
                    )?;
                }
                (
                    Vec::new(),
                    Category::ExactCopy,
                    dw,
                    dh,
                    true,
                    0,
                    100,
                    "raw:dng",
                )
            }
            (
                DerivKind::Resized {
                    long,
                    quality,
                    keep_exif,
                },
                _,
            ) => {
                let img = display.fit_long_edge(long);
                let enc = encode_jpeg(&img, quality);
                let bytes = if keep_exif {
                    editor_jpeg(&enc, &editor_exif(img.width, img.height, k))
                } else {
                    enc
                };
                out.put(
                    &d.path,
                    &bytes,
                    d.mtime,
                    jpeg_meta(
                        Category::Resized,
                        img.width,
                        img.height,
                        keep_exif,
                        3,
                        quality,
                        Some(&s.path),
                    ),
                )?;
                (
                    bytes,
                    Category::Resized,
                    img.width,
                    img.height,
                    keep_exif,
                    3,
                    quality,
                    "jpeg",
                )
            }
            (DerivKind::Recompressed { quality, keep_exif }, _) => {
                let enc = encode_jpeg(&display, quality);
                let bytes = if keep_exif {
                    editor_jpeg(&enc, &editor_exif(dw, dh, k))
                } else {
                    enc
                };
                out.put(
                    &d.path,
                    &bytes,
                    d.mtime,
                    jpeg_meta(
                        Category::Recompressed,
                        dw,
                        dh,
                        keep_exif,
                        3,
                        quality,
                        Some(&s.path),
                    ),
                )?;
                (
                    bytes,
                    Category::Recompressed,
                    dw,
                    dh,
                    keep_exif,
                    3,
                    quality,
                    "jpeg",
                )
            }
            (DerivKind::Stripped, _) => {
                // Rotated shots are re-encoded upright (as web services do); upright ones lose only APP1.
                let bytes = if s.orientation == 1 {
                    strip_segments(&original, &[0xE1])
                } else {
                    encode_jpeg(&display, s.quality)
                };
                out.put(
                    &d.path,
                    &bytes,
                    d.mtime,
                    jpeg_meta(
                        Category::ExifStripped,
                        dw,
                        dh,
                        false,
                        3,
                        s.quality,
                        Some(&s.path),
                    ),
                )?;
                (
                    bytes,
                    Category::ExifStripped,
                    dw,
                    dh,
                    false,
                    3,
                    s.quality,
                    "jpeg",
                )
            }
            (DerivKind::Png, _) => {
                let bytes = encode_png(PngKind::Rgb(&display));
                let mut m = jpeg_meta(
                    Category::PngConversion,
                    dw,
                    dh,
                    false,
                    2,
                    100,
                    Some(&s.path),
                );
                m.format = "png";
                out.put(&d.path, &bytes, d.mtime, m)?;
                (bytes, Category::PngConversion, dw, dh, false, 2, 100, "png")
            }
            (DerivKind::Cache(kind), _) => {
                let (bytes, img, fmt, q) = match kind {
                    CacheKind::Freedesktop(long) => {
                        let img = display.fit_long_edge(long);
                        (encode_png(PngKind::Rgb(&img)), img, "png", 100)
                    }
                    CacheKind::Jpeg(long, q) => {
                        let img = display.fit_long_edge(long);
                        (encode_jpeg(&img, q), img, "jpeg", q)
                    }
                };
                let tier = if fmt == "png" { 2 } else { 3 };
                let mut m = jpeg_meta(
                    Category::CacheThumbnail,
                    img.width,
                    img.height,
                    false,
                    tier,
                    q,
                    Some(&s.path),
                );
                m.format = fmt;
                out.put(&d.path, &bytes, d.mtime, m)?;
                (
                    bytes,
                    Category::CacheThumbnail,
                    img.width,
                    img.height,
                    false,
                    tier,
                    q,
                    fmt,
                )
            }
        };
        produced.push(entry);
    }

    // Crops: their own groups.
    for c in &s.crops {
        let x0 = (u64::from(dw) * u64::from(c.rect.0) / 10_000) as u32;
        let y0 = (u64::from(dh) * u64::from(c.rect.1) / 10_000) as u32;
        let cw = ((u64::from(dw) * u64::from(c.rect.2) / 10_000) as u32).min(dw - x0) & !1;
        let ch = ((u64::from(dh) * u64::from(c.rect.3) / 10_000) as u32).min(dh - y0) & !1;
        let img = display.crop(x0, y0, cw, ch);
        let enc = encode_jpeg(&img, c.quality);
        let bytes = editor_jpeg(&enc, &editor_exif(cw, ch, 0));
        let cg = c.group.as_str();
        let meta = |cat, w, h, q, from| Meta {
            category: cat,
            group: cg,
            format: "jpeg",
            width: w,
            height: h,
            camera_exif: true,
            palette_or_alpha: false,
            video: None,
            tier: 3,
            completeness,
            quality: q,
            derived_from: from,
        };
        out.put(
            &c.path,
            &bytes,
            c.mtime,
            meta(Category::Crop, cw, ch, c.quality, Some(&s.path)),
        )?;
        for d in &c.derivs {
            match d.kind {
                DerivKind::Resized { long, quality, .. } => {
                    let small = img.fit_long_edge(long);
                    let enc = encode_jpeg(&small, quality);
                    let b = editor_jpeg(&enc, &editor_exif(small.width, small.height, 1));
                    out.put(
                        &d.path,
                        &b,
                        d.mtime,
                        meta(
                            Category::Resized,
                            small.width,
                            small.height,
                            quality,
                            Some(&c.path),
                        ),
                    )?;
                }
                _ => out.put(
                    &d.path,
                    &bytes,
                    d.mtime,
                    meta(Category::ExactCopy, cw, ch, c.quality, Some(&c.path)),
                )?,
            }
        }
    }
    Ok(())
}

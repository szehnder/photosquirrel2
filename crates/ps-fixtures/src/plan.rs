//! The corpus plan: every shot, derivative, path and modification time, decided sequentially from the seed
//! before any pixels are rendered. Rendering ([`crate::generate`]) then runs the tasks in parallel.

use std::collections::BTreeSet;

use crate::camera::{Body, Capture, Exposure, Gps, MODELS, Naming, OWNERS};
use crate::preset::{Params, Preset};
use crate::rng::{CorpusRng, Rng, derive, pick};
use crate::synth::{Perturb, Scene};
use crate::truth::{BurstVariant, MustNotMerge, MustNotMergeKind};

pub const DRIVES: [&str; 3] = ["drive-a", "drive-b", "drive-c"];

/// Which derivatives a shot may get. Must-not-merge members only get derivatives that keep their capture
/// metadata, plus (bursts) at most one metadata-free thumbnail of one frame: that one is genuinely ambiguous,
/// so the truth lists the sibling frames as its `alternate_groups` (ADR-0039).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    Full,
    ExifPreserving,
    ExactOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheKind {
    /// freedesktop `.thumbnails` PNG with this long edge.
    Freedesktop(u32),
    /// Cache JPEG (Lightroom preview, Photos proxy, Picasa) with long edge and quality.
    Jpeg(u32, u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DerivKind {
    Exact,
    /// Exact copy of the DNG of a RAW+JPEG pair.
    ExactRaw,
    Renamed,
    Resized {
        long: u32,
        quality: u8,
        keep_exif: bool,
    },
    Recompressed {
        quality: u8,
        keep_exif: bool,
    },
    Stripped,
    Png,
    Cache(CacheKind),
}

/// Where a derivative's bytes come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum From {
    Original,
    /// An earlier derivative of the same image (exact copies of exports).
    Deriv(usize),
}

#[derive(Clone, Debug)]
pub struct Deriv {
    pub kind: DerivKind,
    pub path: String,
    pub mtime: i64,
    pub from: From,
}

/// A crop made from a shot (own group).
#[derive(Clone, Debug)]
pub struct CropPlan {
    pub group: String,
    /// Fractions of the display image: x0, y0, w, h in 1/10000.
    pub rect: (u32, u32, u32, u32),
    pub path: String,
    pub mtime: i64,
    pub quality: u8,
    pub derivs: Vec<Deriv>,
}

#[derive(Clone, Debug)]
pub struct Shot {
    pub group: String,
    pub body: usize,
    pub scene: Scene,
    pub perturb: Perturb,
    pub capture: Capture,
    /// Display (orientation-applied) dimensions.
    pub display: (u32, u32),
    pub orientation: u16,
    pub gps: Option<Gps>,
    pub exposure: Exposure,
    pub quality: u8,
    /// Original path (JPEG); the DNG, if any, sits next to it with the same stem.
    pub path: String,
    pub raw_path: Option<String>,
    pub mtime: i64,
    pub maker_seed: u64,
    pub derivs: Vec<Deriv>,
    pub crops: Vec<CropPlan>,
}

#[derive(Clone, Debug)]
pub struct ScanPlan {
    pub group: String,
    pub scene: Scene,
    pub grain_seed: u64,
    pub display: (u32, u32),
    pub path: String,
    pub mtime: i64,
    pub copies: Vec<(String, i64)>,
}

#[derive(Clone, Debug)]
pub struct VideoPlan {
    pub group: String,
    pub junk: bool,
    pub spec: crate::video::VideoSpec,
    pub path: String,
    pub mtime: i64,
    pub copies: Vec<(String, i64)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JunkKind {
    Icon,
    UiPng,
    UiGif,
    TinyWeb,
}

#[derive(Clone, Debug)]
pub struct JunkPlan {
    pub group: String,
    pub kind: JunkKind,
    pub size: (u32, u32),
    pub path: String,
    pub mtime: i64,
    pub seed: u64,
}

pub enum Task {
    Shot(Box<Shot>),
    Scan(ScanPlan),
    Video(VideoPlan),
    Junk(JunkPlan),
}

pub struct Plan {
    pub seed: u64,
    pub preset: Preset,
    pub bodies: Vec<Body>,
    pub tasks: Vec<Task>,
    pub must_not_merge: Vec<MustNotMerge>,
    /// Path -> the other truth groups the file may be grouped with (ambiguous burst thumbnails).
    pub alternate_groups: std::collections::BTreeMap<String, Vec<String>>,
}

// ---------------------------------------------------------------------------------------------------------

struct Place {
    name: &'static str,
    lat_e6: i64,
    lon_e6: i64,
    offset_min: i16,
    alt_dm: u32,
    far: bool,
}

const PLACES: &[Place] = &[
    Place {
        name: "Home",
        lat_e6: 37_774_900,
        lon_e6: -122_419_400,
        offset_min: -420,
        alt_dm: 160,
        far: false,
    },
    Place {
        name: "Park",
        lat_e6: 37_769_400,
        lon_e6: -122_486_200,
        offset_min: -420,
        alt_dm: 600,
        far: false,
    },
    Place {
        name: "Lake Tahoe",
        lat_e6: 39_096_800,
        lon_e6: -120_032_400,
        offset_min: -420,
        alt_dm: 18_970,
        far: false,
    },
    Place {
        name: "Paris",
        lat_e6: 48_856_600,
        lon_e6: 2_352_200,
        offset_min: 120,
        alt_dm: 350,
        far: true,
    },
    Place {
        name: "Tokyo",
        lat_e6: 35_676_200,
        lon_e6: 139_650_300,
        offset_min: 540,
        alt_dm: 400,
        far: true,
    },
    Place {
        name: "New York",
        lat_e6: 40_712_800,
        lon_e6: -74_006_000,
        offset_min: -240,
        alt_dm: 100,
        far: true,
    },
    Place {
        name: "Zürich",
        lat_e6: 47_376_900,
        lon_e6: 8_541_700,
        offset_min: 120,
        alt_dm: 4_080,
        far: true,
    },
    Place {
        name: "Honolulu",
        lat_e6: 21_306_900,
        lon_e6: -157_858_300,
        offset_min: -600,
        alt_dm: 50,
        far: true,
    },
    Place {
        name: "Denver",
        lat_e6: 39_739_200,
        lon_e6: -104_990_300,
        offset_min: -360,
        alt_dm: 16_090,
        far: false,
    },
    Place {
        name: "Sydney",
        lat_e6: -33_868_800,
        lon_e6: 151_209_300,
        offset_min: 600,
        alt_dm: 580,
        far: true,
    },
];

const TITLES: &[&str] = &[
    "Beach",
    "Birthday Party",
    "Grandma's House",
    "Zoo",
    "Hiking",
    "Christmas",
    "Soccer Game",
    "First Day of School",
    "Camping",
    "Park",
    "Wedding",
    "Thanksgiving",
    "Lake House",
    "Road Trip",
    "Snow Day",
    "Garden",
    "Halloween",
    "Museum",
    "Fishing",
    "Café Brunch",
    "Graduation",
    "Dog Walk",
    "Easter",
    "Recital",
    "Farmers Market",
];

struct Backup {
    dir: String,
    preserve: bool,
    copy_time: i64,
}

struct Event {
    body: usize,
    title: String,
    place: usize,
    offset_min: i16,
    /// `2014/2014-07-04 Beach`
    rel: String,
    primary_dir: String,
    backups: Vec<Backup>,
    cursor_ms: i64,
    counter_hint: u32,
}

const LATEST_YEAR: i32 = 2025;

fn days_in_month(y: i32, m: u8) -> u8 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn random_time_in_year(rng: &mut CorpusRng, y: i32, hours: (u8, u8)) -> i64 {
    let m = rng.random_range(1..=12u8);
    let d = rng.random_range(1..=days_in_month(y, m));
    let h = rng.random_range(hours.0..hours.1);
    let mi = rng.random_range(0..60u8);
    let s = rng.random_range(0..60u8);
    ps_model::CaptureTime::from_parts(y, m, d, h, mi, s).local_seconds
}

fn ymd(secs: i64) -> (i32, u8, u8) {
    ps_model::civil_from_days(secs.div_euclid(86_400))
}

fn hex(rng: &mut CorpusRng, n: usize) -> String {
    (0..n)
        .map(|_| char::from(b"0123456789abcdef"[rng.random_range(0..16)]))
        .collect()
}

fn uuid(rng: &mut CorpusRng) -> String {
    format!(
        "{}-{}-{}-{}-{}",
        hex(rng, 8).to_uppercase(),
        hex(rng, 4).to_uppercase(),
        hex(rng, 4).to_uppercase(),
        hex(rng, 4).to_uppercase(),
        hex(rng, 12).to_uppercase()
    )
}

fn slug(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect()
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Unique path registry (case-insensitive, so corpora are valid on macOS/Windows too).
#[derive(Default)]
struct Paths {
    taken: BTreeSet<String>,
}

impl Paths {
    fn claim(&mut self, dir: &str, name: &str) -> String {
        let (stem, ext) = match name.rfind('.') {
            Some(i) => (&name[..i], &name[i..]),
            None => (name, ""),
        };
        let mut n = 1;
        loop {
            let candidate = if n == 1 {
                format!("{dir}/{name}")
            } else {
                format!("{dir}/{stem} ({n}){ext}")
            };
            if self.taken.insert(candidate.to_lowercase()) {
                return candidate;
            }
            n += 1;
        }
    }
}

struct Planner {
    params: Params,
    rng: CorpusRng,
    bodies: Vec<Body>,
    counters: Vec<u32>,
    events: Vec<Event>,
    paths: Paths,
    next_group: usize,
    next_junk: usize,
    tasks: Vec<Task>,
    mnm: Vec<MustNotMerge>,
    alternates: std::collections::BTreeMap<String, Vec<String>>,
    twelve_left: usize,
    wa_counter: u32,
}

/// A scene and the frame it was rendered into.
#[derive(Clone, Debug, PartialEq)]
pub struct FramedScene {
    pub scene: Scene,
    /// Display width / height (orientation applied).
    pub aspect: f32,
}

impl FramedScene {
    fn new(scene: &Scene, (w, h): (u32, u32)) -> Self {
        Self {
            scene: scene.clone(),
            aspect: w as f32 / h.max(1) as f32,
        }
    }

    /// What the image shows: the scene's shapes inside this frame.
    pub fn summary(&self) -> crate::synth::SceneSummary {
        self.scene.summary_in(self.aspect)
    }
}

/// The scene each true group (`truth.json` `group`) was rendered from, with its frame: camera shots and scanned
/// prints. Crops show only part of a scene and are left out. Demo data uses it to describe each image truthfully.
pub fn scenes_by_group(plan: &Plan) -> std::collections::HashMap<String, FramedScene> {
    plan.tasks
        .iter()
        .filter_map(|t| match t {
            Task::Shot(s) => Some((s.group.clone(), FramedScene::new(&s.scene, s.display))),
            Task::Scan(s) => Some((s.group.clone(), FramedScene::new(&s.scene, s.display))),
            _ => None,
        })
        .collect()
}

/// The scene behind each file of a generated corpus, keyed by the file's content (BLAKE3 hex, as in
/// `truth.json`). The plan is rebuilt from the truth's seed and preset; `None` when it does not match the truth
/// (a corpus generated with custom parameters), so callers never describe a file from the wrong scene. Crops are
/// left out (they show part of a scene).
pub fn scenes_by_content(
    truth: &crate::truth::Truth,
) -> Option<std::collections::HashMap<String, FramedScene>> {
    use crate::truth::Category;
    let preset = Preset::parse(&truth.preset)?;
    let plan = plan(truth.seed, preset);
    let groups = scenes_by_group(&plan);
    // Every shot's original must be in the truth under the shot's group.
    let originals: std::collections::HashSet<(&str, &str)> = truth
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.group.as_str()))
        .collect();
    let consistent = plan.tasks.iter().all(|t| match t {
        Task::Shot(s) => originals.contains(&(s.path.as_str(), s.group.as_str())),
        _ => true,
    });
    if !consistent {
        return None;
    }
    Some(
        truth
            .files
            .iter()
            .filter(|f| f.category != Category::Crop && !f.category.is_junk())
            .filter_map(|f| Some((f.content.clone(), groups.get(&f.group)?.clone())))
            .collect(),
    )
}

/// Build the plan for `seed` and `preset`.
pub fn plan(seed: u64, preset: Preset) -> Plan {
    plan_with(seed, preset, preset.params())
}

/// Build a plan with explicit parameters (tests use reduced sizes).
pub fn plan_with(seed: u64, preset: Preset, params: Params) -> Plan {
    let mut rng = derive(seed, "bodies", 0);
    let mut bodies = Vec::new();
    for m in MODELS {
        bodies.push(Body::new(m, &mut rng));
        // Two bodies of the same model for "same model, different serial" look-alikes.
        if m.model == "Canon EOS 5D Mark III" || m.model == "NIKON D750" {
            bodies.push(Body::new(m, &mut rng));
        }
    }
    let counters = bodies.iter().map(|b| b.counter_start).collect();
    let mut p = Planner {
        twelve_left: params.twelve_mp,
        params,
        rng: derive(seed, "plan", 0),
        bodies,
        counters,
        events: Vec::new(),
        paths: Paths::default(),
        next_group: 0,
        next_junk: 0,
        tasks: Vec::new(),
        mnm: Vec::new(),
        alternates: std::collections::BTreeMap::new(),
        wa_counter: 1,
    };
    p.build();
    let Planner {
        bodies,
        tasks,
        mut mnm,
        alternates,
        ..
    } = p;
    mnm.sort_by(|a, b| a.id.cmp(&b.id));
    Plan {
        seed,
        preset,
        bodies,
        tasks,
        must_not_merge: mnm,
        alternate_groups: alternates,
    }
}

impl Planner {
    fn group(&mut self) -> String {
        self.next_group += 1;
        format!("g{:05}", self.next_group)
    }

    fn junk_group(&mut self) -> String {
        self.next_junk += 1;
        format!("j{:05}", self.next_junk)
    }

    fn body_ok_for(&self, b: usize, need_no_subsec: bool, need_subsec: bool) -> bool {
        let m = self.bodies[b].model;
        (!need_no_subsec || m.subsec_digits == 0) && (!need_subsec || m.subsec_digits > 0)
    }

    fn new_event(&mut self, body: usize) -> usize {
        let rng = &mut self.rng;
        let model = self.bodies[body].model;
        let year = rng.random_range(model.years.0..=model.years.1);
        let place_i = if rng.random_bool(0.7) {
            rng.random_range(0..3)
        } else {
            rng.random_range(0..PLACES.len())
        };
        let place = &PLACES[place_i];
        let title = if place.far && rng.random_bool(0.6) {
            format!("{} Trip", place.name)
        } else {
            (*pick(rng, TITLES)).to_string()
        };
        let start = random_time_in_year(rng, year, (8, 18));
        let (y, m, d) = ymd(start);
        let rel = format!("{y:04}/{y:04}-{m:02}-{d:02} {title}");
        let owner = self.bodies[body].owner;
        let primary_dir = if model.phone && rng.random_bool(0.5) {
            format!(
                "drive-c/Phone Backups/{}'s {}/DCIM/{}",
                capitalised(owner),
                model.model,
                model.dcim
            )
        } else if rng.random_bool(0.15) {
            format!("drive-c/Photos/{rel}")
        } else {
            format!("drive-a/Pictures/{rel}")
        };
        let leaf = rel.rsplit('/').next().unwrap_or(&rel).to_string();
        let (bmin, bmax) = self.params.backups;
        let n_backups = rng.random_range(bmin..=bmax);
        let mut templates: Vec<u8> = (0..8).collect();
        let mut backups = Vec::new();
        for _ in 0..n_backups {
            if templates.is_empty() {
                break;
            }
            let t = templates.remove(rng.random_range(0..templates.len()));
            let by = (y + rng.random_range(0..=3)).min(LATEST_YEAR);
            let by2 = (by + rng.random_range(1..=4)).min(LATEST_YEAR + 1);
            let dir = match t {
                0 => format!("drive-b/Backup {by}/Old Laptop/Pictures/{rel}"),
                1 => format!("drive-b/Backup {by}/MacBook Pro/Users/{owner}/Pictures/{rel}"),
                2 => format!(
                    "drive-c/NAS/Family/Backups/Backup {by2}/USB Drive/Backup {by}/Old Laptop/Pictures/{rel}"
                ),
                3 => format!("drive-c/Photos/Archive/{y:04}/{leaf}"),
                4 => format!("drive-a/Pictures/Old Imports/{y:04}-{m:02}-{d:02}"),
                5 => format!("drive-b/Desktop Backup {by}/Photos to sort/{leaf}"),
                6 if model.phone => format!(
                    "drive-b/Backup {by}/{} Backup/DCIM/{}",
                    model.model, model.dcim
                ),
                6 => format!(
                    "drive-b/Backup {by}/Camera Cards/{}/DCIM/{}",
                    model.model, model.dcim
                ),
                _ => format!(
                    "drive-c/NAS/Family/Backups/Backup {by2}/drive-b/Backup {by}/Old Laptop/Users/{owner}/Pictures/{rel}"
                ),
            };
            if dir == primary_dir {
                continue;
            }
            let preserve = rng.random_bool(0.5);
            let copy_year = by.max(y);
            let copy_time = random_time_in_year(rng, copy_year, (0, 24)).max(start + 86_400);
            backups.push(Backup {
                dir,
                preserve,
                copy_time,
            });
        }
        self.events.push(Event {
            body,
            title,
            place: place_i,
            offset_min: place.offset_min,
            rel,
            primary_dir,
            backups,
            cursor_ms: start * 1000,
            counter_hint: 0,
        });
        self.events.len() - 1
    }

    /// An event for `body` with room for more shots (or a new one).
    fn event_for(&mut self, body: usize) -> usize {
        let candidates: Vec<usize> = self
            .events
            .iter()
            .enumerate()
            .filter(|(_, e)| e.body == body && e.counter_hint < 8)
            .map(|(i, _)| i)
            .collect();
        if !candidates.is_empty() && self.rng.random_bool(0.75) {
            candidates[self.rng.random_range(0..candidates.len())]
        } else {
            self.new_event(body)
        }
    }

    fn random_body(&mut self) -> usize {
        self.rng.random_range(0..self.bodies.len())
    }

    fn stem_ext(&mut self, body: usize, cap: &Capture) -> (String, &'static str) {
        let model = self.bodies[body].model;
        let c = &mut self.counters[body];
        *c = if *c >= 9999 { 1 } else { *c + 1 };
        let n = *c;
        let (y, m, d) = cap.ymd();
        let (hh, mm, ss) = cap.hms();
        match model.naming {
            Naming::Counter(prefix, ext) => {
                if prefix.len() == 4 {
                    (format!("{prefix}{n:04}"), ext)
                } else if prefix == "P" {
                    (format!("P{m:X}{d:02}{n:04}"), ext)
                } else {
                    (format!("{prefix}{n:05}"), ext)
                }
            }
            Naming::DateTime => (format!("{y:04}{m:02}{d:02}_{hh:02}{mm:02}{ss:02}"), "jpg"),
            Naming::Pixel => (
                format!(
                    "PXL_{y:04}{m:02}{d:02}_{hh:02}{mm:02}{ss:02}{:03}",
                    cap.millis
                ),
                "jpg",
            ),
        }
    }

    fn display_dims(&mut self, body: usize, orientation: u16) -> (u32, u32) {
        let model = self.bodies[body].model;
        let (n, d) = self.params.dim_scale;
        let (w, h) = (model.dims.0 * n / d, model.dims.1 * n / d);
        let (w, h) = (w & !1, h & !1);
        if matches!(orientation, 6 | 8) {
            (h, w)
        } else {
            (w, h)
        }
    }

    fn orientation(&mut self, body: usize) -> u16 {
        let phone = self.bodies[body].model.phone;
        let r: f64 = self.rng.random();
        let rot = if phone { 0.25 } else { 0.1 };
        if r < rot {
            6
        } else if r < rot + 0.03 {
            8
        } else if r < rot + 0.05 {
            3
        } else {
            1
        }
    }

    fn gps(&mut self, body: usize, ev: usize) -> Option<Gps> {
        let phone = self.bodies[body].model.phone;
        let p = if phone { 0.95 } else { 0.2 };
        if !self.rng.random_bool(p) {
            return None;
        }
        let pl = &PLACES[self.events[ev].place];
        Some(Gps {
            lat_e6: pl.lat_e6 + self.rng.random_range(-20_000..20_000),
            lon_e6: pl.lon_e6 + self.rng.random_range(-20_000..20_000),
            alt_dm: pl.alt_dm + self.rng.random_range(0..300),
        })
    }

    /// Advance the event clock and return the next capture.
    fn next_capture(&mut self, ev: usize, gap_ms: (i64, i64)) -> Capture {
        let gap = self.rng.random_range(gap_ms.0..gap_ms.1);
        let e = &mut self.events[ev];
        e.cursor_ms += gap;
        e.counter_hint += 1;
        Capture {
            local: e.cursor_ms.div_euclid(1000),
            millis: e.cursor_ms.rem_euclid(1000) as u16,
            offset_min: e.offset_min,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn make_shot(
        &mut self,
        ev: usize,
        body: usize,
        capture: Capture,
        scene: Scene,
        perturb: Perturb,
        orientation: u16,
        gps: Option<Gps>,
        raw: bool,
    ) -> Shot {
        let (stem, ext) = self.stem_ext(body, &capture);
        let mut display = self.display_dims(body, orientation);
        let model = self.bodies[body].model;
        if self.twelve_left > 0 && model.phone && !raw {
            self.twelve_left -= 1;
            display = if matches!(orientation, 6 | 8) {
                (3024, 4032)
            } else {
                (4032, 3024)
            };
        }
        let dir = self.events[ev].primary_dir.clone();
        let path = self.paths.claim(&dir, &format!("{stem}.{ext}"));
        let raw_path = raw.then(|| {
            let jpeg_name = path.rsplit('/').next().unwrap_or_default();
            let stem = jpeg_name
                .rsplit_once('.')
                .map(|(s, _)| s)
                .unwrap_or(jpeg_name);
            self.paths.claim(&dir, &format!("{stem}.DNG"))
        });
        let mtime = capture.utc() + self.rng.random_range(1..3);
        let quality = self.rng.random_range(92..=95);
        let exposure = Exposure::random(model, &mut self.rng);
        let group = self.group();
        Shot {
            group,
            body,
            scene,
            perturb,
            capture,
            display,
            orientation,
            gps,
            exposure,
            quality,
            path,
            raw_path,
            mtime,
            maker_seed: self.rng.random(),
            derivs: Vec::new(),
            crops: Vec::new(),
        }
    }

    fn file_name(path: &str) -> (&str, &str) {
        let name = path.rsplit('/').next().unwrap_or(path);
        match name.rsplit_once('.') {
            Some((s, e)) => (s, e),
            None => (name, ""),
        }
    }

    /// Exact copies into the event's backup folders, renamed copies and the other derivative kinds.
    fn plan_derivs(&mut self, shot: &mut Shot, ev: usize, policy: Policy) {
        let (stem, ext) = {
            let (s, e) = Self::file_name(&shot.path);
            (s.to_string(), e.to_string())
        };
        let e = &self.events[ev];
        let rel = e.rel.clone();
        let title = e.title.clone();
        let owner = self.bodies[shot.body].owner;
        let leaf = rel.rsplit('/').next().unwrap_or(&rel).to_string();
        let (y, mo, d) = shot.capture.ymd();
        let backups: Vec<(String, bool, i64)> = e
            .backups
            .iter()
            .map(|b| (b.dir.clone(), b.preserve, b.copy_time))
            .collect();
        let rng_bool = |p: &mut Planner, prob: f64| p.rng.random_bool(prob);
        let later = |p: &mut Planner, base: i64| base + p.rng.random_range(3_600..86_400 * 400);

        // Exact copies (with the DNG for RAW+JPEG pairs).
        for (dir, preserve, copy_time) in &backups {
            if !rng_bool(self, 0.9) {
                continue;
            }
            let mtime = if *preserve {
                shot.mtime
            } else {
                *copy_time + self.rng.random_range(0..600)
            };
            let path = self.paths.claim(dir, &format!("{stem}.{ext}"));
            shot.derivs.push(Deriv {
                kind: DerivKind::Exact,
                path: path.clone(),
                mtime,
                from: From::Original,
            });
            if shot.raw_path.is_some() {
                let (s, _) = Self::file_name(&path);
                let s = s.to_string();
                let rp = self.paths.claim(dir, &format!("{s}.DNG"));
                shot.derivs.push(Deriv {
                    kind: DerivKind::ExactRaw,
                    path: rp,
                    mtime,
                    from: From::Original,
                });
            }
        }

        // Renamed copy.
        let rename_p = if policy == Policy::Full { 0.55 } else { 0.4 };
        if rng_bool(self, rename_p) {
            let by = (y + self.rng.random_range(0..=3)).min(LATEST_YEAR);
            let n = self.rng.random_range(1..40);
            let (dir, name) = match self.rng.random_range(0..5) {
                0 => (
                    "drive-a/Desktop/To Sort".to_string(),
                    format!("{stem} (1).{ext}"),
                ),
                1 => (
                    format!("drive-b/Backup {by}/Old Laptop/Pictures/{rel}"),
                    format!("Copy of {stem}.{ext}"),
                ),
                2 => (
                    self.events[ev].primary_dir.clone(),
                    format!("{stem}-2.{ext}"),
                ),
                3 => (
                    format!("drive-c/Shared/{}", capitalised(owner)),
                    format!("{title} {n}.jpg"),
                ),
                _ => (
                    "drive-a/Pictures/Favorites".to_string(),
                    format!("{y:04}-{mo:02}-{d:02} {title} {n}.jpg"),
                ),
            };
            let path = self.paths.claim(&dir, &name);
            let mtime = if rng_bool(self, 0.5) {
                shot.mtime
            } else {
                later(self, shot.mtime)
            };
            shot.derivs.push(Deriv {
                kind: DerivKind::Renamed,
                path,
                mtime,
                from: From::Original,
            });
        }
        if policy == Policy::ExactOnly {
            return;
        }

        // Resized copies.
        let (resize_p, keep_p) = if policy == Policy::Full {
            (0.8, 0.5)
        } else {
            (0.55, 1.0)
        };
        if rng_bool(self, resize_p) {
            let n = if rng_bool(self, 0.35) { 2 } else { 1 };
            let mut sizes = vec![1024u32, 384, 160];
            for _ in 0..n {
                let long = sizes.remove(self.rng.random_range(0..sizes.len()));
                let keep_exif = rng_bool(self, keep_p);
                let quality = self.rng.random_range(80..=90);
                let (dir, name) = match self.rng.random_range(0..3) {
                    0 => (
                        format!("drive-a/Pictures/Exports/{leaf}"),
                        format!("{stem}_{long}.jpg"),
                    ),
                    1 => (
                        format!("drive-b/Email Attachments/{y:04}"),
                        format!(
                            "{stem}-{}.jpg",
                            if long <= 384 { "small" } else { "medium" }
                        ),
                    ),
                    _ => (
                        format!("drive-c/Website/gallery/{y:04}/{}", slug(&title)),
                        format!("{stem}_{long}px.jpg"),
                    ),
                };
                let path = self.paths.claim(&dir, &name);
                let mtime = later(self, shot.mtime);
                shot.derivs.push(Deriv {
                    kind: DerivKind::Resized {
                        long,
                        quality,
                        keep_exif,
                    },
                    path: path.clone(),
                    mtime,
                    from: From::Original,
                });
                // Exports get backed up too: an exact copy of a derivative.
                if rng_bool(self, 0.25) {
                    let idx = shot.derivs.len() - 1;
                    let by = (y + self.rng.random_range(1..=4)).min(LATEST_YEAR);
                    let (_, fname) = path.rsplit_once('/').unwrap_or(("", &path));
                    let fname = fname.to_string();
                    let cp = self.paths.claim(
                        &format!("drive-b/Backup {by}/Old Laptop/Pictures/Exports/{leaf}"),
                        &fname,
                    );
                    let mt = if rng_bool(self, 0.5) {
                        mtime
                    } else {
                        later(self, mtime)
                    };
                    shot.derivs.push(Deriv {
                        kind: DerivKind::Exact,
                        path: cp,
                        mtime: mt,
                        from: From::Deriv(idx),
                    });
                }
            }
        }

        // Recompressed copy.
        let (recomp_p, keep_p) = if policy == Policy::Full {
            (0.55, 0.55)
        } else {
            (0.35, 1.0)
        };
        if rng_bool(self, recomp_p) {
            let quality = self.rng.random_range(60..=85);
            let keep_exif = rng_bool(self, keep_p);
            let (dir, name) = if keep_exif {
                if rng_bool(self, 0.5) {
                    (
                        format!("drive-a/Pictures/Exports/{leaf}/Quality {quality}"),
                        format!("{stem}.jpg"),
                    )
                } else {
                    ("drive-c/Shared/Prints".to_string(), format!("{stem}.jpg"))
                }
            } else if rng_bool(self, 0.5) {
                let n = self.wa_counter;
                self.wa_counter += 1;
                (
                    "drive-c/WhatsApp/Media/WhatsApp Images".to_string(),
                    format!("IMG-{y:04}{mo:02}{d:02}-WA{n:04}.jpg"),
                )
            } else {
                let id = self.rng.random_range(100_000_000u64..999_999_999);
                (
                    format!(
                        "drive-b/Facebook Download/photos_and_videos/{}",
                        slug(&title)
                    ),
                    format!("{id}_{}_n.jpg", self.rng.random_range(10_000u32..99_999)),
                )
            };
            let path = self.paths.claim(&dir, &name);
            let mtime = later(self, shot.mtime);
            shot.derivs.push(Deriv {
                kind: DerivKind::Recompressed { quality, keep_exif },
                path,
                mtime,
                from: From::Original,
            });
        }
        if policy != Policy::Full {
            return;
        }

        // EXIF-stripped copy.
        if rng_bool(self, 0.7) {
            let dir = if rng_bool(self, 0.5) {
                format!("drive-c/Shared Albums/{title}")
            } else {
                format!("drive-b/Cloud Download/{y:04}")
            };
            let path = self.paths.claim(&dir, &format!("{stem}.jpg"));
            let mtime = later(self, shot.mtime);
            shot.derivs.push(Deriv {
                kind: DerivKind::Stripped,
                path,
                mtime,
                from: From::Original,
            });
        }

        // PNG conversion.
        if rng_bool(self, 0.15) {
            let dir = if rng_bool(self, 0.5) {
                "drive-a/Pictures/Edits".to_string()
            } else {
                format!(
                    "drive-b/Backup {}/Old Laptop/Pictures/Edits",
                    (y + 1).min(LATEST_YEAR)
                )
            };
            let path = self.paths.claim(&dir, &format!("{stem}.png"));
            let mtime = later(self, shot.mtime);
            shot.derivs.push(Deriv {
                kind: DerivKind::Png,
                path,
                mtime,
                from: From::Original,
            });
        }

        // Cache thumbnails/previews.
        if rng_bool(self, 0.55) {
            let n = if rng_bool(self, 0.25) { 2 } else { 1 };
            for _ in 0..n {
                let (dir, name, kind) = match self.rng.random_range(0..4) {
                    0 => {
                        let size = if rng_bool(self, 0.5) { 128 } else { 256 };
                        let sub = if size == 128 { "normal" } else { "large" };
                        (
                            format!("drive-a/Users/{owner}/.thumbnails/{sub}"),
                            format!("{}.png", hex(&mut self.rng, 32)),
                            CacheKind::Freedesktop(size),
                        )
                    }
                    1 => {
                        let h4 = hex(&mut self.rng, 4).to_uppercase();
                        let long = if rng_bool(self, 0.5) { 2048 } else { 1024 };
                        (
                            format!(
                                "drive-a/Pictures/Lightroom/Lightroom Catalog Previews.lrdata/{}/{h4}",
                                &h4[..1]
                            ),
                            format!("{}-{}.jpg", uuid(&mut self.rng), hex(&mut self.rng, 32)),
                            CacheKind::Jpeg(long, 75),
                        )
                    }
                    2 => (
                        format!(
                            "drive-c/Mac Backup/Pictures/Photos Library.photoslibrary/resources/proxies/derivatives/{}",
                            hex(&mut self.rng, 2)
                        ),
                        format!("{}_1_105_c.jpeg", uuid(&mut self.rng)),
                        CacheKind::Jpeg(1024, 80),
                    ),
                    _ => (
                        format!(
                            "drive-b/Backup {}/Old Laptop/Users/{owner}/AppData/Local/Google/Picasa2/thumbs",
                            (y + 1).min(LATEST_YEAR)
                        ),
                        format!("{}.jpg", hex(&mut self.rng, 16)),
                        CacheKind::Jpeg(512, 70),
                    ),
                };
                let path = self.paths.claim(&dir, &name);
                let mtime = later(self, shot.mtime);
                shot.derivs.push(Deriv {
                    kind: DerivKind::Cache(kind),
                    path,
                    mtime,
                    from: From::Original,
                });
            }
        }
    }

    fn crop_derivs(&mut self, crop: &mut CropPlan, ev: usize) {
        let (stem, ext) = {
            let (s, e) = Self::file_name(&crop.path);
            (s.to_string(), e.to_string())
        };
        let backups: Vec<(String, bool, i64)> = self.events[ev]
            .backups
            .iter()
            .map(|b| (b.dir.clone(), b.preserve, b.copy_time))
            .collect();
        for (dir, preserve, copy_time) in backups {
            if !self.rng.random_bool(0.6) {
                continue;
            }
            let path = self.paths.claim(&dir, &format!("{stem}.{ext}"));
            crop.derivs.push(Deriv {
                kind: DerivKind::Exact,
                path,
                mtime: if preserve {
                    crop.mtime
                } else {
                    copy_time.max(crop.mtime) + 60
                },
                from: From::Original,
            });
        }
        if self.rng.random_bool(0.4) {
            let leaf = self.events[ev]
                .rel
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string();
            let path = self.paths.claim(
                &format!("drive-a/Pictures/Exports/{leaf}"),
                &format!("{stem}_1024.jpg"),
            );
            crop.derivs.push(Deriv {
                kind: DerivKind::Resized {
                    long: 1024,
                    quality: 85,
                    keep_exif: true,
                },
                path,
                mtime: crop.mtime + 120,
                from: From::Original,
            });
        }
    }

    fn push_shot(&mut self, shot: Shot) {
        self.tasks.push(Task::Shot(Box::new(shot)));
    }

    fn mnm_id(&self) -> String {
        format!("m{:04}", self.mnm.len() + 1)
    }

    fn build(&mut self) {
        let p = self.params.clone();

        // Standalone originals (some become crop bases).
        let mut crop_bases_left = p.sets_per_kind;
        for _ in 0..p.standalone {
            let body = self.random_body();
            let ev = self.event_for(body);
            let cap = self.next_capture(ev, (20_000, 900_000));
            let scene = Scene::random(&mut self.rng);
            let grain = self.rng.random();
            let orientation = self.orientation(body);
            let gps = self.gps(body, ev);
            let raw = self.bodies[body].model.can_raw && self.rng.random_bool(p.raw_share);
            let mut shot = self.make_shot(
                ev,
                body,
                cap,
                scene,
                Perturb::identity(grain),
                orientation,
                gps,
                raw,
            );
            self.plan_derivs(&mut shot, ev, Policy::Full);
            if crop_bases_left > 0 {
                crop_bases_left -= 1;
                self.add_crop(&mut shot, ev);
            }
            self.push_shot(shot);
        }

        // Bursts (ADR-0035 variants, evenly split).
        for i in 0..p.burst_sets {
            let variant = match i % 3 {
                0 => BurstVariant::Spaced,
                1 => BurstVariant::SameSecondSubsec,
                _ => BurstVariant::SameSecondNoSubsec,
            };
            self.add_burst(variant);
        }
        for _ in 0..p.sets_per_kind {
            self.add_minutes_apart();
        }
        for i in 0..p.sets_per_kind {
            self.add_different_camera(i % 2 == 0);
        }
        for _ in 0..p.scans {
            self.add_scan();
        }
        for _ in 0..p.videos {
            self.add_video(false);
        }
        for _ in 0..p.junk_videos {
            self.add_video(true);
        }
        for i in 0..p.icons {
            self.add_junk(JunkKind::Icon, i);
        }
        for i in 0..p.ui_graphics {
            self.add_junk(
                if i % 4 == 3 {
                    JunkKind::UiGif
                } else {
                    JunkKind::UiPng
                },
                i,
            );
        }
        for i in 0..p.tiny_web {
            self.add_junk(JunkKind::TinyWeb, i);
        }
    }

    fn add_crop(&mut self, shot: &mut Shot, ev: usize) {
        // Remove 10-30% of the area and change the aspect ratio by at least 5%.
        let (w, h) = (shot.display.0 as f64, shot.display.1 as f64);
        let aspect = w / h;
        let (fw, fh) = loop {
            let area = self.rng.random_range(0.70..0.90);
            let change = self.rng.random_range(0.06..0.25)
                * if self.rng.random_bool(0.5) { 1.0 } else { -1.0 };
            let new_aspect = aspect * (1.0 + change);
            // fw * fh = area, (fw*w)/(fh*h) = new_aspect  =>  fw/fh = new_aspect / aspect
            let ratio = new_aspect / aspect;
            let fh = (area / ratio).sqrt();
            let fw = area / fh;
            if fw <= 1.0 && fh <= 1.0 {
                break (fw, fh);
            }
        };
        let x0 = self.rng.random_range(0.0..(1.0 - fw).max(1e-6));
        let y0 = self.rng.random_range(0.0..(1.0 - fh).max(1e-6));
        let q = |v: f64| (v * 10_000.0).round() as u32;
        let (stem, _) = Self::file_name(&shot.path);
        let stem = stem.to_string();
        let dir = self.events[ev].primary_dir.clone();
        let path = self.paths.claim(&dir, &format!("{stem}-Edit.jpg"));
        let group = self.group();
        let mut crop = CropPlan {
            group: group.clone(),
            rect: (q(x0), q(y0), q(fw), q(fh)),
            path: path.clone(),
            mtime: shot.mtime + self.rng.random_range(86_400..86_400 * 200),
            quality: self.rng.random_range(88..=92),
            derivs: Vec::new(),
        };
        self.crop_derivs(&mut crop, ev);
        let id = self.mnm_id();
        self.mnm.push(MustNotMerge {
            id,
            kind: MustNotMergeKind::Crop,
            variant: None,
            groups: vec![shot.group.clone(), group],
            originals: vec![shot.path.clone(), path],
        });
        shot.crops.push(crop);
    }

    fn add_burst(&mut self, variant: BurstVariant) {
        let (need_no, need_sub) = match variant {
            BurstVariant::Spaced => (false, false),
            BurstVariant::SameSecondSubsec => (false, true),
            BurstVariant::SameSecondNoSubsec => (true, false),
        };
        let candidates: Vec<usize> = (0..self.bodies.len())
            .filter(|&b| self.body_ok_for(b, need_no, need_sub))
            .collect();
        let body = candidates[self.rng.random_range(0..candidates.len())];
        let ev = self.event_for(body);
        let frames = match variant {
            BurstVariant::SameSecondNoSubsec => 2,
            _ => self.rng.random_range(2..=3),
        };
        let has_subsec = self.bodies[body].model.subsec_digits > 0;
        let mut first = self.next_capture(ev, (60_000, 900_000));
        if variant != BurstVariant::Spaced {
            // Leave room in the second for all frames.
            first.millis = self.rng.random_range(0..200);
        }
        let scene = Scene::random(&mut self.rng);
        let orientation = self.orientation(body);
        let gps = self.gps(body, ev);
        let policy = if variant == BurstVariant::SameSecondNoSubsec {
            Policy::ExactOnly
        } else {
            Policy::ExifPreserving
        };
        let mut groups = Vec::new();
        let mut originals = Vec::new();
        let mut cap = first;
        for f in 0..frames {
            if f > 0 {
                cap = match variant {
                    BurstVariant::Spaced => {
                        let mut gap = self.rng.random_range(300..5_000);
                        if !has_subsec {
                            // Without sub-seconds, frames must land in different seconds.
                            gap = gap.max(1_000);
                            let next = cap.plus_millis(gap);
                            if next.local == cap.local {
                                gap += 1_000;
                            }
                        }
                        cap.plus_millis(gap)
                    }
                    _ => {
                        let gap = self.rng.random_range(110..260);
                        let next = cap.plus_millis(gap);
                        if next.local != first.local {
                            cap.plus_millis(1)
                        } else {
                            next
                        }
                    }
                };
                if variant != BurstVariant::Spaced && (cap.local != first.local) {
                    cap = first;
                }
            }
            let mut c = cap;
            if variant == BurstVariant::SameSecondSubsec
                && f > 0
                && c.millis / 10 == first.millis / 10
            {
                c.millis = (first.millis + 100 * f as u16).min(990);
            }
            let perturb = Perturb::near(&mut self.rng, 1.0);
            let mut shot =
                self.make_shot(ev, body, c, scene.clone(), perturb, orientation, gps, false);
            // Same-second frames are written within the same second.
            if variant != BurstVariant::Spaced {
                shot.mtime = first.utc() + 1;
            }
            self.plan_derivs(&mut shot, ev, policy);
            // ADR-0039: sometimes the last frame also gets a metadata-free (EXIF-stripped) thumbnail that
            // hash-matches its siblings too; the truth accepts it with any frame, never with two.
            if f + 1 == frames && self.rng.random_bool(0.6) {
                let (stem, _) = Self::file_name(&shot.path);
                let stem = stem.to_string();
                let title = self.events[ev].title.clone();
                let long = if self.rng.random_bool(0.5) { 1024 } else { 384 };
                let quality = self.rng.random_range(75..=88);
                let path = self.paths.claim(
                    &format!("drive-c/Shared Albums/{title}"),
                    &format!("{stem}.jpg"),
                );
                let mtime = shot.mtime + self.rng.random_range(3_600..86_400 * 400);
                shot.derivs.push(Deriv {
                    kind: DerivKind::Resized {
                        long,
                        quality,
                        keep_exif: false,
                    },
                    path: path.clone(),
                    mtime,
                    from: From::Original,
                });
                let mut alts = groups.clone();
                alts.sort();
                self.alternates.insert(path, alts);
            }
            groups.push(shot.group.clone());
            originals.push(shot.path.clone());
            self.push_shot(shot);
        }
        let ev_cursor = &mut self.events[ev].cursor_ms;
        *ev_cursor = (*ev_cursor).max(cap.local * 1000 + i64::from(cap.millis));
        let id = self.mnm_id();
        self.mnm.push(MustNotMerge {
            id,
            kind: MustNotMergeKind::Burst,
            variant: Some(variant),
            groups,
            originals,
        });
    }

    fn add_minutes_apart(&mut self) {
        let body = self.random_body();
        let ev = self.event_for(body);
        let n = if self.rng.random_bool(0.25) { 3 } else { 2 };
        let scene = Scene::random(&mut self.rng);
        let orientation = self.orientation(body);
        let gps = self.gps(body, ev);
        let mut groups = Vec::new();
        let mut originals = Vec::new();
        let mut cur = scene;
        for i in 0..n {
            let cap = if i == 0 {
                self.next_capture(ev, (60_000, 600_000))
            } else {
                self.next_capture(ev, (120_000, 1_800_000))
            };
            if i > 0 {
                cur = cur.later(&mut self.rng);
            }
            let perturb = Perturb::near(&mut self.rng, 2.0);
            let mut shot =
                self.make_shot(ev, body, cap, cur.clone(), perturb, orientation, gps, false);
            self.plan_derivs(&mut shot, ev, Policy::ExifPreserving);
            groups.push(shot.group.clone());
            originals.push(shot.path.clone());
            self.push_shot(shot);
        }
        let id = self.mnm_id();
        self.mnm.push(MustNotMerge {
            id,
            kind: MustNotMergeKind::MinutesApart,
            variant: None,
            groups,
            originals,
        });
    }

    fn add_different_camera(&mut self, same_model: bool) {
        // Same model with another serial needs a model with two bodies.
        let (a, b) = if same_model {
            let pairs: Vec<(usize, usize)> = (0..self.bodies.len())
                .flat_map(|i| (i + 1..self.bodies.len()).map(move |j| (i, j)))
                .filter(|&(i, j)| std::ptr::eq(self.bodies[i].model, self.bodies[j].model))
                .collect();
            pairs[self.rng.random_range(0..pairs.len())]
        } else {
            let a = self.random_body();
            let others: Vec<usize> = (0..self.bodies.len())
                .filter(|&j| self.bodies[j].model.model != self.bodies[a].model.model)
                .collect();
            (a, others[self.rng.random_range(0..others.len())])
        };
        let ev = self.event_for(a);
        let scene = Scene::random(&mut self.rng);
        let cap_a = self.next_capture(ev, (60_000, 600_000));
        let cap_b = cap_a.plus_millis(self.rng.random_range(0..90_000));
        let orientation = self.orientation(a);
        let gps = self.gps(a, ev);
        let mut groups = Vec::new();
        let mut originals = Vec::new();
        for (body, cap) in [(a, cap_a), (b, cap_b)] {
            let perturb = Perturb::near(&mut self.rng, 1.5);
            let gps_b = if body == a { gps } else { self.gps(body, ev) };
            let mut shot = self.make_shot(
                ev,
                body,
                cap,
                scene.clone(),
                perturb,
                orientation,
                gps_b,
                false,
            );
            if body != a {
                // The other person's camera files live in their own folder on the NAS.
                let (_, name) = shot.path.rsplit_once('/').unwrap_or(("", ""));
                let name = name.to_string();
                let m = self.bodies[body].model;
                let dir = format!(
                    "drive-c/Phone Backups/{}'s {}/DCIM/{}",
                    capitalised(self.bodies[body].owner),
                    m.model,
                    m.dcim
                );
                shot.path = self.paths.claim(&dir, &name);
            }
            self.plan_derivs(&mut shot, ev, Policy::ExifPreserving);
            groups.push(shot.group.clone());
            originals.push(shot.path.clone());
            self.push_shot(shot);
        }
        let id = self.mnm_id();
        self.mnm.push(MustNotMerge {
            id,
            kind: MustNotMergeKind::DifferentCamera,
            variant: None,
            groups,
            originals,
        });
    }

    fn add_scan(&mut self) {
        let scene = Scene::random(&mut self.rng);
        let album = *pick(
            &mut self.rng,
            &[
                "Grandma Album",
                "Wedding 1985",
                "Old Prints",
                "Baby Pictures",
            ],
        );
        let n = self.rng.random_range(1..999);
        let path = self.paths.claim(
            &format!("drive-a/Pictures/Scans/{album}"),
            &format!("scan{n:04}.jpg"),
        );
        let scan_year = self.rng.random_range(2010..2020);
        let mtime = random_time_in_year(&mut self.rng, scan_year, (9, 22));
        let landscape = self.rng.random_bool(0.6);
        let (n_, d_) = self.params.dim_scale;
        let (w, h) = (1800 * n_ / d_, 1200 * n_ / d_);
        let display = if landscape { (w, h) } else { (h, w) };
        let mut copies = Vec::new();
        if self.rng.random_bool(0.8) {
            let cp = self.paths.claim(
                &format!("drive-c/NAS/Family/Scans/{album}"),
                &format!("scan{n:04}.jpg"),
            );
            copies.push((cp, mtime));
        }
        let group = self.group();
        self.tasks.push(Task::Scan(ScanPlan {
            group,
            scene,
            grain_seed: self.rng.random(),
            display,
            path,
            mtime,
            copies,
        }));
    }

    fn add_video(&mut self, junk: bool) {
        let phone_bodies: Vec<usize> = (0..self.bodies.len())
            .filter(|&b| self.bodies[b].model.can_video)
            .collect();
        let body = phone_bodies[self.rng.random_range(0..phone_bodies.len())];
        let model = self.bodies[body].model;
        if junk {
            let tiny = self.rng.random_bool(0.5);
            let (w, h) = if tiny { (240, 180) } else { (1280, 720) };
            let duration_ms = if tiny {
                self.rng.random_range(3_000..20_000)
            } else {
                self.rng.random_range(400..1_900)
            };
            let year = self.rng.random_range(2012..2025);
            let created = random_time_in_year(&mut self.rng, year, (0, 24));
            let n = self.rng.random_range(1000..9999);
            let owner = *pick(&mut self.rng, OWNERS);
            let dir = if tiny {
                format!(
                    "drive-b/Backup {}/Old Laptop/Users/{owner}/Downloads/Messenger",
                    year.min(LATEST_YEAR)
                )
            } else {
                format!("drive-a/Users/{owner}/Downloads/WhatsApp Video")
            };
            let path = self.paths.claim(&dir, &format!("VID-{n}.mp4"));
            let group = self.junk_group();
            let seed = self.rng.random();
            self.tasks.push(Task::Video(VideoPlan {
                group,
                junk: true,
                spec: crate::video::VideoSpec {
                    quicktime: false,
                    width: w,
                    height: h,
                    duration_ms,
                    created_utc: created,
                    make: None,
                    model: None,
                    location: None,
                    content_identifier: None,
                    payload: (duration_ms as usize / 1000 + 1) * 8_000,
                    seed,
                },
                path,
                mtime: created + 5,
                copies: Vec::new(),
            }));
            return;
        }
        let ev = self.event_for(body);
        let cap = self.next_capture(ev, (20_000, 600_000));
        let duration_ms = self.rng.random_range(3_000..60_000);
        let gap = duration_ms as i64;
        self.events[ev].cursor_ms += gap;
        let quicktime = model.make == "Apple";
        let (stem, _) = self.stem_ext(body, &cap);
        let (stem, ext) = match model.naming {
            Naming::Pixel => (stem, "mp4"),
            Naming::DateTime => (stem, "mp4"),
            _ if quicktime => (stem, "MOV"),
            _ => (format!("C{}", &stem[stem.len() - 4..]), "MP4"),
        };
        let dir = self.events[ev].primary_dir.clone();
        let path = self.paths.claim(&dir, &format!("{stem}.{ext}"));
        let mtime = cap.utc() + (duration_ms / 1000) as i64 + 1;
        let pl = &PLACES[self.events[ev].place];
        let location = Some(format!(
            "{:+08.4}{:+09.4}/",
            pl.lat_e6 as f64 / 1e6,
            pl.lon_e6 as f64 / 1e6
        ));
        let backups: Vec<(String, bool, i64)> = self.events[ev]
            .backups
            .iter()
            .map(|b| (b.dir.clone(), b.preserve, b.copy_time))
            .collect();
        let mut copies = Vec::new();
        for (d, preserve, t) in backups {
            if self.rng.random_bool(0.8) {
                let cp = self.paths.claim(&d, &format!("{stem}.{ext}"));
                copies.push((cp, if preserve { mtime } else { t }));
            }
        }
        let group = self.group();
        let seed = self.rng.random();
        self.tasks.push(Task::Video(VideoPlan {
            group,
            junk: false,
            spec: crate::video::VideoSpec {
                quicktime,
                width: 1920,
                height: 1080,
                duration_ms,
                created_utc: cap.utc(),
                make: Some(model.make.to_string()),
                model: Some(model.model.to_string()),
                location,
                content_identifier: None,
                payload: (duration_ms as usize / 1000 + 1) * 24_000,
                seed,
            },
            path,
            mtime,
            copies,
        }));
    }

    fn add_junk(&mut self, kind: JunkKind, i: usize) {
        let owner = *pick(&mut self.rng, OWNERS);
        let year = self.rng.random_range(2009..2025);
        let mtime = random_time_in_year(&mut self.rng, year, (0, 24));
        const APPS: &[&str] = &[
            "Paint Shop",
            "Media Player",
            "Old Game",
            "Photo Viewer",
            "Chat",
        ];
        const SITES: &[&str] = &["recipes", "news", "shop", "forum", "travel-blog", "wiki"];
        const UI: &[&str] = &[
            "button_ok",
            "logo",
            "spinner",
            "arrow_left",
            "arrow_right",
            "close",
            "banner",
            "badge",
            "star",
            "avatar_placeholder",
            "background_tile",
            "toolbar",
        ];
        let (dir, name, size) = match kind {
            JunkKind::Icon => {
                let app = *pick(&mut self.rng, APPS);
                let dir = if self.rng.random_bool(0.5) {
                    format!("drive-a/Program Files/{app}/icons")
                } else {
                    format!(
                        "drive-b/Backup {}/Old Laptop/Program Files/{app}/res",
                        year.min(LATEST_YEAR)
                    )
                };
                (dir, format!("icon_{i:03}.ico"), (256, 256))
            }
            JunkKind::UiPng | JunkKind::UiGif => {
                let w = self.rng.random_range(16..900);
                let h = self.rng.random_range(16..700);
                let dir = match self.rng.random_range(0..3) {
                    0 => "drive-a/Projects/website/assets/img".to_string(),
                    1 => format!(
                        "drive-b/Backup {}/Old Laptop/Program Files/{}/ui",
                        year.min(LATEST_YEAR),
                        pick(&mut self.rng, APPS)
                    ),
                    _ => format!("drive-c/Shared/{}/Presentation Assets", capitalised(owner)),
                };
                let ext = if kind == JunkKind::UiGif {
                    "gif"
                } else {
                    "png"
                };
                (
                    dir,
                    format!("{}_{i}.{ext}", pick(&mut self.rng, UI)),
                    (w, h),
                )
            }
            JunkKind::TinyWeb => {
                let site = *pick(&mut self.rng, SITES);
                let (w, h) = if self.rng.random_bool(0.7) {
                    (
                        self.rng.random_range(40..190),
                        self.rng.random_range(30..190),
                    )
                } else {
                    // Above 200 px on both sides but under 120k pixels.
                    (
                        self.rng.random_range(200..400),
                        self.rng.random_range(200..290),
                    )
                };
                let (w, h) = if w * h >= 120_000 { (w, 200) } else { (w, h) };
                (
                    format!("drive-a/Users/{owner}/Documents/Saved Pages/{site}_files"),
                    format!("img_{}.jpg", self.rng.random_range(1000..99999)),
                    (w, h),
                )
            }
        };
        let path = self.paths.claim(&dir, &name);
        let group = self.junk_group();
        let seed = self.rng.random();
        self.tasks.push(Task::Junk(JunkPlan {
            group,
            kind,
            size,
            path,
            mtime,
            seed,
        }));
    }
}

//! Procedural "photos": seeded scene compositions rendered to RGB pixels.
//!
//! A [`Scene`] is a resolution-independent description (horizon with hills, sky/ground or wall/floor
//! gradients, clouds, sun, 2-6 soft shapes such as trees, people, buildings and blobs, vignetting, multi-octave
//! value-noise texture). Rendering it with a [`Perturb`] (camera pan/zoom, light change, subject movement,
//! sensor grain) produces near-identical frames of the same scene for bursts and look-alikes, while distinct
//! scenes differ in their low-frequency structure so perceptual hashes stay far apart.
//!
//! Only `+ - * /` and `sqrt` are used on floats (all IEEE-exact), so renders are bit-identical on every
//! platform; no `sin`/`exp`/`powf`.

use crate::rng::{CorpusRng, Rng, mix64};

/// An 8-bit RGB image, row-major, no padding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Rgb {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; width as usize * height as usize * 3],
        }
    }

    fn px(&self, x: u32, y: u32) -> [u8; 3] {
        let i = (y as usize * self.width as usize + x as usize) * 3;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    /// Crop to the pixel rectangle `[x0, x0+w) x [y0, y0+h)`.
    pub fn crop(&self, x0: u32, y0: u32, w: u32, h: u32) -> Rgb {
        let mut out = Rgb::new(w, h);
        let row = w as usize * 3;
        for y in 0..h {
            let src = ((y0 + y) as usize * self.width as usize + x0 as usize) * 3;
            let dst = y as usize * row;
            out.data[dst..dst + row].copy_from_slice(&self.data[src..src + row]);
        }
        out
    }

    /// Area-average downscale (integer arithmetic only, so deterministic everywhere).
    pub fn downscale(&self, w: u32, h: u32) -> Rgb {
        assert!(w >= 1 && h >= 1 && w <= self.width && h <= self.height);
        let (sw, sh) = (self.width as u64, self.height as u64);
        let xb: Vec<(usize, usize)> = (0..w as u64)
            .map(|i| {
                let a = (i * sw / w as u64) as usize;
                let b = (((i + 1) * sw / w as u64) as usize).max(a + 1);
                (a, b)
            })
            .collect();
        let mut out = Rgb::new(w, h);
        let mut acc = vec![0u32; w as usize * 3];
        for oy in 0..h as u64 {
            let y0 = (oy * sh / h as u64) as usize;
            let y1 = (((oy + 1) * sh / h as u64) as usize).max(y0 + 1);
            acc.iter_mut().for_each(|a| *a = 0);
            for y in y0..y1 {
                let row =
                    &self.data[y * self.width as usize * 3..(y + 1) * self.width as usize * 3];
                for (ox, &(a, b)) in xb.iter().enumerate() {
                    let (mut r, mut g, mut bl) = (0u32, 0u32, 0u32);
                    for x in a..b {
                        r += u32::from(row[x * 3]);
                        g += u32::from(row[x * 3 + 1]);
                        bl += u32::from(row[x * 3 + 2]);
                    }
                    acc[ox * 3] += r;
                    acc[ox * 3 + 1] += g;
                    acc[ox * 3 + 2] += bl;
                }
            }
            let rows = (y1 - y0) as u32;
            let dst =
                &mut out.data[oy as usize * w as usize * 3..(oy as usize + 1) * w as usize * 3];
            for (ox, &(a, b)) in xb.iter().enumerate() {
                let n = rows * (b - a) as u32;
                for c in 0..3 {
                    dst[ox * 3 + c] = ((acc[ox * 3 + c] + n / 2) / n) as u8;
                }
            }
        }
        out
    }

    /// Downscale so the long edge is `long` px (keeps aspect; never upscales).
    pub fn fit_long_edge(&self, long: u32) -> Rgb {
        let (w, h) = fit_long_edge(self.width, self.height, long);
        if (w, h) == (self.width, self.height) {
            self.clone()
        } else {
            self.downscale(w, h)
        }
    }

    /// Rotate 90 degrees clockwise.
    pub fn rotate_cw(&self) -> Rgb {
        let mut out = Rgb::new(self.height, self.width);
        for y in 0..self.height {
            for x in 0..self.width {
                let p = self.px(x, y);
                let (nx, ny) = (self.height - 1 - y, x);
                let i = (ny as usize * out.width as usize + nx as usize) * 3;
                out.data[i..i + 3].copy_from_slice(&p);
            }
        }
        out
    }

    /// Rotate 90 degrees counter-clockwise.
    pub fn rotate_ccw(&self) -> Rgb {
        let mut out = Rgb::new(self.height, self.width);
        for y in 0..self.height {
            for x in 0..self.width {
                let p = self.px(x, y);
                let (nx, ny) = (y, self.width - 1 - x);
                let i = (ny as usize * out.width as usize + nx as usize) * 3;
                out.data[i..i + 3].copy_from_slice(&p);
            }
        }
        out
    }

    pub fn rotate_180(&self) -> Rgb {
        let mut out = self.clone();
        let px: Vec<[u8; 3]> = self.data.as_chunks::<3>().0.iter().rev().copied().collect();
        for (dst, p) in out.data.as_chunks_mut::<3>().0.iter_mut().zip(px) {
            dst.copy_from_slice(&p);
        }
        out
    }

    /// Stored (sensor) pixels for display pixels `self` and EXIF orientation `o` (1, 3, 6, 8).
    pub fn to_stored(&self, o: u16) -> Rgb {
        match o {
            3 => self.rotate_180(),
            6 => self.rotate_ccw(),
            8 => self.rotate_cw(),
            _ => self.clone(),
        }
    }
}

/// Dimensions with long edge `long`, keeping the aspect of `w x h` (never upscales).
pub fn fit_long_edge(w: u32, h: u32, long: u32) -> (u32, u32) {
    let m = w.max(h);
    if m <= long {
        return (w, h);
    }
    let scale =
        |v: u32| ((u64::from(v) * u64::from(long) + u64::from(m) / 2) / u64::from(m)).max(1) as u32;
    (scale(w), scale(h))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShapeKind {
    Ellipse,
    /// Building / box with a window grid.
    Building,
    /// Round crown on a trunk.
    Tree,
    /// Head + body silhouette.
    Person,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub kind: ShapeKind,
    pub cx: f32,
    pub cy: f32,
    pub rx: f32,
    pub ry: f32,
    /// Unit rotation vector (cos, sin).
    pub rot: (f32, f32),
    pub color: [f32; 3],
    pub shade: f32,
    pub soft: f32,
}

/// A resolution-independent scene. Coordinates: y in [-0.5, 0.5] top to bottom, x centred, in units of the
/// image height.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub seed: u64,
    pub indoor: bool,
    pub horizon: f32,
    pub tilt: f32,
    pub hill_amp: f32,
    pub hill_freq: f32,
    pub top: [f32; 3],
    pub upper: [f32; 3],
    pub far: [f32; 3],
    pub near: [f32; 3],
    pub cloud_amp: f32,
    pub texture_amp: f32,
    pub fine_amp: f32,
    pub sun: Option<(f32, f32, f32)>,
    pub shapes: Vec<Shape>,
    pub vignette: f32,
}

/// Per-frame variation of a scene.
#[derive(Clone, Debug, PartialEq)]
pub struct Perturb {
    pub dx: f32,
    pub dy: f32,
    pub zoom: f32,
    pub light: f32,
    /// Per-shape movement (subjects move between frames), in image heights.
    pub jitter: f32,
    pub jitter_seed: u64,
    pub grain: f32,
    pub grain_seed: u64,
}

impl Perturb {
    pub fn identity(grain_seed: u64) -> Self {
        Self {
            dx: 0.0,
            dy: 0.0,
            zoom: 1.0,
            light: 1.0,
            jitter: 0.0,
            jitter_seed: 0,
            grain: 5.0,
            grain_seed,
        }
    }

    /// A near-identical frame: tiny pan, slight subject movement (bursts, look-alikes).
    pub fn near(rng: &mut CorpusRng, strength: f32) -> Self {
        Self {
            dx: rng.random_range(-0.012..0.012) * strength,
            dy: rng.random_range(-0.008..0.008) * strength,
            zoom: 1.0 + rng.random_range(-0.01..0.01) * strength,
            light: 1.0 + rng.random_range(-0.03..0.03) * strength,
            jitter: 0.01 * strength,
            jitter_seed: rng.random(),
            grain: rng.random_range(3.0..7.0),
            grain_seed: rng.random(),
        }
    }
}

fn rand_color(rng: &mut CorpusRng, lo: [f32; 3], hi: [f32; 3]) -> [f32; 3] {
    [
        rng.random_range(lo[0]..hi[0]),
        rng.random_range(lo[1]..hi[1]),
        rng.random_range(lo[2]..hi[2]),
    ]
}

fn unit_vec(rng: &mut CorpusRng, max_angle_frac: f32) -> (f32, f32) {
    // Small rotation without trig: (1, t) normalised, t in [-max, max].
    let t = rng.random_range(-max_angle_frac..max_angle_frac.max(1e-6));
    let n = (1.0 + t * t).sqrt();
    (1.0 / n, t / n)
}

impl Scene {
    /// A random scene.
    pub fn random(rng: &mut CorpusRng) -> Scene {
        let seed: u64 = rng.random();
        let indoor = rng.random_bool(0.25);
        let (top, upper, far, near) = if indoor {
            let wall = rand_color(rng, [120.0, 100.0, 80.0], [235.0, 225.0, 210.0]);
            let floor = rand_color(rng, [60.0, 40.0, 25.0], [170.0, 130.0, 100.0]);
            (
                [wall[0] * 0.8, wall[1] * 0.8, wall[2] * 0.8],
                wall,
                floor,
                [floor[0] * 0.7, floor[1] * 0.7, floor[2] * 0.7],
            )
        } else {
            let sky = match rng.random_range(0..4) {
                0 => (
                    rand_color(rng, [30.0, 70.0, 150.0], [90.0, 130.0, 220.0]),
                    rand_color(rng, [140.0, 180.0, 220.0], [200.0, 225.0, 255.0]),
                ),
                1 => (
                    rand_color(rng, [70.0, 50.0, 100.0], [140.0, 90.0, 160.0]),
                    rand_color(rng, [220.0, 120.0, 60.0], [255.0, 190.0, 130.0]),
                ),
                2 => (
                    rand_color(rng, [120.0, 120.0, 125.0], [170.0, 170.0, 175.0]),
                    rand_color(rng, [180.0, 180.0, 185.0], [225.0, 225.0, 230.0]),
                ),
                _ => (
                    rand_color(rng, [10.0, 15.0, 40.0], [40.0, 50.0, 90.0]),
                    rand_color(rng, [60.0, 60.0, 110.0], [110.0, 100.0, 150.0]),
                ),
            };
            let ground = match rng.random_range(0..5) {
                0 => rand_color(rng, [40.0, 90.0, 20.0], [110.0, 160.0, 70.0]),
                1 => rand_color(rng, [170.0, 150.0, 100.0], [235.0, 215.0, 170.0]),
                2 => rand_color(rng, [20.0, 60.0, 100.0], [70.0, 130.0, 180.0]),
                3 => rand_color(rng, [200.0, 205.0, 210.0], [245.0, 248.0, 252.0]),
                _ => rand_color(rng, [60.0, 55.0, 50.0], [120.0, 110.0, 100.0]),
            };
            let near = [ground[0] * 0.6, ground[1] * 0.6, ground[2] * 0.6];
            (sky.0, sky.1, ground, near)
        };
        let n_shapes = rng.random_range(2..=6);
        let mut shapes = Vec::with_capacity(n_shapes);
        let horizon: f32 = rng.random_range(-0.3..0.3);
        for _ in 0..n_shapes {
            let kind = match rng.random_range(0..10) {
                0..=2 => ShapeKind::Ellipse,
                3..=4 => ShapeKind::Building,
                5..=6 => ShapeKind::Tree,
                _ => ShapeKind::Person,
            };
            let scale = rng.random_range(0.06..0.32);
            let (rx, ry) = match kind {
                ShapeKind::Ellipse => (scale * rng.random_range(0.6..1.6), scale),
                ShapeKind::Building => (
                    scale * rng.random_range(0.5..1.2),
                    scale * rng.random_range(0.8..1.8),
                ),
                ShapeKind::Tree => (scale * 0.7, scale * 1.3),
                ShapeKind::Person => (scale * 0.35, scale * 1.1),
            };
            let cx = rng.random_range(-0.75..0.75);
            let cy = match kind {
                ShapeKind::Ellipse => rng.random_range(-0.4..0.4),
                // Standing on the ground.
                _ => (horizon + rng.random_range(0.0..0.25) - ry * 0.6).clamp(-0.45, 0.45),
            };
            let color = rand_color(rng, [10.0, 10.0, 10.0], [250.0, 250.0, 250.0]);
            shapes.push(Shape {
                kind,
                cx,
                cy,
                rx,
                ry,
                rot: if kind == ShapeKind::Ellipse {
                    unit_vec(rng, 0.8)
                } else {
                    unit_vec(rng, 0.05)
                },
                color,
                shade: rng.random_range(0.1..0.5),
                soft: rng.random_range(0.02..0.25),
            });
        }
        let sun = (!indoor && rng.random_bool(0.35)).then(|| {
            (
                rng.random_range(-0.6..0.6),
                rng.random_range(-0.45..(horizon - 0.05).max(-0.44)),
                rng.random_range(0.03..0.09),
            )
        });
        Scene {
            seed,
            indoor,
            horizon,
            tilt: rng.random_range(-0.15..0.15),
            hill_amp: if indoor {
                0.0
            } else {
                rng.random_range(0.0..0.18)
            },
            hill_freq: rng.random_range(1.5..6.0),
            top,
            upper,
            far,
            near,
            cloud_amp: if indoor {
                0.0
            } else {
                rng.random_range(0.0..60.0)
            },
            texture_amp: rng.random_range(0.15..0.5),
            fine_amp: rng.random_range(0.04..0.14),
            sun,
            shapes,
            vignette: rng.random_range(0.1..0.6),
        }
    }

    /// The same place a few minutes later: light shifted, subjects moved more than in a burst.
    pub fn later(&self, rng: &mut CorpusRng) -> Scene {
        let mut s = self.clone();
        for sh in &mut s.shapes {
            if matches!(sh.kind, ShapeKind::Person | ShapeKind::Ellipse) {
                sh.cx += rng.random_range(-0.05..0.05);
                sh.cy += rng.random_range(-0.02..0.02);
            }
        }
        let k = rng.random_range(0.9..1.1);
        for c in [&mut s.top, &mut s.upper] {
            for v in c.iter_mut() {
                *v = (*v * k).min(255.0);
            }
        }
        s
    }
}

/// The sky of an outdoor scene, read back from its colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sky {
    Blue,
    Sunset,
    Overcast,
    Night,
}

/// The ground of a scene, read back from its colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ground {
    Grass,
    Sand,
    Water,
    Snow,
    Rock,
    /// Indoor floor.
    Floor,
}

/// What a scene shows (the shapes it draws, its sky and ground), so demo data can describe the image that was
/// rendered from it instead of inventing a subject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneSummary {
    pub indoor: bool,
    /// Person silhouettes, split by drawn size.
    pub adults: u32,
    pub children: u32,
    pub buildings: u32,
    pub trees: u32,
    /// Plain ellipses (round shapes with no further detail).
    pub round_shapes: u32,
    /// `None` indoors.
    pub sky: Option<Sky>,
    pub ground: Ground,
    pub hills: bool,
    pub sun: bool,
}

/// A person this tall (in image heights) or taller reads as an adult.
const ADULT_HEIGHT: f32 = 0.16;
/// Shapes whose centre lies within this distance (image heights) of the frame's left or right edge, or beyond
/// it, are mostly out of frame.
const IN_FRAME_MARGIN: f32 = 0.05;

impl Scene {
    /// What this scene shows in a 4:3 landscape frame ([`Scene::summary_in`]).
    pub fn summary(&self) -> SceneSummary {
        self.summary_in(4.0 / 3.0)
    }

    /// What this scene shows in a frame of `aspect` (width / height) ([`SceneSummary`]). x is in image heights
    /// from the centre, so a portrait frame shows a narrower part of the scene; shapes centred outside the
    /// frame (or at its very edge) are not counted.
    pub fn summary_in(&self, aspect: f32) -> SceneSummary {
        let in_frame = aspect / 2.0 - IN_FRAME_MARGIN;
        let mut s = SceneSummary {
            indoor: self.indoor,
            adults: 0,
            children: 0,
            buildings: 0,
            trees: 0,
            round_shapes: 0,
            sky: (!self.indoor).then(|| sky_of(self.top, self.upper)),
            ground: if self.indoor {
                Ground::Floor
            } else {
                ground_of(self.far)
            },
            hills: !self.indoor && self.hill_amp > 0.09,
            sun: self.sun.is_some(),
        };
        for sh in self.shapes.iter().filter(|sh| sh.cx.abs() <= in_frame) {
            match sh.kind {
                ShapeKind::Person if sh.ry >= ADULT_HEIGHT => s.adults += 1,
                ShapeKind::Person => s.children += 1,
                ShapeKind::Building => s.buildings += 1,
                ShapeKind::Tree => s.trees += 1,
                ShapeKind::Ellipse => s.round_shapes += 1,
            }
        }
        s
    }
}

/// The sky palettes of [`Scene::random`], told apart by colour.
fn sky_of(top: [f32; 3], upper: [f32; 3]) -> Sky {
    let [r, g, b] = top;
    if r < 45.0 && g < 55.0 && b < 95.0 {
        Sky::Night
    } else if (r - g).abs() < 12.0 && (r - b).abs() < 12.0 {
        Sky::Overcast
    } else if upper[0] > 215.0 && upper[2] < 135.0 {
        Sky::Sunset
    } else {
        Sky::Blue
    }
}

/// The ground palettes of [`Scene::random`], told apart by colour.
fn ground_of([r, g, b]: [f32; 3]) -> Ground {
    if r >= 195.0 && g >= 200.0 && b >= 205.0 {
        Ground::Snow
    } else if b > r + 15.0 && b > g {
        Ground::Water
    } else if g > r && g > b {
        Ground::Grass
    } else if r >= 165.0 && g >= 145.0 && b >= 95.0 {
        Ground::Sand
    } else {
        Ground::Rock
    }
}

// ---------------------------------------------------------------------------------------------------------
// Noise

#[inline]
fn lattice(ix: i32, iy: i32, seed: u32) -> f32 {
    let mut h =
        (ix as u32).wrapping_mul(0x8DA6_B343) ^ (iy as u32).wrapping_mul(0xD816_3841) ^ seed;
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h >> 8) as f32 * (1.0 / 16_777_216.0)
}

#[inline]
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Fills `out[i]` with value noise at (x0 + i*dx, y) * freq for one row; lattice values are reused while the
/// cell does not change.
fn noise_row(out: &mut [f32], xs: &[f32], y: f32, freq: f32, seed: u32, add_amp: f32) {
    let fy = y * freq;
    let iy = fy.floor();
    let ty = smooth(fy - iy);
    let iy = iy as i32;
    let mut cur = i32::MIN;
    let (mut a, mut b) = (0.0f32, 0.0f32); // left/right column values (interpolated in y)
    for (o, &x) in out.iter_mut().zip(xs) {
        let fx = x * freq;
        let ixf = fx.floor();
        let ix = ixf as i32;
        if ix != cur {
            cur = ix;
            let v00 = lattice(ix, iy, seed);
            let v01 = lattice(ix, iy + 1, seed);
            let v10 = lattice(ix + 1, iy, seed);
            let v11 = lattice(ix + 1, iy + 1, seed);
            a = v00 + (v01 - v00) * ty;
            b = v10 + (v11 - v10) * ty;
        }
        let tx = smooth(fx - ixf);
        *o += (a + (b - a) * tx - 0.5) * add_amp;
    }
}

fn noise1(x: f32, seed: u32) -> f32 {
    let ix = x.floor();
    let t = smooth(x - ix);
    let a = lattice(ix as i32, 0, seed);
    let b = lattice(ix as i32 + 1, 0, seed);
    a + (b - a) * t
}

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Shape coverage (0..1) and shading factor at (x, y).
fn shape_alpha(s: &Shape, x: f32, y: f32) -> (f32, f32) {
    let (c, sn) = s.rot;
    let dx = x - s.cx;
    let dy = y - s.cy;
    let u = (dx * c + dy * sn) / s.rx;
    let v = (-dx * sn + dy * c) / s.ry;
    let shade = 1.0 - s.shade * 0.5 * (v + u * 0.3);
    let soft = s.soft;
    let a = match s.kind {
        ShapeKind::Ellipse => {
            let d = u * u + v * v;
            1.0 - smoothstep(1.0 - soft, 1.0 + soft, d)
        }
        ShapeKind::Building => {
            let d = u.abs().max(v.abs());
            let body = 1.0 - smoothstep(1.0 - soft * 0.1, 1.0, d);
            // Window grid: darker cells.
            let wx = (u + 1.0) * 3.0;
            let wy = (v + 1.0) * 4.0;
            let win = (wx - wx.floor() > 0.3) && (wy - wy.floor() > 0.35) && d < 0.85;
            return (body, if win { shade * 0.45 } else { shade });
        }
        ShapeKind::Tree => {
            let crown = {
                let cv = v + 0.35;
                let d = u * u + (cv * 1.4) * (cv * 1.4);
                1.0 - smoothstep(0.7 - soft, 0.7 + soft, d)
            };
            let trunk = if v > 0.2 && v < 1.0 && u.abs() < 0.12 {
                1.0
            } else {
                0.0
            };
            let a: f32 = crown.max(trunk);
            return (a, if crown < trunk { shade * 0.45 } else { shade });
        }
        ShapeKind::Person => {
            let head = {
                let hv = (v + 0.75) * 2.2;
                let hu = u * 1.6;
                1.0 - smoothstep(0.8, 1.0, hu * hu + hv * hv)
            };
            let body = {
                let bv = (v - 0.2) * 1.3;
                1.0 - smoothstep(0.85, 1.0, u * u * 0.8 + bv * bv)
            };
            head.max(body)
        }
    };
    (a, shade)
}

/// Render `scene` with `p` at `width x height` display pixels.
pub fn render(scene: &Scene, p: &Perturb, width: u32, height: u32) -> Rgb {
    let mut img = Rgb::new(width, height);
    let w = width as usize;
    let inv_h = 1.0 / height as f32;
    let half_w = width as f32 * 0.5;
    let half_h = height as f32 * 0.5;
    let zoom = 1.0 / p.zoom;
    let seed32 = (scene.seed ^ (scene.seed >> 32)) as u32;

    // Jittered shapes for this frame.
    let shapes: Vec<Shape> = scene
        .shapes
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mut s = s.clone();
            if p.jitter > 0.0 && matches!(s.kind, ShapeKind::Person | ShapeKind::Ellipse) {
                let h = mix64(p.jitter_seed ^ (i as u64 + 1));
                let jx = ((h & 0xFFFF) as f32 / 65535.0 - 0.5) * 2.0 * p.jitter;
                let jy = (((h >> 16) & 0xFFFF) as f32 / 65535.0 - 0.5) * p.jitter;
                s.cx += jx;
                s.cy += jy;
            }
            s
        })
        .collect();
    // Bounding boxes in scene coordinates.
    let bbox: Vec<(f32, f32, f32, f32)> = shapes
        .iter()
        .map(|s| {
            let r = s.rx.max(s.ry) * (1.0 + s.soft) * 1.3;
            (s.cx - r, s.cx + r, s.cy - r, s.cy + r)
        })
        .collect();

    // Per-column scene x and horizon.
    let xs: Vec<f32> = (0..w)
        .map(|i| ((i as f32 + 0.5 - half_w) * inv_h) * zoom + p.dx)
        .collect();
    let horizon: Vec<f32> = xs
        .iter()
        .map(|&x| {
            scene.horizon
                + scene.tilt * x
                + scene.hill_amp * (noise1(x * scene.hill_freq + 10.0, seed32 ^ 0x51) - 0.5)
                + scene.hill_amp * 0.4 * (noise1(x * scene.hill_freq * 3.0, seed32 ^ 0x77) - 0.5)
        })
        .collect();

    let mut low = vec![0.0f32; w];
    let mut fine = vec![0.0f32; w];
    let mut grain_state: u64;
    let tex_freq = 9.0;
    for (row_i, row) in img.data.chunks_exact_mut(w * 3).enumerate() {
        let y = ((row_i as f32 + 0.5 - half_h) * inv_h) * zoom + p.dy;
        // Low-frequency (clouds / ground texture) and fine texture noise for the row.
        low.iter_mut().for_each(|v| *v = 0.0);
        fine.iter_mut().for_each(|v| *v = 0.0);
        noise_row(&mut low, &xs, y, 3.0, seed32 ^ 0x1001, 1.0);
        noise_row(&mut low, &xs, y, tex_freq, seed32 ^ 0x2002, 0.5);
        noise_row(&mut fine, &xs, y, 70.0, seed32 ^ 0x3003, 1.0);
        noise_row(&mut fine, &xs, y, 260.0, seed32 ^ 0x4004, 0.6);
        let active: Vec<usize> = (0..shapes.len())
            .filter(|&i| y >= bbox[i].2 && y <= bbox[i].3)
            .collect();
        grain_state = mix64(p.grain_seed ^ (row_i as u64).wrapping_mul(0x9E37_79B9));
        for (col, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let x = xs[col];
            let hy = horizon[col];
            let mut c: [f32; 3];
            if y < hy {
                let t = ((y + 0.5) / (hy + 0.5 + 1e-3)).clamp(0.0, 1.0);
                c = [
                    scene.top[0] + (scene.upper[0] - scene.top[0]) * t,
                    scene.top[1] + (scene.upper[1] - scene.top[1]) * t,
                    scene.top[2] + (scene.upper[2] - scene.top[2]) * t,
                ];
                let cl = (low[col] * scene.cloud_amp).max(0.0);
                c = [c[0] + cl, c[1] + cl, c[2] + cl];
                if let Some((sx, sy, sr)) = scene.sun {
                    let d = (x - sx) * (x - sx) + (y - sy) * (y - sy);
                    let g = 1.0 - smoothstep(0.0, sr * sr * 6.0, d);
                    let disc = 1.0 - smoothstep(sr * sr * 0.8, sr * sr, d);
                    let k = g * 70.0 + disc * 200.0;
                    c = [c[0] + k, c[1] + k * 0.95, c[2] + k * 0.75];
                }
            } else {
                let t = ((y - hy) / (0.5 - hy + 1e-3)).clamp(0.0, 1.0);
                let tex = 1.0 + scene.texture_amp * low[col];
                c = [
                    (scene.far[0] + (scene.near[0] - scene.far[0]) * t) * tex,
                    (scene.far[1] + (scene.near[1] - scene.far[1]) * t) * tex,
                    (scene.far[2] + (scene.near[2] - scene.far[2]) * t) * tex,
                ];
            }
            for &si in &active {
                let b = bbox[si];
                if x < b.0 || x > b.1 {
                    continue;
                }
                let (a, shade) = shape_alpha(&shapes[si], x, y);
                if a > 0.0 {
                    let s = &shapes[si];
                    let tex = 1.0 + low[col] * 0.25;
                    for (ck, sk) in c.iter_mut().zip(s.color) {
                        *ck += (sk * shade * tex - *ck) * a;
                    }
                }
            }
            let vig = 1.0 - scene.vignette * (x * x * 0.6 + y * y);
            let f = (1.0 + scene.fine_amp * fine[col]) * vig * p.light;
            grain_state ^= grain_state << 13;
            grain_state ^= grain_state >> 7;
            grain_state ^= grain_state << 17;
            let g = ((grain_state & 0xFF) as f32 - 127.5) * (p.grain / 127.5);
            let gc = (((grain_state >> 8) & 0xFF) as f32 - 127.5) * (p.grain * 0.3 / 127.5);
            px[0] = (c[0] * f + g + gc).clamp(0.0, 255.0) as u8;
            px[1] = (c[1] * f + g).clamp(0.0, 255.0) as u8;
            px[2] = (c[2] * f + g - gc).clamp(0.0, 255.0) as u8;
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::derive;

    #[test]
    fn a_portrait_frame_counts_only_the_shapes_inside_it() {
        let mut scene = Scene::random(&mut derive(3, "s", 0));
        let shape = |kind, cx| Shape {
            kind,
            cx,
            cy: 0.1,
            rx: 0.05,
            ry: 0.2,
            rot: (1.0, 0.0),
            color: [200.0, 100.0, 50.0],
            shade: 0.0,
            soft: 0.0,
        };
        scene.shapes = vec![
            shape(ShapeKind::Person, -0.2),
            shape(ShapeKind::Building, 0.5),
            shape(ShapeKind::Tree, 0.31),
        ];
        let wide = scene.summary_in(1.5);
        assert_eq!((wide.adults, wide.buildings, wide.trees), (1, 1, 1));
        // 2:3 portrait: half the width is 0.33 image heights; the tree at 0.31 sits on the edge.
        let tall = scene.summary_in(2.0 / 3.0);
        assert_eq!((tall.adults, tall.buildings, tall.trees), (1, 0, 0));
        assert_eq!(scene.summary(), scene.summary_in(4.0 / 3.0));
    }

    #[test]
    fn render_is_deterministic_and_textured() {
        let scene = Scene::random(&mut derive(1, "s", 0));
        let p = Perturb::identity(9);
        let a = render(&scene, &p, 300, 200);
        let b = render(&scene, &p, 300, 200);
        assert_eq!(a, b);
        // Not flat: many distinct values.
        let mut seen = std::collections::BTreeSet::new();
        for c in a.data.as_chunks::<3>().0 {
            seen.insert((c[0], c[1], c[2]));
        }
        assert!(seen.len() > 5000, "only {} colours", seen.len());
    }

    #[test]
    fn rotations_and_downscale() {
        let scene = Scene::random(&mut derive(2, "s", 0));
        let img = render(&scene, &Perturb::identity(1), 40, 30);
        assert_eq!(img.rotate_cw().rotate_ccw(), img);
        assert_eq!(img.rotate_180().rotate_180(), img);
        assert_eq!(img.to_stored(6).rotate_cw(), img);
        let small = img.downscale(20, 15);
        assert_eq!((small.width, small.height), (20, 15));
        assert_eq!(fit_long_edge(4000, 3000, 1024), (1024, 768));
        assert_eq!(fit_long_edge(100, 50, 1024), (100, 50));
    }
}

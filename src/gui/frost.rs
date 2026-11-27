//! CPU material bakes for the slab: the blurred world, its crop under the slab,
//! the glass tint and film grain, and the procedural atmosphere used when no
//! wallpaper is configured.
//!
//! GPUI 0.2.2 has no backdrop sampling or custom shaders, and the world behind
//! the slab is a static image. So the frosted slab is a texture: the wallpaper,
//! blurred once per image ([`World::new`]), then cropped to the slab's window
//! rectangle with the same cover/contain mapping the stage uses, tinted with the
//! glass gradient and grained, once per slab geometry ([`bake`]). The result is
//! opaque and painted with the slab's corner radii, so the GPU work per frame is
//! one textured quad. Everything here runs on background executors.
//!
//! Pixel buffers are BGRA, the order GPUI's `RenderImage` expects.
use crate::appearance::Fit;

/// Linear RGB in 0..1 plus alpha, the bake's working color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba(pub [f32; 4]);
impl From<gpui::Hsla> for Rgba {
    fn from(color: gpui::Hsla) -> Self {
        let c: gpui::Rgba = color.into();
        Self([c.r, c.g, c.b, c.a])
    }
}

/// A reduced, optionally blurred copy of the world image, in RGB floats.
pub struct World {
    width: usize,
    height: usize,
    /// Size of the image the stage paints, for identical cover mapping.
    pub source: (u32, u32),
    pixels: Vec<[f32; 3]>,
}
impl World {
    /// Downscales `bgra` (`width`×`height`) to at most `max_width` columns with a
    /// box filter, then applies `radius` (in reduced pixels) of three-pass box
    /// blur, which approximates a Gaussian. `radius == 0` keeps it sharp.
    pub fn new(bgra: &[u8], width: u32, height: u32, max_width: u32, radius: usize) -> Self {
        let (sw, sh) = (width.max(1) as usize, height.max(1) as usize);
        let factor = (sw as f32 / max_width.max(1) as f32).max(1.);
        let w = ((sw as f32 / factor).round() as usize).max(1);
        let h = ((sh as f32 / factor).round() as usize).max(1);
        let mut pixels = vec![[0f32; 3]; w * h];
        for (y, row) in pixels.chunks_mut(w).enumerate() {
            let y0 = y * sh / h;
            let y1 = ((y + 1) * sh / h).max(y0 + 1).min(sh);
            for (x, out) in row.iter_mut().enumerate() {
                let x0 = x * sw / w;
                let x1 = ((x + 1) * sw / w).max(x0 + 1).min(sw);
                let mut sum = [0f32; 3];
                for sy in y0..y1 {
                    let line = &bgra[(sy * sw) * 4..];
                    for sx in x0..x1 {
                        let p = &line[sx * 4..sx * 4 + 3];
                        sum[0] += f32::from(p[2]);
                        sum[1] += f32::from(p[1]);
                        sum[2] += f32::from(p[0]);
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as f32 * 255.;
                *out = [sum[0] / n, sum[1] / n, sum[2] / n];
            }
        }
        let mut world = Self {
            width: w,
            height: h,
            source: (width, height),
            pixels,
        };
        for _ in 0..3 {
            world.box_blur(radius);
        }
        world
    }
    fn box_blur(&mut self, radius: usize) {
        if radius == 0 {
            return;
        }
        let (w, h) = (self.width, self.height);
        let mut scratch = vec![[0f32; 3]; w.max(h)];
        // Rows, then columns; edges clamp so the frame never darkens.
        for pass in 0..2 {
            let (lines, length) = if pass == 0 { (h, w) } else { (w, h) };
            for line in 0..lines {
                let at = |i: usize| {
                    if pass == 0 {
                        line * w + i
                    } else {
                        i * w + line
                    }
                };
                let window = (2 * radius + 1) as f32;
                let mut sum = [0f32; 3];
                for i in 0..=2 * radius {
                    let p = self.pixels[at(i.saturating_sub(radius).min(length - 1))];
                    for c in 0..3 {
                        sum[c] += p[c];
                    }
                }
                for (i, out) in scratch.iter_mut().take(length).enumerate() {
                    *out = [sum[0] / window, sum[1] / window, sum[2] / window];
                    let leaving = self.pixels[at(i.saturating_sub(radius))];
                    let entering = self.pixels[at((i + radius + 1).min(length - 1))];
                    for c in 0..3 {
                        sum[c] += entering[c] - leaving[c];
                    }
                }
                for (i, value) in scratch.iter().take(length).enumerate() {
                    self.pixels[at(i)] = *value;
                }
            }
        }
    }
    /// Bilinear sample at reduced-image coordinates, clamped to the edges.
    fn sample(&self, x: f32, y: f32) -> [f32; 3] {
        let x = x.clamp(0., (self.width - 1) as f32);
        let y = y.clamp(0., (self.height - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let p = |x: usize, y: usize| self.pixels[y * self.width + x];
        let (a, b, c, d) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
        let mut out = [0.; 3];
        for i in 0..3 {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bottom = c[i] + (d[i] - c[i]) * fx;
            out[i] = top + (bottom - top) * fy;
        }
        out
    }
}

/// Everything that decides a slab bake. Equal keys produce identical pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Bake {
    /// Window size in logical pixels.
    pub viewport: (f32, f32),
    /// Slab rectangle in logical window pixels: x, y, width, height.
    pub slab: [f32; 4],
    /// Output pixels per logical pixel (the window scale factor).
    pub scale: f32,
    pub fit: Fit,
    /// `background.opacity`: the world image over `base`.
    pub opacity: f32,
    pub base: Rgba,
    pub world_tint: Rgba,
    pub glass_top: Rgba,
    pub glass_bottom: Rgba,
    /// Film grain amplitude in 8-bit steps (0 disables).
    pub grain: f32,
    /// Legibility clamp on relative luminance after tinting: dark glass caps
    /// brightness at `ceiling`, light glass lifts darkness to `floor`. Hue and
    /// saturation survive, so a vivid world still colors the glass. 0 disables.
    pub ceiling: f32,
    pub floor: f32,
}
impl Bake {
    /// Output size in device pixels, capped so a huge display stays bounded.
    pub fn size(&self) -> (usize, usize) {
        let scale = self.scale.clamp(0.5, 4.);
        let w = (self.slab[2] * scale).round().clamp(1., 4096.) as usize;
        let h = (self.slab[3] * scale).round().clamp(1., 4096.) as usize;
        (w, h)
    }
}
/// The world rectangle the stage paints for `fit`, in logical window pixels,
/// mirroring GPUI's `ObjectFit` for cover, contain and stretch.
pub fn fitted(fit: Fit, viewport: (f32, f32), image: (u32, u32)) -> [f32; 4] {
    let (vw, vh) = viewport;
    let (iw, ih) = (image.0.max(1) as f32, image.1.max(1) as f32);
    let scale = match fit {
        Fit::Cover => (vw / iw).max(vh / ih),
        Fit::Contain => (vw / iw).min(vh / ih),
        Fit::Stretch => return [0., 0., vw, vh],
    };
    let (w, h) = (iw * scale, ih * scale);
    [(vw - w) / 2., (vh - h) / 2., w, h]
}
fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}
fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * c[0].powf(2.2) + 0.7152 * c[1].powf(2.2) + 0.0722 * c[2].powf(2.2)
}
/// Soft-knee legibility clamp: values below 60% of the limit pass untouched,
/// brighter (or, for light glass, darker) values compress smoothly toward it.
fn clamp_luminance(color: [f32; 3], ceiling: f32, floor: f32) -> [f32; 3] {
    let l = luminance(color);
    if ceiling > 0. && l > ceiling * 0.6 {
        let knee = ceiling * 0.6;
        let target = knee + (ceiling - knee) * ((l - knee) / (ceiling - knee)).tanh();
        let k = (target / l).powf(1. / 2.2);
        return [color[0] * k, color[1] * k, color[2] * k];
    }
    if floor > 0. && l < floor {
        // Lift toward white until the floor is met (luminance is monotonic in t).
        let mut t = 0.;
        let mut step = 0.5;
        for _ in 0..10 {
            let candidate = mix(color, [1.; 3], t + step);
            if luminance(candidate) < floor {
                t += step;
            }
            step *= 0.5;
        }
        return mix(color, [1.; 3], t + step);
    }
    color
}
/// Stable per-pixel noise in -0.5..0.5 (a small integer hash, no state).
fn noise(x: u32, y: u32) -> f32 {
    let mut h = x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841);
    h ^= h >> 13;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 16;
    (h & 0xffff) as f32 / 65535. - 0.5
}
/// Bakes the slab texture: world (or `base` where the image does not reach),
/// world tint, glass gradient and grain. Returns opaque BGRA pixels of
/// [`Bake::size`].
pub fn bake(world: Option<&World>, params: &Bake) -> (usize, usize, Vec<u8>) {
    let (w, h) = params.size();
    let base = [params.base.0[0], params.base.0[1], params.base.0[2]];
    let tint = params.world_tint.0;
    let fitted = world.map(|world| fitted(params.fit, params.viewport, world.source));
    let mut out = vec![0u8; w * h * 4];
    let [sx, sy, sw, sh] = params.slab;
    for y in 0..h {
        let v = (y as f32 + 0.5) / h as f32;
        let wy = sy + v * sh;
        let top = params.glass_top.0;
        let bottom = params.glass_bottom.0;
        let glass = [
            top[0] + (bottom[0] - top[0]) * v,
            top[1] + (bottom[1] - top[1]) * v,
            top[2] + (bottom[2] - top[2]) * v,
            top[3] + (bottom[3] - top[3]) * v,
        ];
        let row = &mut out[y * w * 4..(y + 1) * w * 4];
        for x in 0..w {
            let wx = sx + (x as f32 + 0.5) / w as f32 * sw;
            let mut color = base;
            if let (Some(world), Some([fx, fy, fw, fh])) = (world, fitted)
                && wx >= fx
                && wy >= fy
                && wx < fx + fw
                && wy < fy + fh
            {
                let u = (wx - fx) / fw * world.width as f32 - 0.5;
                let t = (wy - fy) / fh * world.height as f32 - 0.5;
                color = mix(base, world.sample(u, t), params.opacity);
            }
            color = mix(color, [tint[0], tint[1], tint[2]], tint[3]);
            color = mix(color, [glass[0], glass[1], glass[2]], glass[3]);
            color = clamp_luminance(color, params.ceiling, params.floor);
            let grain = noise(x as u32, y as u32) * params.grain / 255.;
            let pixel = &mut row[x * 4..x * 4 + 4];
            pixel[0] = ((color[2] + grain).clamp(0., 1.) * 255.).round() as u8;
            pixel[1] = ((color[1] + grain).clamp(0., 1.) * 255.).round() as u8;
            pixel[2] = ((color[0] + grain).clamp(0., 1.) * 255.).round() as u8;
            pixel[3] = 255;
        }
    }
    (w, h, out)
}

/// One soft light in the procedural atmosphere: center and radius are fractions
/// of the image, `color` is added with Gaussian falloff.
pub struct Glow {
    pub center: (f32, f32),
    pub radius: f32,
    pub color: Rgba,
}
/// The default world when no wallpaper is configured: a night gradient lit by a
/// few broad, colored glows, grained so the gradients never band. BGRA.
pub fn atmosphere(width: usize, height: usize, top: Rgba, bottom: Rgba, glows: &[Glow]) -> Vec<u8> {
    let mut out = vec![0u8; width * height * 4];
    let aspect = width as f32 / height.max(1) as f32;
    for y in 0..height {
        let v = y as f32 / height.max(1) as f32;
        let ground = mix(
            [top.0[0], top.0[1], top.0[2]],
            [bottom.0[0], bottom.0[1], bottom.0[2]],
            v,
        );
        for x in 0..width {
            let u = x as f32 / width.max(1) as f32;
            let mut color = ground;
            for glow in glows {
                let dx = (u - glow.center.0) * aspect;
                let dy = v - glow.center.1;
                let falloff = (-(dx * dx + dy * dy) / (glow.radius * glow.radius)).exp();
                let weight = falloff * glow.color.0[3];
                for c in 0..3 {
                    // Screen blend: lights add without clipping to white.
                    color[c] = 1. - (1. - color[c]) * (1. - glow.color.0[c] * weight);
                }
            }
            let grain = noise(x as u32 ^ 0x5bd1, y as u32) * 3. / 255.;
            let pixel = &mut out[(y * width + x) * 4..(y * width + x) * 4 + 4];
            pixel[0] = ((color[2] + grain).clamp(0., 1.) * 255.).round() as u8;
            pixel[1] = ((color[1] + grain).clamp(0., 1.) * 255.).round() as u8;
            pixel[2] = ((color[0] + grain).clamp(0., 1.) * 255.).round() as u8;
            pixel[3] = 255;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn solid(w: u32, h: u32, bgra: [u8; 4]) -> Vec<u8> {
        bgra.repeat((w * h) as usize)
    }
    fn params(slab: [f32; 4]) -> Bake {
        Bake {
            viewport: (200., 100.),
            slab,
            scale: 1.,
            fit: Fit::Cover,
            opacity: 1.,
            base: Rgba([0., 0., 0., 1.]),
            world_tint: Rgba([0., 0., 0., 0.]),
            glass_top: Rgba([0., 0., 0., 0.]),
            glass_bottom: Rgba([0., 0., 0., 0.]),
            grain: 0.,
            ceiling: 0.,
            floor: 0.,
        }
    }
    #[test]
    fn world_reduction_preserves_flat_color_and_blur_spreads_light() {
        let world = World::new(&solid(64, 32, [40, 80, 200, 255]), 64, 32, 16, 2);
        assert_eq!((world.width, world.height), (16, 8));
        let p = world.sample(5.5, 3.2);
        assert!((p[0] - 200. / 255.).abs() < 1e-4 && (p[2] - 40. / 255.).abs() < 1e-4);
        // A single bright column spreads sideways under blur.
        let mut image = solid(32, 4, [0, 0, 0, 255]);
        for y in 0..4 {
            image[(y * 32 + 16) * 4..(y * 32 + 16) * 4 + 3].copy_from_slice(&[255, 255, 255]);
        }
        let world = World::new(&image, 32, 4, 32, 2);
        assert!(world.sample(14., 1.)[0] > 0.01 && world.sample(16., 1.)[0] < 0.9);
    }
    #[test]
    fn bakes_crop_the_world_where_the_stage_paints_it_and_apply_glass() {
        // Left half red, right half blue; a slab over the right half sees blue.
        let mut image = solid(200, 100, [0, 0, 255, 255]);
        for y in 0..100 {
            for x in 100..200 {
                image[(y * 200 + x) * 4..(y * 200 + x) * 4 + 3].copy_from_slice(&[255, 0, 0]);
            }
        }
        let world = World::new(&image, 200, 100, 200, 0);
        let (w, h, pixels) = bake(Some(&world), &params([120., 10., 60., 40.]));
        assert_eq!((w, h), (60, 40));
        assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
        // A fully opaque glass gradient replaces the world: top and bottom differ.
        let mut glass = params([0., 0., 200., 100.]);
        glass.glass_top = Rgba([1., 1., 1., 1.]);
        glass.glass_bottom = Rgba([0., 0., 0., 1.]);
        glass.scale = 0.5;
        let (w, h, pixels) = bake(Some(&world), &glass);
        assert_eq!((w, h), (100, 50));
        assert!(pixels[0] > 240 && pixels[(h - 1) * w * 4] < 15);
        // Without a world the base color shows; grain stays within its amplitude.
        let mut grained = params([0., 0., 10., 10.]);
        grained.base = Rgba([0.5, 0.5, 0.5, 1.]);
        grained.grain = 4.;
        let (_, _, pixels) = bake(None, &grained);
        assert!(
            pixels
                .chunks(4)
                .all(|p| p[0].abs_diff(128) <= 3 && p[3] == 255)
        );
        assert!(pixels.chunks(4).any(|p| p[0] != 128));
    }
    #[test]
    fn legibility_clamp_caps_bright_worlds_and_lifts_dark_ones_keeping_hue() {
        let emerald = [0.05, 0.85, 0.6];
        let capped = clamp_luminance(emerald, 0.05, 0.);
        assert!(luminance(capped) <= 0.0501 && luminance(capped) > 0.03);
        // Hue survives: green stays the dominant channel, in proportion.
        assert!(capped[1] > capped[2] && capped[2] > capped[0]);
        let dim = [0.1, 0.12, 0.15];
        assert_eq!(clamp_luminance(dim, 0.05, 0.), dim);
        let lifted = clamp_luminance([0.05, 0.1, 0.2], 0., 0.7);
        assert!((luminance(lifted) - 0.7).abs() < 0.01);
    }
    #[test]
    fn fitting_matches_cover_contain_and_stretch() {
        assert_eq!(
            fitted(Fit::Cover, (100., 100.), (200, 100)),
            [-50., 0., 200., 100.]
        );
        assert_eq!(
            fitted(Fit::Contain, (100., 100.), (200, 100)),
            [0., 25., 100., 50.]
        );
        assert_eq!(
            fitted(Fit::Stretch, (100., 100.), (200, 100)),
            [0., 0., 100., 100.]
        );
        let glows = [Glow {
            center: (0.5, 0.5),
            radius: 0.2,
            color: Rgba([1., 0., 0., 1.]),
        }];
        let sky = atmosphere(
            20,
            10,
            Rgba([0., 0., 0., 1.]),
            Rgba([0., 0., 0., 1.]),
            &glows,
        );
        let at = |x: usize, y: usize| sky[(y * 20 + x) * 4 + 2];
        assert!(at(10, 5) > 200 && at(0, 0) < 20);
    }
}

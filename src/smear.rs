// SPDX-License-Identifier: GPL-3.0-only
//! Rust adaptation of smear-cursor.nvim by Son Pham-Ba and contributors.
//! Physics, edge classification, fractional blocks, quadrant coverage and colors
//! derive from animation.lua/draw.lua/color.lua at f6cb57c. See THIRD_PARTY_NOTICES.md.
use crate::config::{CursorQuality, SmearConfig};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier},
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Point {
    pub x: f64,
    pub y: f64,
}
impl Point {
    fn distance(self, b: Self) -> f64 {
        (self.x - b.x).hypot(self.y - b.y)
    }
}
fn corners(p: Point) -> [Point; 4] {
    [
        p,
        Point {
            x: p.x + 1.0,
            y: p.y,
        },
        Point {
            x: p.x + 1.0,
            y: p.y + 1.0,
        },
        Point {
            x: p.x,
            y: p.y + 1.0,
        },
    ]
}
fn center(q: &[Point; 4]) -> Point {
    Point {
        x: q.iter().map(|p| p.x).sum::<f64>() / 4.0,
        y: q.iter().map(|p| p.y).sum::<f64>() / 4.0,
    }
}
#[derive(Clone)]
pub(crate) struct Spring {
    pub q: [Point; 4],
    target: [Point; 4],
    velocity: [Point; 4],
    stiffness: [f64; 4],
    head: usize,
    tail: usize,
    pub active: bool,
    remainder_ms: f64,
    render_q: [Point; 4],
}
impl Spring {
    pub fn new(p: Point) -> Self {
        Self {
            q: corners(p),
            target: corners(p),
            velocity: [Point::default(); 4],
            stiffness: [0.6; 4],
            head: 0,
            tail: 0,
            active: false,
            remainder_ms: 0.0,
            render_q: corners(p),
        }
    }
    pub fn retarget(&mut self, p: Point, s: SmearConfig) {
        if !self.active {
            self.remainder_ms = 0.0;
        }
        self.target = corners(p);
        let c = center(&self.target);
        let distances = self.q.map(|p| p.distance(c));
        let min = distances.into_iter().fold(f64::INFINITY, f64::min);
        let max = distances.into_iter().fold(0.0, f64::max);
        for (i, d) in distances.into_iter().enumerate() {
            let x = if max == min {
                0.0
            } else {
                (d - min) / (max - min)
            };
            self.stiffness[i] =
                s.stiffness + (s.trailing_stiffness - s.stiffness) * x.powf(s.trailing_exponent);
            if !self.active {
                self.velocity[i] = Point {
                    x: (self.q[i].x - self.target[i].x) * s.anticipation,
                    y: (self.q[i].y - self.target[i].y) * s.anticipation,
                };
            }
        }
        self.active = true;
    }
    /// Shift in buffer space when the viewport scrolls, preserving the spring.
    pub fn shift(&mut self, x: f64, y: f64) {
        for q in [&mut self.q, &mut self.target, &mut self.render_q] {
            for p in q {
                p.x += x;
                p.y += y;
            }
        }
    }
    pub fn step(&mut self, dt: f64, s: SmearConfig) {
        if !self.active {
            return;
        }
        // Preserve the reference's exact 17ms dynamics at every display rate.
        // Tiny partial integrations used to apply velocity too often and made
        // motion depend on the number of input/worker-driven redraws.
        self.remainder_ms += dt.clamp(0.0, 250.0);
        while self.remainder_ms >= 17.0 && self.active {
            self.integrate(17.0, s);
            self.remainder_ms -= 17.0;
        }
        if !self.active {
            self.remainder_ms = 0.0;
        }
        self.render_q = self.q;
        if self.active && self.remainder_ms > 0.0 {
            // Fractional sampling between consecutive *spring states*, never
            // integer cells or a linear cursor path. No extra integration of
            // the committed physics and no allocation are needed.
            let mut next = self.clone();
            next.integrate(17.0, s);
            let t = self.remainder_ms / 17.0;
            for (i, p) in self.render_q.iter_mut().enumerate() {
                p.x += (next.q[i].x - p.x) * t;
                p.y += (next.q[i].y - p.y) * t;
            }
        }
    }
    fn integrate(&mut self, dt: f64, s: SmearConfig) {
        let speed = dt / 17.0;
        let conservation = (1.0 - s.damping).powf(speed);
        let correction = 1.0 / (1.0 + 2.5 * conservation);
        let mut nearest = f64::INFINITY;
        let mut farthest = 0.0;
        for i in 0..4 {
            let d = self.q[i].distance(self.target[i]);
            if d < nearest {
                nearest = d;
                self.head = i
            }
            if d > farthest {
                farthest = d;
                self.tail = i
            }
            let k = 1.0 - (1.0 - self.stiffness[i] * correction).powf(speed);
            self.velocity[i].x += (self.target[i].x - self.q[i].x) * k;
            self.velocity[i].y += (self.target[i].y - self.q[i].y) * k;
            self.q[i].x += self.velocity[i].x;
            self.q[i].y += self.velocity[i].y;
            self.velocity[i].x *= conservation;
            self.velocity[i].y *= conservation;
        }
        let head = self.q[self.head];
        let length = self.q.iter().map(|p| p.distance(head)).fold(0.0, f64::max);
        if length > s.max_length {
            for p in &mut self.q {
                p.x = head.x + (p.x - head.x) * s.max_length / length;
                p.y = head.y + (p.y - head.y) * s.max_length / length;
            }
        }
        if (0..4).all(|i| {
            self.q[i].distance(self.target[i]) <= 0.1
                && self.velocity[i].distance(Point::default()) <= 0.1
        }) {
            self.q = self.target;
            self.velocity = [Point::default(); 4];
            self.active = false;
        }
    }
    fn drawn(&self) -> [Point; 4] {
        let mut q = self.render_q;
        let c = center(&q);
        let t = center(&self.target);
        if (t.x - c.x).abs() < 0.125 || (t.y - c.y).abs() < 0.125 {
            return q;
        }
        // Reference volume correction (two equal triangle cross products).
        let volume = (q[1].x - q[0].x) * (q[2].y - q[0].y) - (q[1].y - q[0].y) * (q[2].x - q[0].x);
        if volume <= 0.0 {
            return q;
        }
        let factor = (1.0 / volume).powf(0.3 / 2.0).max(0.7);
        for (i, p) in q.iter_mut().enumerate() {
            let dx = self.target[i].x - p.x;
            let dy = self.target[i].y - p.y;
            let len = dx.hypot(dy);
            if len > 0.0 {
                let nx = -dy / len;
                let ny = dx / len;
                let shift = ((p.x - c.x) * nx + (p.y - c.y) * ny) * (1.0 - factor);
                p.x -= nx * shift;
                p.y -= ny * shift;
            }
        }
        q
    }
    pub fn draw(
        &self,
        buffer: &mut Buffer,
        clip: Rect,
        s: SmearConfig,
        color: Color,
        intensity: f64,
        alternate: bool,
    ) {
        let clip = clip.intersection(buffer.area);
        let mut q = self.drawn();
        let origin = q[self.head];
        let tail = q[self.tail];
        if cross(q[0], q[1], q[3]) < 0.0 {
            q.swap(0, 2)
        }
        let edges = std::array::from_fn::<_, 4, _>(|i| Edge::new(q[i], q[(i + 1) % 4]));
        let x0 = q
            .iter()
            .map(|p| p.x)
            .fold(f64::INFINITY, f64::min)
            .floor()
            .max(f64::from(clip.x)) as u16;
        let y0 = q
            .iter()
            .map(|p| p.y)
            .fold(f64::INFINITY, f64::min)
            .floor()
            .max(f64::from(clip.y)) as u16;
        let x1 = q
            .iter()
            .map(|p| p.x)
            .fold(0.0, f64::max)
            .ceil()
            .min(f64::from(clip.right())) as u16;
        let y1 = q
            .iter()
            .map(|p| p.y)
            .fold(0.0, f64::max)
            .ceil()
            .min(f64::from(clip.bottom())) as u16;
        let length2 = origin.distance(tail).powi(2);
        for y in y0..y1 {
            for x in x0..x1 {
                let projection = if length2 > 1.0 {
                    ((f64::from(x) + 0.5 - origin.x) * (tail.x - origin.x)
                        + (f64::from(y) + 0.5 - origin.y) * (tail.y - origin.y))
                        / length2
                } else {
                    0.0
                };
                let shade = (1.0 - projection.clamp(0.0, 1.0))
                    .powf(s.gradient_exponent / intensity.max(0.1));
                if let Some(g) = raster(
                    &edges,
                    f64::from(x),
                    f64::from(y),
                    shade,
                    s.quality,
                    alternate,
                ) {
                    paint(buffer, x, y, g, color, clip);
                }
            }
        }
    }
}
fn cross(a: Point, b: Point, c: Point) -> f64 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Top,
    Bottom,
    Left,
    Right,
    LeftDiagonal,
    RightDiagonal,
    None,
}
struct Edge {
    p: Point,
    slope: f64,
    kind: Kind,
}
impl Edge {
    fn new(p: Point, b: Point) -> Self {
        let dx = b.x - p.x;
        let dy = b.y - p.y;
        let slope = dy / dx;
        let kind = if slope.is_nan() {
            Kind::None
        } else if slope.abs() <= 1.0 / 4.5 {
            if dx > 0.0 { Kind::Top } else { Kind::Bottom }
        } else if slope.abs() >= 3.0 {
            if dy > 0.0 { Kind::Right } else { Kind::Left }
        } else if dy > 0.0 {
            Kind::RightDiagonal
        } else {
            Kind::LeftDiagonal
        };
        Self { p, slope, kind }
    }
    fn horizontal(&self, x: f64) -> f64 {
        self.p.y + (x - self.p.x) * self.slope
    }
    fn vertical(&self, y: f64) -> f64 {
        self.p.x + (y - self.p.y) / self.slope
    }
    fn intersection(&self, x: f64, y: f64, low: bool) -> f64 {
        match self.kind {
            Kind::Top => self.horizontal(x + 0.5) - y,
            Kind::Bottom => y + 1.0 - self.horizontal(x + 0.5),
            Kind::Left => self.vertical(y + 0.5) - x,
            Kind::Right => x + 1.0 - self.vertical(y + 0.5),
            Kind::LeftDiagonal => {
                self.vertical(y + 0.5) + (if low { -0.5 } else { 0.5 }) / self.slope.abs() - x
            }
            Kind::RightDiagonal => {
                x + 1.0 - self.vertical(y + 0.5) - (if low { 0.5 } else { -0.5 }) / self.slope.abs()
            }
            Kind::None => 0.0,
        }
    }
}
struct Glyph {
    symbol: &'static str,
    shade: f64,
    inverted: bool,
}
const BOTTOM: [&str; 9] = ["█", "▇", "▆", "▅", "▄", "▃", "▂", "▁", " "];
const LEFT: [&str; 9] = [" ", "▏", "▎", "▍", "▌", "▋", "▊", "▉", "█"];
const TOP: [&str; 9] = [" ", "▔", "🮂", "🮃", "▀", "🮄", "🮅", "🮆", "█"];
const RIGHT: [&str; 9] = ["█", "🮋", "🮊", "🮉", "▐", "🮈", "🮇", "▕", " "];
const MATRIX: [&str; 16] = [
    " ", "▘", "▝", "▀", "▖", "▌", "▞", "▛", "▗", "▚", "▐", "▜", "▄", "▙", "▟", "█",
];
fn shifted(
    a: f64,
    b: f64,
    shade: f64,
    vertical: bool,
    legacy: bool,
    alternate: bool,
) -> Option<Glyph> {
    if a >= b {
        return None;
    }
    let center = (a + b) / 2.0;
    let thickness = b - a;
    let (near, micro) = if (a.max(1.0 - b)) / 2.0 < a.min(1.0 - b) {
        if alternate {
            (true, b * 8.0)
        } else {
            (false, a * 8.0)
        }
    } else if center < 0.5 {
        (true, center * 16.0)
    } else {
        (false, center * 16.0 - 8.0)
    };
    let index = if near { micro.ceil() } else { micro.floor() }.clamp(0.0, 8.0) as usize;
    let coverage = if near {
        index as f64 / 8.0
    } else {
        1.0 - index as f64 / 8.0
    };
    if coverage <= 0.0 {
        return None;
    }
    let (symbol, inverted) = match (vertical, near, legacy) {
        (true, true, true) => (TOP[index], false),
        (true, true, false) => (BOTTOM[index], true),
        (true, false, _) => (BOTTOM[index], false),
        (false, true, _) => (LEFT[index], false),
        (false, false, true) => (RIGHT[index], false),
        (false, false, false) => (LEFT[index], true),
    };
    Some(Glyph {
        symbol,
        shade: shade * thickness / coverage,
        inverted,
    })
}
fn raster(
    edges: &[Edge; 4],
    x: f64,
    y: f64,
    shade: f64,
    quality: CursorQuality,
    alternate: bool,
) -> Option<Glyph> {
    let mut cuts = [0.0_f64; 5];
    let mut diagonal = None;
    let mut single = true;
    for (i, e) in edges.iter().enumerate() {
        let d = e.intersection(x, y, false);
        let k = match e.kind {
            Kind::Top => 0,
            Kind::Bottom => 1,
            Kind::Left => 2,
            Kind::Right => 3,
            Kind::LeftDiagonal | Kind::RightDiagonal => 4,
            Kind::None => continue,
        };
        if k == 4 {
            if e.intersection(x, y, true) >= 1.0 {
                return None;
            }
            if d > 0.2 && cuts[4] > 0.0 {
                single = false
            }
        } else {
            if d >= 1.0 {
                return None;
            }
            if d > 0.2 {
                single = false
            }
        }
        if d > cuts[k] {
            cuts[k] = d;
            if k == 4 {
                diagonal = Some(i)
            }
        }
    }
    if quality == CursorQuality::Ascii {
        let coverage = matrix(edges, x, y);
        let coverage = coverage.iter().sum::<f64>() / 4.0;
        return (coverage > 0.05).then_some(Glyph {
            symbol: "#",
            shade: shade * coverage,
            inverted: false,
        });
    }
    if cuts[4] < 0.25 {
        let vs = 1.0 - cuts[0] - cuts[1];
        let hs = 1.0 - cuts[2] - cuts[3];
        let mut vertical = cuts[0] > 0.0 || cuts[1] > 0.0;
        let mut horizontal = cuts[2] > 0.0 || cuts[3] > 0.0;
        if vertical && horizontal {
            if vs < 0.75 && hs < 0.75 {
                vertical = false;
                horizontal = false
            } else if 2.0 * (1.0 - vs) > 1.0 - hs {
                horizontal = false
            } else {
                vertical = false
            }
        }
        if vertical {
            return shifted(
                cuts[0],
                1.0 - cuts[1],
                shade * hs,
                true,
                quality == CursorQuality::Legacy,
                alternate,
            );
        }
        if horizontal {
            return shifted(
                cuts[2],
                1.0 - cuts[3],
                shade * vs,
                false,
                quality == CursorQuality::Legacy,
                alternate,
            );
        }
    }
    if single
        && quality == CursorQuality::Legacy
        && let Some(i) = diagonal
        && let Some(symbol) = diagonal_glyph(&edges[i], x, y)
    {
        return Some(Glyph {
            symbol,
            shade,
            inverted: false,
        });
    }
    let m = matrix(edges, x, y);
    let max = m.into_iter().fold(0.0, f64::max);
    if max < 0.7 {
        return None;
    }
    let mut bits = 0;
    let mut count = 0;
    for (i, v) in m.into_iter().enumerate() {
        if v > max * 0.5 {
            bits |= 1 << i;
            count += 1
        }
    }
    if count == 0 {
        return None;
    }
    Some(Glyph {
        symbol: MATRIX[bits],
        shade: shade * m.iter().sum::<f64>() / f64::from(count),
        inverted: false,
    })
}
fn matrix(edges: &[Edge; 4], x: f64, y: f64) -> [f64; 4] {
    let mut m = [1.0; 4];
    for e in edges {
        for f in 0..2 {
            let offset = 0.25 + f as f64 * 0.5;
            let (v, vertical, reverse) = match e.kind {
                Kind::Top => (2.0 * (e.horizontal(x + offset) - y), true, false),
                Kind::Bottom => (2.0 * (e.horizontal(x + offset) - y), true, true),
                Kind::Left | Kind::LeftDiagonal => {
                    (2.0 * (e.vertical(y + offset) - x), false, false)
                }
                Kind::Right | Kind::RightDiagonal => {
                    (2.0 * (e.vertical(y + offset) - x), false, true)
                }
                Kind::None => continue,
            };
            for j in 0..2 {
                let coverage = if reverse {
                    v - j as f64
                } else {
                    j as f64 + 1.0 - v
                };
                let index = if vertical { j * 2 + f } else { f * 2 + j };
                m[index] *= coverage.clamp(0.0, 1.0);
            }
        }
    }
    m
}
// Slope/offset symbol tables from draw.lua. Opt-in: many fonts lack U+1FBxx.
fn diagonal_glyph(e: &Edge, x: f64, y: f64) -> Option<&'static str> {
    const TABLE: &[(f64, f64, &str, &str)] = &[
        (-2.0, -0.25, "🭛", "🭅"),
        (-2.0, 0.25, "🭟", "🭋"),
        (-4.0 / 3.0, -0.375, "🭙", "🭃"),
        (-4.0 / 3.0, 0.375, "🭟", "🭉"),
        (-1.0, 0.0, "◤", "◢"),
        (-2.0 / 3.0, -0.75, "🭗", "🭁"),
        (-2.0 / 3.0, -0.25, "🭚", "🭄"),
        (-2.0 / 3.0, 0.25, "🭠", "🭊"),
        (-2.0 / 3.0, 0.75, "🭝", "🭇"),
        (-1.0 / 3.0, -1.0, "🭘", "🭂"),
        (-1.0 / 3.0, 0.0, "🭜", "🭆"),
        (-1.0 / 3.0, 1.0, "🭞", "🭈"),
        (1.0 / 3.0, -1.0, "🬽", "🭓"),
        (1.0 / 3.0, 0.0, "🭑", "🭧"),
        (1.0 / 3.0, 1.0, "🭍", "🭣"),
        (2.0 / 3.0, -0.75, "🬼", "🭒"),
        (2.0 / 3.0, -0.25, "🬿", "🭕"),
        (2.0 / 3.0, 0.25, "🭏", "🭥"),
        (2.0 / 3.0, 0.75, "🭌", "🭢"),
        (1.0, 0.0, "◣", "◥"),
        (4.0 / 3.0, -0.375, "🬾", "🭔"),
        (4.0 / 3.0, 0.375, "🭎", "🭤"),
        (2.0, -0.25, "🭀", "🭖"),
        (2.0, 0.25, "🭐", "🭦"),
    ];
    let mut best = None;
    let mut angle = f64::INFINITY;
    for &(s, _, _, _) in TABLE {
        let a = ((2.0 * s).atan() - (2.0 * e.slope).atan()).abs();
        if a < angle {
            angle = a;
            best = Some(s)
        }
    }
    if angle > std::f64::consts::PI / 16.0 {
        return None;
    }
    let mut offset = 0.2;
    let mut result = None;
    for &(s, shift, left, right) in TABLE {
        if Some(s) == best {
            let d = (e.vertical(y + 0.5) - x - 0.5 - shift).abs();
            if d <= offset {
                offset = d;
                result = Some(if e.kind == Kind::LeftDiagonal {
                    right
                } else {
                    left
                })
            }
        }
    }
    result
}
fn paint(buffer: &mut Buffer, x: u16, y: u16, g: Glyph, color: Color, clip: Rect) {
    let level = (g.shade.clamp(0.0, 1.0) * 16.0).round();
    if level < 1.0 {
        return;
    }
    // Clearing both halves prevents a wide glyph suppressing a following overlay
    // cell in Ratatui's diff. The complete underlay is rebuilt on the next frame.
    use unicode_width::UnicodeWidthStr;
    if x > clip.x && buffer[(x - 1, y)].symbol().width() > 1 {
        buffer[(x - 1, y)].set_symbol(" ");
    }
    if buffer[(x, y)].symbol().width() > 1 && x + 1 < clip.right() {
        buffer[(x + 1, y)].set_symbol(" ");
    }
    let cell = &mut buffer[(x, y)];
    let bg = cell.bg;
    let ink = blend(bg, color, (level / 16.0).powf(1.0 / 2.2));
    cell.set_symbol(g.symbol)
        .set_fg(if g.inverted { bg } else { ink })
        .set_bg(if g.inverted { ink } else { bg });
    cell.modifier = Modifier::empty();
}
fn blend(a: Color, b: Color, t: f64) -> Color {
    let Some((ar, ag, ab)) = rgb(a) else { return b };
    let Some((br, bg, bb)) = rgb(b) else { return b };
    let mix = |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * t).round() as u8;
    let result = (mix(ar, br), mix(ag, bg), mix(ab, bb));
    match a {
        Color::Rgb(..) => Color::Rgb(result.0, result.1, result.2),
        Color::Indexed(_) => Color::Indexed(
            (0..=255)
                .min_by_key(|i| distance(rgb(Color::Indexed(*i)).unwrap(), result))
                .unwrap(),
        ),
        _ => ANSI
            .into_iter()
            .min_by_key(|c| distance(rgb(*c).unwrap(), result))
            .unwrap(),
    }
}
const ANSI: [Color; 16] = [
    Color::Black,
    Color::Red,
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::Magenta,
    Color::Cyan,
    Color::Gray,
    Color::DarkGray,
    Color::LightRed,
    Color::LightGreen,
    Color::LightYellow,
    Color::LightBlue,
    Color::LightMagenta,
    Color::LightCyan,
    Color::White,
];
fn distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> i32 {
    (i32::from(a.0) - i32::from(b.0)).pow(2)
        + (i32::from(a.1) - i32::from(b.1)).pow(2)
        + (i32::from(a.2) - i32::from(b.2)).pow(2)
}
pub(crate) fn rgb(c: Color) -> Option<(u8, u8, u8)> {
    let i = match c {
        Color::Rgb(r, g, b) => return Some((r, g, b)),
        Color::Indexed(i) => i,
        _ => ANSI.iter().position(|a| *a == c)? as u8,
    };
    if i >= 232 {
        let v = 8 + 10 * (i - 232);
        Some((v, v, v))
    } else if i >= 16 {
        let n = i - 16;
        let component = |c: u8| if c == 0 { 0 } else { 55 + 40 * c };
        Some((component(n / 36), component(n / 6 % 6), component(n % 6)))
    } else {
        Some(
            [
                (0, 0, 0),
                (128, 0, 0),
                (0, 128, 0),
                (128, 128, 0),
                (0, 0, 128),
                (128, 0, 128),
                (0, 128, 128),
                (192, 192, 192),
                (128, 128, 128),
                (255, 0, 0),
                (0, 255, 0),
                (255, 255, 0),
                (0, 0, 255),
                (255, 0, 255),
                (0, 255, 255),
                (255, 255, 255),
            ][usize::from(i)],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_rate_and_input_redraw_frequency_do_not_change_spring_trajectory() {
        let s = SmearConfig::default();
        let mut baseline = Spring::new(Point::default());
        baseline.retarget(Point { x: 70.0, y: 20.0 }, s);
        baseline.step(17.0, s);
        baseline.step(102.0, s);
        for frames in [3, 6, 12, 24, 48, 102] {
            let mut spring = Spring::new(Point::default());
            spring.retarget(Point { x: 70.0, y: 20.0 }, s);
            spring.step(17.0, s);
            for _ in 0..frames {
                spring.step(102.0 / f64::from(frames), s);
            }
            for i in 0..4 {
                assert!(spring.q[i].distance(baseline.q[i]) < 1e-8);
                assert!(spring.render_q[i].distance(baseline.render_q[i]) < 1e-8);
            }
        }
    }
    #[test]
    fn subframe_sampling_changes_fractional_geometry_without_extra_physics_ticks() {
        let s = SmearConfig::default();
        let mut spring = Spring::new(Point::default());
        spring.retarget(Point { x: 30.0, y: 10.0 }, s);
        spring.step(17.0, s);
        let committed = spring.q;
        let drawn = spring.render_q;
        spring.step(4.0, s);
        assert_eq!(spring.q, committed);
        assert_ne!(spring.render_q, drawn);
        let mut expected = spring.clone();
        expected.remainder_ms = 0.0;
        expected.step(17.0, s);
        spring.step(13.0, s);
        assert_eq!(spring.q, expected.q);
    }
    #[test]
    fn reference_first_step_and_elastic_corners() {
        let s = SmearConfig::default();
        let mut spring = Spring::new(Point { x: 0.0, y: 0.0 });
        spring.retarget(Point { x: 10.0, y: 0.0 }, s);
        spring.step(17.0, s);
        // Farthest corners use .45, nearest .6; anticipation initially -2.
        assert!((spring.q[0].x - (-2.0 + 10.0 * 0.45 / 1.375)).abs() < 1e-10);
        assert!((spring.q[1].x - (1.0 - 2.0 + 10.0 * 0.6 / 1.375)).abs() < 1e-10);
        assert!(spring.q[1].x - spring.q[0].x > 1.5);
    }
    #[test]
    fn converges_in_all_directions_and_after_rapid_retargeting() {
        let s = SmearConfig::default();
        for end in [
            Point { x: 1.0, y: 0.0 },
            Point { x: 0.0, y: 1.0 },
            Point { x: 70.0, y: 20.0 },
            Point { x: -30.0, y: -12.0 },
        ] {
            let mut spring = Spring::new(Point::default());
            spring.retarget(end, s);
            for _ in 0..120 {
                spring.step(17.0, s);
            }
            assert!(!spring.active);
            assert_eq!(spring.q, corners(end));
        }
        let mut spring = Spring::new(Point::default());
        for i in 0..100 {
            spring.retarget(
                Point {
                    x: f64::from(i % 20),
                    y: f64::from(i % 9),
                },
                s,
            );
            spring.step(8.0, s);
            assert!(spring.q.iter().all(|p| p.x.is_finite() && p.y.is_finite()));
        }
        for _ in 0..120 {
            spring.step(17.0, s);
        }
        assert!(!spring.active);
    }
    #[test]
    fn fractional_blocks_quadrants_and_diagonals() {
        let glyph = shifted(0.0, 0.375, 1.0, true, false, false).unwrap();
        assert_eq!(glyph.symbol, "▅");
        assert!(glyph.inverted);
        let edges = std::array::from_fn(|i| {
            let q = corners(Point { x: 0.5, y: 0.0 });
            Edge::new(q[i], q[(i + 1) % 4])
        });
        let glyph = raster(&edges, 0.0, 0.0, 1.0, CursorQuality::Blocks, false).unwrap();
        assert!(glyph.inverted);
        assert_eq!(glyph.symbol, "▌");
        let e = Edge::new(Point { x: 0.0, y: 1.0 }, Point { x: 1.0, y: 0.0 });
        assert_eq!(diagonal_glyph(&e, 0.0, 0.0), Some("◢"));
        assert_eq!(MATRIX[6], "▞");
    }
    #[test]
    fn geometry_is_clipped_and_all_quality_modes_draw() {
        let clip = Rect::new(4, 3, 20, 8);
        let area = Rect::new(0, 0, 30, 15);
        for quality in [
            CursorQuality::Blocks,
            CursorQuality::Legacy,
            CursorQuality::Ascii,
        ] {
            let s = SmearConfig {
                quality,
                ..SmearConfig::default()
            };
            let mut spring = Spring::new(Point { x: -10.0, y: -10.0 });
            spring.retarget(Point { x: 30.0, y: 14.0 }, s);
            let mut painted = false;
            for _ in 0..10 {
                spring.step(17.0, s);
                let mut b = Buffer::empty(area);
                spring.draw(&mut b, clip, s, Color::White, 1.0, false);
                for y in 0..15 {
                    for x in 0..30 {
                        if b[(x, y)].symbol() != " " {
                            assert!(clip.contains((x, y).into()));
                            painted = true;
                        }
                    }
                }
            }
            assert!(painted);
        }
    }
    #[test]
    fn colors_use_reference_gamma_and_16_levels() {
        let mut b = Buffer::empty(Rect::new(0, 0, 1, 1));
        b[(0, 0)].set_bg(Color::Rgb(0, 0, 0));
        paint(
            &mut b,
            0,
            0,
            Glyph {
                symbol: "█",
                shade: 0.5,
                inverted: false,
            },
            Color::Rgb(255, 255, 255),
            Rect::new(0, 0, 1, 1),
        );
        assert_eq!(b[(0, 0)].fg, Color::Rgb(186, 186, 186));
        assert!(matches!(
            blend(Color::Indexed(236), Color::Rgb(196, 167, 231), 0.5),
            Color::Indexed(_)
        ));
        assert!(!matches!(
            blend(Color::Black, Color::Rgb(196, 167, 231), 0.5),
            Color::Rgb(..) | Color::Indexed(_)
        ));
    }
}

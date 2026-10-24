//! Kitty sprite placements, kept outside both Ratatui cells and particle physics.
//! One RGBA atlas is uploaded once; animation sends only small placement updates.
use super::{MAX_PARTICLES, Particle};
use crate::config::{CursorEffect, Effects};
use base64::{Engine, engine::general_purpose::STANDARD};
use crossterm::event::{Event, KeyCode, KeyModifiers};
use std::{
    fmt::Write as _,
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

static OWNED_IMAGE: AtomicU32 = AtomicU32::new(0);
pub fn cleanup_owned(writer: &mut impl io::Write) -> io::Result<()> {
    let id = OWNED_IMAGE.load(Ordering::Acquire);
    if id != 0 {
        write!(writer, "\x1b_Ga=d,d=I,i={id},q=2\x1b\\")?;
        writer.flush()?;
        OWNED_IMAGE
            .compare_exchange(id, 0, Ordering::AcqRel, Ordering::Relaxed)
            .ok();
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellSize {
    pub width: u16,
    pub height: u16,
}
impl CellSize {
    pub fn new(width: u16, height: u16) -> Option<Self> {
        (2..=64).contains(&width).then_some(()).and_then(|_| {
            (4..=128)
                .contains(&height)
                .then_some(Self { width, height })
        })
    }
    pub fn detect() -> Option<Self> {
        let s = crossterm::terminal::window_size().ok()?;
        if s.columns == 0 || s.rows == 0 || s.width % s.columns != 0 || s.height % s.rows != 0 {
            return None;
        }
        Self::new(s.width / s.columns, s.height / s.rows)
    }
}
/// Environment only gates a probe. It can never enable rendering by itself.
pub fn candidate(get: impl Fn(&str) -> Option<String>, unix: bool) -> bool {
    unix && ![
        "SSH_CONNECTION",
        "SSH_TTY",
        "TMUX",
        "STY",
        "ZELLIJ",
        "ZELLIJ_SESSION_NAME",
        "WEZTERM_PANE",
        "WEZTERM_EXECUTABLE",
    ]
    .iter()
    .any(|k| get(k).is_some())
        && (get("KITTY_WINDOW_ID").is_some()
            || get("TERM_PROGRAM").as_deref() == Some("kitty")
            || get("TERM").as_deref() == Some("xterm-kitty"))
}
fn kitty_version(s: &str) -> bool {
    let Some(version) = s.strip_prefix(">|kitty(").and_then(|s| s.strip_suffix(')')) else {
        return false;
    };
    let mut parts = version.split('.');
    let major = parts.next().and_then(|s| s.parse::<u32>().ok());
    let minor = parts.next().and_then(|s| s.parse::<u32>().ok());
    matches!((major,minor),(Some(a),Some(b)) if a>0 || b>=20)
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Disabled,
    Start,
    Query,
    Upload,
    Placement,
    Ready,
    Failed,
}
pub struct Graphics {
    phase: Phase,
    id: u32,
    cell: Option<CellSize>,
    color: [u8; 3],
    allowed: bool,
    native: bool,
    filter: Arc<AtomicBool>,
    version_filter: Arc<AtomicBool>,
    deadline: Option<Instant>,
    version: bool,
    query: bool,
    upload: String,
    upload_at: usize,
    tile: u16,
    placed: usize,
    clear: bool,
    free: bool,
    allocated: bool,
    commands: String,
    next_present: Option<Instant>,
    slow: u8,
}
impl Graphics {
    pub fn disabled() -> Self {
        Self::new(false, None, [196, 167, 231], false)
    }
    pub fn native(effects: Effects, color: [u8; 3]) -> Self {
        let allowed = candidate(|k| std::env::var(k).ok(), cfg!(unix));
        let mut result = Self::new(
            allowed,
            if allowed { CellSize::detect() } else { None },
            color,
            true,
        );
        result.configure(effects, color);
        result
    }
    fn new(allowed: bool, cell: Option<CellSize>, color: [u8; 3], native: bool) -> Self {
        Self {
            phase: Phase::Disabled,
            id: (std::process::id().wrapping_mul(2654435761) & 0x3fff_fffe) | 0x4000_0000,
            cell,
            color,
            allowed,
            native,
            filter: Arc::new(AtomicBool::new(false)),
            version_filter: Arc::new(AtomicBool::new(false)),
            deadline: None,
            version: false,
            query: false,
            upload: String::new(),
            upload_at: 0,
            tile: 0,
            placed: 0,
            clear: false,
            free: false,
            allocated: false,
            commands: String::new(),
            next_present: None,
            slow: 0,
        }
    }
    pub fn filter(&self) -> (Arc<AtomicBool>, Arc<AtomicBool>, u32) {
        (self.filter.clone(), self.version_filter.clone(), self.id)
    }
    pub fn native_candidate(&self) -> bool {
        self.native && self.allowed
    }
    pub fn configure(&mut self, effects: Effects, color: [u8; 3]) {
        self.free |= self.allocated;
        self.clear = true;
        self.color = color;
        self.next_present = None;
        self.slow = 0;
        self.phase = if self.allowed
            && self.cell.is_some()
            && effects.cursor
            && matches!(
                effects.cursor_effect,
                CursorEffect::Auto | CursorEffect::Pixiedust
            )
            && effects.pixiedust.density > 0.0
        {
            Phase::Start
        } else {
            Phase::Disabled
        };
        self.deadline = None;
    }
    pub fn resize(&mut self, cell: Option<CellSize>, effects: Effects, color: [u8; 3]) {
        self.cell = cell.and_then(|c| CellSize::new(c.width, c.height));
        self.configure(effects, color);
    }
    pub fn ready(&self) -> Option<CellSize> {
        (self.phase == Phase::Ready).then_some(self.cell).flatten()
    }
    pub fn fallback(&self) -> bool {
        matches!(self.phase, Phase::Disabled | Phase::Failed)
    }
    pub fn waiting(&self, now: Instant) -> Option<Duration> {
        match self.phase {
            Phase::Start | Phase::Upload => Some(Duration::from_millis(16)),
            Phase::Query | Phase::Placement => {
                self.deadline.map(|d| d.saturating_duration_since(now))
            }
            _ => None,
        }
    }
    fn fail(&mut self) {
        self.phase = Phase::Failed;
        self.deadline = None;
        self.free |= self.allocated;
        self.clear = true;
    }
    pub fn reply(&mut self, reply: &str, now: Instant) {
        if reply.starts_with(">|") {
            if self.phase == Phase::Query {
                if kitty_version(reply) {
                    self.version = true;
                } else {
                    self.fail();
                }
            }
            return;
        }
        let Some((fields, status)) = reply.strip_prefix('G').and_then(|s| s.split_once(';')) else {
            return;
        };
        let field = |key: &str| {
            fields
                .split(',')
                .find_map(|f| f.strip_prefix(key))
                .and_then(|s| s.parse::<u32>().ok())
        };
        let Some(id) = field("i=") else {
            return;
        };
        if id != self.id && id != self.id + 1 {
            return;
        }
        if status != "OK" {
            self.fail();
            return;
        }
        match self.phase {
            Phase::Query if id == self.id + 1 => self.query = true,
            Phase::Upload if id == self.id && self.upload_at == self.upload.len() => {
                self.phase = Phase::Placement;
                self.deadline = Some(now + Duration::from_millis(500));
                self.clear = true;
            }
            Phase::Placement if id == self.id && field("p=") == Some(u32::MAX) => {
                self.phase = Phase::Ready;
                self.deadline = None;
                self.clear = true;
                self.filter.store(false, Ordering::Release);
                self.version_filter.store(false, Ordering::Release);
            }
            _ => {}
        }
    }
    fn atlas(&mut self) {
        let cell = self.cell.expect("atlas only follows a validated cell size");
        self.tile = cell.width.clamp(4, 12);
        let tile = usize::from(self.tile);
        let width = tile * 48;
        let mut rgba = vec![0u8; width * tile * 4];
        for size in 0..3 {
            for level in 0..16 {
                for y in 0..tile {
                    for x in 0..tile {
                        let dx = (x as f32 + 0.5 - tile as f32 / 2.0).abs();
                        let dy = (y as f32 + 0.5 - tile as f32 / 2.0).abs();
                        let core = f32::from(cell.width) * (0.075 + size as f32 * 0.015);
                        let distance = dx.max(dy);
                        let alpha = if distance <= core {
                            1.0
                        } else {
                            0.22 * (-(distance - core).powi(2) / 1.5).exp()
                        };
                        let at = (y * width + (size * 16 + level) * tile + x) * 4;
                        rgba[at..at + 3].copy_from_slice(&self.color);
                        rgba[at + 3] = (alpha * level as f32 / 15.0 * 200.0).round() as u8;
                    }
                }
            }
        }
        STANDARD.encode_string(&rgba, &mut self.upload);
        self.upload_at = 0;
    }
    /// Bounded handshake work; never reads stdin or waits for a reply.
    pub fn service(&mut self, now: Instant) -> &[u8] {
        self.commands.clear();
        if self.deadline.is_some_and(|d| now >= d) {
            self.fail();
        }
        if self.free {
            let _ = write!(self.commands, "\x1b_Ga=d,d=I,i={},q=2\x1b\\", self.id);
            self.free = false;
            self.allocated = false;
            self.placed = 0;
        } else if self.clear && self.placed > 0 {
            let _ = write!(self.commands, "\x1b_Ga=d,d=i,i={},q=2\x1b\\", self.id);
            self.placed = 0;
        }
        self.clear = false;
        match self.phase {
            Phase::Start => {
                self.commands.reserve(16 * 1024);
                self.version = false;
                self.query = false;
                self.upload.clear();
                self.filter.store(true, Ordering::Release);
                self.version_filter.store(true, Ordering::Release);
                let _ = write!(
                    self.commands,
                    "\x1b[>q\x1b_Ga=q,t=d,f=32,s=1,v=1,i={};AAAAAA==\x1b\\",
                    self.id + 1
                );
                self.phase = Phase::Query;
                self.deadline = Some(now + Duration::from_millis(350));
            }
            Phase::Query if self.version && self.query => {
                self.atlas();
                self.phase = Phase::Upload;
                self.deadline = None;
            }
            Phase::Upload if self.upload_at < self.upload.len() => {
                let end = (self.upload_at + 4096).min(self.upload.len());
                let more = u8::from(end < self.upload.len());
                if self.upload_at == 0 {
                    self.allocated = true;
                    if self.native {
                        OWNED_IMAGE.store(self.id, Ordering::Release);
                    }
                    let _ = write!(
                        self.commands,
                        "\x1b_Ga=t,t=d,f=32,s={},v={},i={},q=0,m={more};",
                        self.tile * 48,
                        self.tile,
                        self.id
                    );
                } else {
                    let _ = write!(self.commands, "\x1b_Gm={more};");
                }
                self.commands.push_str(&self.upload[self.upload_at..end]);
                self.commands.push_str("\x1b\\");
                self.upload_at = end;
                self.deadline = Some(now + Duration::from_millis(500));
            }
            Phase::Placement if self.placed == 0 => {
                // Transparent crop validates crop, sub-cell offset, C=1,
                // placement identity and z=-1 without covering any text.
                let _ = write!(
                    self.commands,
                    "\x1b7\x1b[1;1H\x1b_Ga=p,i={},p={},x=0,y=0,w={},h={},X=0,Y=0,C=1,z=-1,q=0\x1b\\\x1b8",
                    self.id,
                    u32::MAX,
                    self.tile,
                    self.tile
                );
                self.placed = 1;
            }
            _ => {}
        }
        self.commands.as_bytes()
    }
    pub fn present(
        &mut self,
        particles: &[Particle],
        viewport: [u16; 4],
        lifetime_ms: u16,
        fps: u16,
        now: Instant,
    ) -> &[u8] {
        self.commands.clear();
        if self.phase != Phase::Ready {
            return self.commands.as_bytes();
        }
        if particles.is_empty() {
            if self.placed > 0 {
                let _ = write!(self.commands, "\x1b_Ga=d,d=i,i={},q=2\x1b\\", self.id);
                self.placed = 0;
            }
            return self.commands.as_bytes();
        }
        let interval = Duration::from_secs_f64(1.0 / f64::from(fps.max(1)));
        if self.next_present.is_some_and(|t| now < t) {
            return self.commands.as_bytes();
        }
        let due = self.next_present.unwrap_or(now);
        let remainder = now.saturating_duration_since(due).as_nanos() % interval.as_nanos();
        self.next_present =
            Some(now + Duration::from_nanos((interval.as_nanos() - remainder) as u64));
        let Some(cell) = self.cell else {
            return self.commands.as_bytes();
        };
        let [left, top, width, height] = viewport;
        let cw = f32::from(cell.width);
        let ch = f32::from(cell.height);
        let half = f32::from(self.tile) / 2.0;
        let mut count = 0;
        self.commands.push_str("\x1b7");
        for p in particles.iter().take(MAX_PARTICLES) {
            let x = (p.position[0] - half).floor();
            let y = (p.position[1] - half).floor();
            if x < f32::from(left) * cw
                || y < f32::from(top) * ch
                || x + f32::from(self.tile) > f32::from(left.saturating_add(width)) * cw
                || y + f32::from(self.tile) > f32::from(top.saturating_add(height)) * ch
            {
                continue;
            }
            let alpha = (p.remaining * 1000.0 / f32::from(lifetime_ms) * 15.0)
                .ceil()
                .clamp(0.0, 15.0) as u16;
            if alpha == 0 {
                continue;
            }
            count += 1;
            let x = x as u32;
            let y = y as u32;
            let crop = (u16::from(p.size.min(2)) * 16 + alpha) * self.tile;
            let _ = write!(
                self.commands,
                "\x1b[{};{}H\x1b_Ga=p,i={},p={count},x={crop},y=0,w={},h={},X={},Y={},C=1,z=-{count},q=2\x1b\\",
                y / u32::from(cell.height) + 1,
                x / u32::from(cell.width) + 1,
                self.id,
                self.tile,
                self.tile,
                x % u32::from(cell.width),
                y % u32::from(cell.height)
            );
        }
        for p in count + 1..=self.placed {
            let _ = write!(self.commands, "\x1b_Ga=d,d=i,i={},p={p},q=2\x1b\\", self.id);
        }
        self.commands.push_str("\x1b8");
        if count == 0 && self.placed == 0 {
            self.commands.clear();
        }
        self.placed = count;
        debug_assert!(self.commands.len() < 16 * 1024);
        self.commands.as_bytes()
    }
    pub fn frame_cost(&mut self, elapsed: Duration, budget: Duration) {
        if self.phase == Phase::Ready {
            self.slow = if elapsed > budget.saturating_mul(2) {
                self.slow.saturating_add(1)
            } else {
                0
            };
            if self.slow >= 3 {
                self.fail();
            }
        }
    }
    pub fn finish(&mut self) -> &[u8] {
        self.phase = Phase::Disabled;
        self.deadline = None;
        self.free |= self.allocated;
        self.service(Instant::now())
    }
}

/// Crossterm 0.29 decodes APC/DCS envelopes as Alt-_ / Alt-P followed by
/// ordinary ASCII keys. Consume only our exact replies; replay other input.
/// This runs in the existing input reader, never in an additional thread.
pub struct ReplyFilter {
    held: Vec<Event>,
    text: String,
    kind: Option<char>,
    at: Option<Instant>,
    id: u32,
    version: bool,
}
impl ReplyFilter {
    pub fn new(id: u32) -> Self {
        Self {
            held: Vec::with_capacity(256),
            text: String::with_capacity(256),
            kind: None,
            at: None,
            id,
            version: true,
        }
    }
    pub fn holding(&self) -> bool {
        self.kind.is_some()
    }
    pub fn version_pending(&mut self, pending: bool) {
        self.version = pending;
    }
    pub fn expired(&self, now: Instant) -> bool {
        self.at
            .is_some_and(|t| now.saturating_duration_since(t) >= Duration::from_millis(50))
    }
    pub fn flush(&mut self, mut input: impl FnMut(Event)) {
        for event in self.held.drain(..) {
            input(event);
        }
        self.text.clear();
        self.kind = None;
        self.at = None;
    }
    pub fn push(
        &mut self,
        event: Event,
        now: Instant,
        mut input: impl FnMut(Event),
        mut reply: impl FnMut(String),
    ) {
        let key = match &event {
            Event::Key(k) => Some(*k),
            _ => None,
        };
        if self.kind.is_none() {
            if let Some(k) = key
                && k.modifiers.contains(KeyModifiers::ALT)
                && !k.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(k.code, KeyCode::Char('_' | 'P'))
                && (k.code == KeyCode::Char('_') || self.version)
            {
                self.kind = match k.code {
                    KeyCode::Char(c) => Some(c),
                    _ => None,
                };
                self.at = Some(now);
                self.held.push(event);
                return;
            }
            input(event);
            return;
        }
        if let Some(k) = key {
            if k.code == KeyCode::Char('\\') && k.modifiers.contains(KeyModifiers::ALT) {
                let ours = if self.kind == Some('P') {
                    self.text.starts_with(">|")
                } else {
                    self.text
                        .strip_prefix("Gi=")
                        .and_then(|s| s.split([',', ';']).next())
                        .and_then(|s| s.parse::<u32>().ok())
                        .is_some_and(|id| id == self.id || id == self.id + 1)
                };
                if ours {
                    reply(std::mem::take(&mut self.text));
                    self.held.clear();
                    self.kind = None;
                    self.at = None;
                } else {
                    self.held.push(event);
                    self.flush(input);
                }
                return;
            }
            if let KeyCode::Char(c) = k.code
                && c.is_ascii()
                && !k
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                && self.held.len() < 255
            {
                self.text.push(c);
                self.held.push(event);
                let prefix = if self.kind == Some('P') { ">|" } else { "Gi=" };
                if self.text.len() <= prefix.len() && !prefix.starts_with(&self.text) {
                    self.flush(input);
                }
                return;
            }
        }
        self.flush(&mut input);
        input(event);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::Config;
    #[cfg(feature = "perf")]
    #[test]
    #[ignore = "release headless sprite/simulation allocation and output benchmark"]
    fn pixiedust_budget() {
        use crate::pixiedust::Particles;
        let (mut g, now) = ready();
        let mut model = Particles::default();
        let config = crate::config::PixiedustConfig::default();
        let mut samples = Vec::with_capacity(10000);
        let mut bytes = 0;
        let mut max_bytes = 0;
        let mut max_particles = 0;
        // Warm all retained buffers before allocation accounting.
        model.emit([100.0, 100.0], [300.0, 100.0], 20.0, config);
        g.present(model.particles(), [0, 0, 120, 40], 500, 60, now);
        let (allocations, allocated_bytes) = crate::measurement::allocations(|| {
            for i in 1..=10000 {
                let at = Instant::now();
                model.advance(1.0 / 60.0);
                let x = 100.0 + (i % 60) as f32 * 10.0;
                model.emit([x, 100.0], [x + 10.0, 100.0], 20.0, config);
                let output = g.present(
                    model.particles(),
                    [0, 0, 120, 40],
                    500,
                    60,
                    now + Duration::from_nanos(i as u64 * 16_666_667),
                );
                bytes += output.len();
                max_bytes = max_bytes.max(output.len());
                max_particles = max_particles.max(model.particles().len());
                samples.push(at.elapsed().as_nanos() as u64);
            }
        });
        samples.sort_unstable();
        let p = |n: usize| samples[(samples.len() * n).div_ceil(100) - 1] as f64 / 1000.0;
        println!(
            "pixiedust_budget: samples={} p50_us={:.3} p95_us={:.3} p99_us={:.3} allocations={allocations} allocated_bytes={allocated_bytes} avg_protocol_bytes={:.1} max_protocol_bytes={max_bytes} max_particles={max_particles} atlas_base64_bytes={} retained_command_capacity={} retained_atlas_capacity={}",
            samples.len(),
            p(50),
            p(95),
            p(99),
            bytes as f64 / samples.len() as f64,
            g.upload.len(),
            g.commands.capacity(),
            g.upload.capacity()
        );
        assert_eq!((allocations, allocated_bytes), (0, 0));
        assert!(max_bytes < 16 * 1024);
    }
    pub(crate) fn ready() -> (Graphics, Instant) {
        let now = Instant::now();
        let mut g = Graphics::new(true, CellSize::new(10, 20), [196, 167, 231], false);
        g.configure(Config::default().effects(), g.color);
        assert!(!g.service(now).is_empty());
        g.reply(">|kitty(0.40.1)", now);
        g.reply(&format!("Gi={};OK", g.id + 1), now);
        g.service(now);
        while g.upload_at < g.upload.len() {
            assert!(g.service(now).len() < 4256);
        }
        g.reply(&format!("Gi={};OK", g.id), now);
        let probe = String::from_utf8(g.service(now).to_vec()).unwrap();
        assert!(probe.contains("C=1,z=-1,q=0"));
        assert!(g.ready().is_none());
        g.reply(&format!("Gi={},p={};OK", g.id, u32::MAX), now);
        g.service(now);
        assert!(g.ready().is_some());
        (g, now)
    }
    #[test]
    fn only_queried_kitty_profile_can_select_pixels() {
        let get = |k: &str| (k == "KITTY_WINDOW_ID").then(|| "1".into());
        assert!(candidate(get, true));
        assert!(!candidate(get, false));
        for blocked in [
            "SSH_CONNECTION",
            "SSH_TTY",
            "TMUX",
            "STY",
            "ZELLIJ",
            "ZELLIJ_SESSION_NAME",
            "WEZTERM_PANE",
            "WEZTERM_EXECUTABLE",
        ] {
            assert!(!candidate(
                |k| ((k == "KITTY_WINDOW_ID") || k == blocked).then(|| "1".into()),
                true
            ));
        }
        assert!(!candidate(|_| None, true));
        let now = Instant::now();
        let mut g = Graphics::new(true, CellSize::new(10, 20), [0; 3], false);
        g.configure(Config::default().effects(), [0; 3]);
        g.service(now);
        g.reply(&format!("Gi={};OK", g.id + 1), now);
        g.service(now);
        assert!(g.ready().is_none());
        assert!(g.upload.is_empty());
        g.reply(">|WezTerm(2026)", now);
        g.service(now);
        assert!(g.ready().is_none());
        assert!(g.waiting(now).is_none());
        for version in [
            ">|kitty(0.19.3)",
            ">|Ghostty(1.2.0)",
            ">|kitty(bad)",
            "kitty(0.40.0)",
        ] {
            assert!(!kitty_version(version));
        }
        assert!(kitty_version(">|kitty(0.20.0)"));
    }
    #[test]
    fn absent_metrics_off_smear_and_timeout_never_emit_pixel_frames() {
        let now = Instant::now();
        for effect in [CursorEffect::Off, CursorEffect::Smear] {
            let mut effects = Config::default().effects();
            effects.cursor_effect = effect;
            let mut g = Graphics::new(true, CellSize::new(10, 20), [0; 3], false);
            g.configure(effects, [0; 3]);
            assert!(g.service(now).is_empty());
            assert!(g.waiting(now).is_none());
            assert_eq!(g.commands.capacity(), 0);
        }
        let mut g = Graphics::new(true, None, [0; 3], false);
        g.configure(Config::default().effects(), [0; 3]);
        assert!(g.service(now).is_empty());
        let mut g = Graphics::new(true, CellSize::new(10, 20), [0; 3], false);
        g.configure(Config::default().effects(), [0; 3]);
        g.service(now);
        assert!(g.service(now + Duration::from_secs(1)).is_empty());
        assert!(g.ready().is_none());
        assert!(g.waiting(now).is_none());
    }
    #[test]
    fn atlas_alpha_placements_and_output_are_bounded() {
        let (mut g, now) = ready();
        assert!(!g.filter.load(Ordering::Acquire));
        let rgba = STANDARD.decode(&g.upload).unwrap();
        let tile = usize::from(g.tile);
        let width = tile * 48;
        assert_eq!(rgba.len(), width * tile * 4);
        for y in 0..tile {
            for x in 0..tile {
                assert_eq!(rgba[(y * width + x) * 4 + 3], 0);
            }
        }
        assert!(rgba.chunks_exact(4).any(|p| p[3] == 200));
        let p = Particle {
            position: [100.0, 100.0],
            velocity: [0.0; 2],
            curl: 0.0,
            remaining: 0.4,
            size: 1,
        };
        let bytes = g
            .present(&[p; MAX_PARTICLES], [0, 0, 120, 40], 500, 60, now)
            .to_vec();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(bytes.len() < 16 * 1024);
        assert_eq!(text.matches("a=p,").count(), MAX_PARTICLES);
        assert!(!text.contains("a=t,"));
        assert!(text.contains("C=1,z=-1,q=2"));
        assert!(text.contains("z=-96,"));
        assert!(text.starts_with("\x1b7"));
        assert!(text.ends_with("\x1b8"));
        assert!(
            g.present(
                &[p],
                [0, 0, 120, 40],
                500,
                60,
                now + Duration::from_millis(1)
            )
            .is_empty()
        );
        let cleanup = std::str::from_utf8(g.present(&[], [0; 4], 500, 60, now)).unwrap();
        assert!(cleanup.contains("a=d,d=i"));
        assert!(g.present(&[], [0; 4], 500, 60, now).is_empty());
        assert!(g.waiting(now).is_none());
        let final_cleanup = std::str::from_utf8(g.finish()).unwrap();
        assert!(final_cleanup.contains("a=d,d=I"));
        assert!(!final_cleanup.contains("d=A"));
    }
    #[test]
    fn output_cap_keeps_phase_after_late_input_frames() {
        let (mut g, now) = ready();
        let p = Particle {
            position: [100.0, 100.0],
            velocity: [0.0; 2],
            curl: 0.0,
            remaining: 0.4,
            size: 0,
        };
        for micros in [0, 17000, 33400, 50001] {
            assert!(
                !g.present(
                    &[p],
                    [0, 0, 120, 40],
                    500,
                    60,
                    now + Duration::from_micros(micros)
                )
                .is_empty()
            );
        }
        assert!(
            g.present(
                &[p],
                [0, 0, 120, 40],
                500,
                60,
                now + Duration::from_micros(51000)
            )
            .is_empty()
        );
    }
    #[test]
    fn cleanup_only_targets_owned_image_and_can_retry_io_failure() {
        struct Fails;
        impl io::Write for Fails {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("injected output failure"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let old = OWNED_IMAGE.swap(123456789, Ordering::AcqRel);
        assert!(cleanup_owned(&mut Fails).is_err());
        assert_eq!(OWNED_IMAGE.load(Ordering::Acquire), 123456789);
        let mut bytes = Vec::new();
        cleanup_owned(&mut bytes).unwrap();
        assert_eq!(bytes, b"\x1b_Ga=d,d=I,i=123456789,q=2\x1b\\");
        assert_eq!(OWNED_IMAGE.load(Ordering::Acquire), 0);
        OWNED_IMAGE.store(old, Ordering::Release);
    }
    #[test]
    fn resize_context_cleanup_errors_and_slow_frames_degrade_without_polling() {
        let (mut g, now) = ready();
        g.resize(None, Config::default().effects(), [0; 3]);
        let cleanup = g.service(now);
        assert!(std::str::from_utf8(cleanup).unwrap().contains("d=I"));
        assert!(g.ready().is_none());
        assert!(g.waiting(now).is_none());
        let (mut g, now) = ready();
        for _ in 0..3 {
            g.frame_cost(Duration::from_millis(50), Duration::from_millis(16));
        }
        assert!(g.ready().is_none());
        assert!(g.waiting(now).is_none());
        assert!(!g.service(now).is_empty());
        let (mut g, now) = ready();
        g.reply(&format!("Gi={};ENOENT:evicted", g.id), now);
        assert!(g.ready().is_none());
        assert!(!g.service(now).is_empty());
    }
    fn key(c: char, alt: bool) -> Event {
        Event::Key(crossterm::event::KeyEvent::new(
            KeyCode::Char(c),
            if alt {
                KeyModifiers::ALT
            } else {
                KeyModifiers::NONE
            },
        ))
    }
    fn envelope(prefix: char, text: &str) -> Vec<Event> {
        std::iter::once(key(prefix, true))
            .chain(text.chars().map(|c| key(c, false)))
            .chain(std::iter::once(key('\\', true)))
            .collect()
    }
    #[test]
    fn crossterm_replies_are_consumed_and_normal_input_is_preserved() {
        let mut filter = ReplyFilter::new(123);
        let now = Instant::now();
        let mut keys = Vec::new();
        let mut replies = Vec::new();
        for event in [key('i', false)]
            .into_iter()
            .chain(envelope('_', "Gi=123;OK"))
            .chain(envelope('P', ">|kitty(0.40.0)"))
            .chain([key('x', false)])
        {
            filter.push(event, now, |e| keys.push(e), |r| replies.push(r));
        }
        assert_eq!(keys, vec![key('i', false), key('x', false)]);
        assert_eq!(replies, vec!["Gi=123;OK", ">|kitty(0.40.0)"]);
        let unknown = envelope('_', "Gi=999;OK");
        for e in unknown.clone() {
            filter.push(e, now, |e| keys.push(e), |r| replies.push(r));
        }
        assert_eq!(&keys[2..], &unknown);
        filter.push(key('_', true), now, |e| keys.push(e), |r| replies.push(r));
        assert!(filter.expired(now + Duration::from_millis(51)));
        filter.flush(|e| keys.push(e));
        assert_eq!(keys.last(), Some(&key('_', true)));
        filter.version_pending(false);
        filter.push(key('P', true), now, |e| keys.push(e), |r| replies.push(r));
        assert!(!filter.holding());
        assert_eq!(keys.last(), Some(&key('P', true)));
    }
}

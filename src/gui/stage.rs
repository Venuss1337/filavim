//! The stage: the world behind FilaVim and the light-bearing pane material.
//!
//! The stage is a root sibling painted *beneath* the cached editor view, so its
//! motion (mode light) and its asynchronous material bakes never rebuild editor
//! layout. It owns:
//! - the world image: the configured wallpaper (decoded off-thread, bounded) or,
//!   when none is configured, a procedural atmosphere in theme colors;
//! - the pane materials: one baked frosted crop per pane ([`super::frost`]) or,
//!   for solid glass and while a bake is pending, a gradient fill; plus each
//!   pane's shadow, outline, lit rim and sheen;
//! - the mode light: a tint on the pane rims and a broad glow in the world above
//!   the editor pane, both settling toward the current mode color.
//!
//! Geometry comes from [`design::Shell`], the same function the editor uses; the
//! editor reports the explorer width with [`Stage::set_sidebar`]. Bakes are keyed
//! by everything that affects their pixels, debounced during live resize,
//! generation-checked on completion, and replaced images leave the sprite atlas.
//! A settled stage schedules no frames.
use super::{
    design::{self, Scale, Shell},
    frost::{self, Bake, Glow, Rgba, World},
    motion::{Character, Clock, Motion},
    theme::{Hue, Theme},
    wallpaper::decode,
};
use crate::{
    appearance::{BackgroundConfig, Fit, Glass},
    config::Config,
};
use gpui::{prelude::*, *};
use std::{path::Path, sync::Arc, time::Duration};

#[derive(Clone)]
pub struct WallpaperError(pub String);

/// The world image and its reduced copy for baking.
struct Scenery {
    image: Arc<RenderImage>,
    world: Arc<World>,
    /// Generated from the theme rather than loaded from disk.
    atmosphere: bool,
}
/// One pane's material: the bake painted now and the one in flight.
#[derive(Default)]
struct Material {
    painted: Option<(Bake, Arc<RenderImage>)>,
    pending: Option<Bake>,
    task: Option<Task<()>>,
    generation: u64,
}
pub struct Stage {
    settings: BackgroundConfig,
    transparent: bool,
    theme: Theme,
    scale: Scale,
    reduced_motion: bool,
    sidebar: Option<f32>,
    scenery: Option<Scenery>,
    load: Option<Task<()>>,
    generation: u64,
    /// Indexed like [`Shell::panes`] with the editor last: [explorer, editor].
    materials: [Material; 2],
    clock: Clock,
    light: Motion,
    light_from: Hsla,
    light_to: Hsla,
    pub frames: u64,
}
impl EventEmitter<WallpaperError> for Stage {}
impl Stage {
    pub fn new(config: &Config, theme: Theme, cx: &mut Context<Self>) -> Self {
        cx.on_release(|this, cx| this.release(cx)).detach();
        let normal = theme.mode(crate::editor::Mode::Normal);
        let mut this = Self {
            settings: config.background.clone(),
            transparent: config.appearance.transparent,
            theme,
            scale: Scale::new(config.font_size),
            reduced_motion: config.cursor.reduced_motion,
            sidebar: None,
            scenery: None,
            load: None,
            generation: 0,
            materials: Default::default(),
            clock: Clock::default(),
            light: Motion::new(1., Character::Settle),
            light_from: normal,
            light_to: normal,
            frames: 0,
        };
        this.load(cx);
        this
    }
    /// Applies changed settings. The world reloads only when its source or the
    /// material that consumes it changed; every other change rebakes lazily.
    pub fn configure(
        &mut self,
        config: &Config,
        theme: Theme,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        let next = &config.background;
        let reload = force
            || self.settings.enabled != next.enabled
            || self.settings.image != next.image
            || self.transparent != config.appearance.transparent
            || self.theme.glass != theme.glass
            // The atmosphere is drawn in theme colors.
            || (self.scenery.as_ref().is_none_or(|s| s.atmosphere)
                && (self.theme.base != theme.base || self.theme.accent != theme.accent));
        self.settings = next.clone();
        self.transparent = config.appearance.transparent;
        self.theme = theme;
        self.scale = Scale::new(config.font_size);
        self.reduced_motion = config.cursor.reduced_motion;
        if reload {
            self.load(cx);
        }
        cx.notify();
    }
    /// Retargets the mode light. Cheap and idempotent: the editor calls it from
    /// every prepaint and only a changed color starts motion.
    pub fn set_mode(&mut self, color: Hsla, cx: &mut Context<Self>) {
        if color == self.light_to {
            return;
        }
        let now = self.clock.now();
        self.light_from = self.mode_light(now);
        self.light_to = color;
        self.light.jump(0., now);
        self.light.to(1., now, self.reduced_motion);
        cx.notify();
    }
    /// The explorer pane's requested width, or `None` while it is hidden.
    pub fn set_sidebar(&mut self, width: Option<f32>, cx: &mut Context<Self>) {
        if self.sidebar != width {
            self.sidebar = width;
            cx.notify();
        }
    }
    fn mode_light(&mut self, now: f64) -> Hsla {
        mix(self.light_from, self.light_to, self.light.unit(now))
    }
    fn release(&mut self, cx: &mut App) {
        if let Some(scenery) = self.scenery.take() {
            cx.drop_image(scenery.image, None);
        }
        for material in &mut self.materials {
            if let Some((_, image)) = material.painted.take() {
                cx.drop_image(image, None);
            }
            material.pending = None;
            material.task = None;
        }
    }
    fn load(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.load = None;
        self.release(cx);
        // Opaque themes keep a plain world; a disabled background means no image.
        if !self.settings.enabled || !self.transparent {
            return;
        }
        // Frosted glass reads the world blurred; clear glass needs it sharp.
        let (width, radius) = match self.theme.glass {
            Glass::Frosted => (480, 8),
            _ => (1280, 0),
        };
        let path = self
            .settings
            .image
            .clone()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|path| {
                // Relative wallpapers are anchored to the configuration directory.
                if path.is_absolute() {
                    path
                } else {
                    Config::path().parent().unwrap_or(Path::new(".")).join(path)
                }
            });
        let theme = self.theme;
        let work = cx.background_executor().spawn(async move {
            let (image, atmosphere) = match path {
                Some(path) => (decode(&path)?, false),
                None => (atmosphere(&theme), true),
            };
            let size = image.size(0);
            let world = World::new(
                image.as_bytes(0).unwrap_or_default(),
                size.width.0 as u32,
                size.height.0 as u32,
                width,
                radius,
            );
            anyhow::Ok(Scenery {
                image,
                world: Arc::new(world),
                atmosphere,
            })
        });
        self.load = Some(cx.spawn(async move |stage, cx| {
            let result = work.await;
            let _ = stage.update(cx, |stage, cx| {
                if stage.generation != generation {
                    return;
                }
                match result {
                    Ok(scenery) => stage.scenery = Some(scenery),
                    Err(error) => {
                        cx.emit(WallpaperError(format!("Wallpaper unavailable: {error:#}")))
                    }
                }
                stage.load = None;
                cx.notify();
            });
        }));
    }
    /// Starts a debounced bake of pane `index` unless it is painted or in flight.
    fn schedule_bake(&mut self, index: usize, params: Bake, cx: &mut Context<Self>) {
        let material = &mut self.materials[index];
        if material.painted.as_ref().is_some_and(|(p, _)| *p == params)
            || material.pending.as_ref() == Some(&params)
        {
            return;
        }
        let Some(world) = self.scenery.as_ref().map(|s| s.world.clone()) else {
            return;
        };
        material.generation = material.generation.wrapping_add(1);
        let generation = material.generation;
        material.pending = Some(params.clone());
        let executor = cx.background_executor().clone();
        // The first bake is immediate; later ones wait out a live resize.
        let delay = if material.painted.is_some() { 90 } else { 0 };
        material.task = Some(cx.spawn(async move |stage, cx| {
            executor.timer(Duration::from_millis(delay)).await;
            let current = stage
                .read_with(cx, |stage, _| {
                    stage.materials[index].generation == generation
                })
                .unwrap_or(false);
            if !current {
                return;
            }
            let key = params.clone();
            let image = executor
                .spawn(async move {
                    let (w, h, pixels) = frost::bake(Some(&world), &params);
                    image::RgbaImage::from_raw(w as u32, h as u32, pixels)
                        .map(|frame| Arc::new(RenderImage::new(vec![image::Frame::new(frame)])))
                })
                .await;
            let _ = stage.update(cx, |stage, cx| {
                let material = &mut stage.materials[index];
                if material.generation != generation {
                    return;
                }
                material.pending = None;
                material.task = None;
                if let Some(image) = image {
                    if let Some((_, old)) = material.painted.replace((key, image)) {
                        cx.drop_image(old, None);
                    }
                    cx.notify();
                }
            });
        }));
    }
}
/// The procedural world in theme colors: a night gradient lit by broad glows.
fn atmosphere(theme: &Theme) -> Arc<RenderImage> {
    let (w, h) = (1600, 1000);
    let base: Rgba = theme.base.into();
    let lift = |c: Rgba, k: f32| Rgba([c.0[0] + k, c.0[1] + k, c.0[2] + k * 1.6, 1.]);
    let strength = if theme.light { 0.45 } else { 0.62 };
    let glow = |color: Hsla, center: (f32, f32), radius: f32, alpha: f32| Glow {
        center,
        radius,
        color: Rgba({
            let c: Rgba = color.into();
            [c.0[0], c.0[1], c.0[2], alpha * strength]
        }),
    };
    let glows = [
        glow(theme.hue(Hue::Cyan), (0.58, 0.46), 0.36, 0.85),
        glow(theme.accent, (0.18, 0.1), 0.4, 0.8),
        glow(theme.hue(Hue::Purple), (0.88, 0.88), 0.44, 0.7),
        glow(theme.hue(Hue::Green), (0.28, 0.92), 0.3, 0.45),
    ];
    let pixels = frost::atmosphere(w, h, lift(base, 0.03), base, &glows);
    let frame = image::RgbaImage::from_raw(w as u32, h as u32, pixels)
        .expect("atmosphere buffer matches its dimensions");
    Arc::new(RenderImage::new(vec![image::Frame::new(frame)]))
}
fn corners(r: f32) -> Corners<Pixels> {
    Corners::all(px(r))
}
impl Render for Stage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frames += 1;
        let viewport = window.viewport_size();
        let shell = Shell::new(
            viewport,
            self.scale,
            window.is_maximized() || window.is_fullscreen(),
            self.sidebar,
        );
        let theme = self.theme;
        let now = self.clock.now();
        let light = self.mode_light(now);
        if self.light.active() {
            window.request_animation_frame();
        }
        let fit = self.settings.fit;
        let panes: Vec<Bounds<Pixels>> = shell.panes().collect();
        // Material slots: the explorer bakes into slot 0, the editor into slot 1.
        let slots: Vec<usize> = if panes.len() == 2 {
            vec![0, 1]
        } else {
            vec![1]
        };
        if theme.glass != Glass::Solid
            && let Some((world_atmosphere, _)) = self.scenery.as_ref().map(|s| (s.atmosphere, ()))
        {
            for (bounds, slot) in panes.iter().zip(&slots) {
                let params = Bake {
                    viewport: (f32::from(viewport.width), f32::from(viewport.height)),
                    slab: [
                        f32::from(bounds.left()),
                        f32::from(bounds.top()),
                        f32::from(bounds.size.width),
                        f32::from(bounds.size.height),
                    ],
                    scale: window.scale_factor(),
                    fit: if world_atmosphere { Fit::Cover } else { fit },
                    opacity: if world_atmosphere {
                        1.
                    } else {
                        self.settings.opacity
                    },
                    base: theme.base.into(),
                    world_tint: if world_atmosphere {
                        Rgba([0., 0., 0., 0.])
                    } else {
                        theme.world_tint.into()
                    },
                    glass_top: theme.glass_top.into(),
                    glass_bottom: theme.glass_bottom.into(),
                    grain: if theme.light { 3. } else { 5. },
                    ceiling: if theme.light { 0. } else { 0.05 },
                    floor: if theme.light { 0.62 } else { 0. },
                };
                self.schedule_bake(*slot, params, cx);
            }
        }
        let scenery = self
            .scenery
            .as_ref()
            .map(|s| (s.image.clone(), s.atmosphere));
        let baked: Vec<Option<Arc<RenderImage>>> = slots
            .iter()
            .map(|slot| {
                self.materials[*slot]
                    .painted
                    .as_ref()
                    .map(|(_, image)| image.clone())
            })
            .collect();
        let opacity = self.settings.opacity;
        let slab = shell.slab;
        canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                let radius = design::radius::SLAB;
                window.paint_quad(fill(bounds, theme.base));
                if let Some((image, atmosphere)) = &scenery {
                    let size = image.size(0);
                    let fit = if *atmosphere { Fit::Cover } else { fit };
                    let [x, y, w, h] = frost::fitted(
                        fit,
                        (f32::from(bounds.size.width), f32::from(bounds.size.height)),
                        (size.width.0 as u32, size.height.0 as u32),
                    );
                    let area = Bounds::new(point(px(x), px(y)), gpui::size(px(w), px(h)));
                    let _ = window.paint_image(area, corners(0.), image.clone(), 0, false);
                    if !*atmosphere {
                        // `background.opacity` fades the image toward the night.
                        window.paint_quad(fill(bounds, theme.base.opacity(1. - opacity)));
                        window.paint_quad(fill(bounds, theme.world_tint));
                    }
                }
                // Mode light: a broad, low glow in the world above the editor pane.
                window.paint_shadows(
                    Bounds::new(
                        point(slab.left() + slab.size.width * 0.18, slab.top() + px(6.)),
                        gpui::size(slab.size.width * 0.64, px(16.)),
                    ),
                    corners(8.),
                    &[BoxShadow {
                        color: light.opacity(if theme.light { 0.12 } else { 0.22 }),
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(70.),
                        spread_radius: px(0.),
                    }],
                );
                for (pane, image) in panes.iter().zip(&baked) {
                    let pane = *pane;
                    // Lift: a broad ambient shadow and a tight contact shadow.
                    window.paint_shadows(
                        pane,
                        corners(radius),
                        &[
                            BoxShadow {
                                color: theme.shadow.opacity(0.7),
                                offset: point(px(0.), px(16.)),
                                blur_radius: px(40.),
                                spread_radius: px(-8.),
                            },
                            BoxShadow {
                                color: theme.shadow.opacity(0.45),
                                offset: point(px(0.), px(2.)),
                                blur_radius: px(5.),
                                spread_radius: px(0.),
                            },
                        ],
                    );
                    match image {
                        Some(image) if theme.glass != Glass::Solid => {
                            let _ =
                                window.paint_image(pane, corners(radius), image.clone(), 0, false);
                        }
                        _ => window.paint_quad(
                            fill(
                                pane,
                                linear_gradient(
                                    180.,
                                    linear_color_stop(theme.glass_top, 0.),
                                    linear_color_stop(theme.glass_bottom, 1.),
                                ),
                            )
                            .corner_radii(corners(radius)),
                        ),
                    }
                    // Sheen inside the top edge, then the outline and the lit rim.
                    // The rim takes some of the mode light, so the instrument
                    // answers a mode change at the edge of vision.
                    window.paint_quad(
                        fill(
                            Bounds::new(pane.origin, gpui::size(pane.size.width, px(56.))),
                            linear_gradient(
                                180.,
                                linear_color_stop(theme.sheen, 0.),
                                linear_color_stop(theme.sheen.opacity(0.), 1.),
                            ),
                        )
                        .corner_radii(Corners {
                            top_left: px(radius),
                            top_right: px(radius),
                            bottom_left: px(0.),
                            bottom_right: px(0.),
                        }),
                    );
                    window.paint_quad(
                        outline(pane, theme.edge, BorderStyle::Solid).corner_radii(corners(radius)),
                    );
                    let rim = mix(
                        theme.rim,
                        light.opacity(theme.rim.a.max(0.5)),
                        if theme.light { 0.25 } else { 0.45 },
                    );
                    window.paint_quad(PaintQuad {
                        bounds: pane,
                        corner_radii: corners(radius),
                        background: transparent_black().into(),
                        border_widths: Edges {
                            top: px(1.),
                            right: px(0.),
                            bottom: px(0.),
                            left: px(0.),
                        },
                        border_color: rim,
                        border_style: BorderStyle::Solid,
                    });
                }
            },
        )
        .size_full()
    }
}
/// Straight RGBA interpolation; `t` belongs to `b`.
pub fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let a: gpui::Rgba = a.into();
    let b: gpui::Rgba = b.into();
    gpui::Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
    .into()
}

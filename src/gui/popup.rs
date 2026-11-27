//! Logical-pixel popup placement and keyboard policy, independent of GPUI layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presentation {
    Completion,
    ContextMenu,
    Information,
    InformationCentered,
    Picker,
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
impl Rect {
    pub fn right(self) -> f32 {
        self.x + self.width
    }
    pub fn bottom(self) -> f32 {
        self.y + self.height
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub menu: Rect,
    pub documentation: Option<Rect>,
    pub above: bool,
}
/// A signature is secondary to completion. Hide it if neither side of the caret
/// has room, rather than covering the list or its documentation.
pub fn assistance(view: Rect, caret: Rect, occupied: &[Rect], height: f32) -> Option<Rect> {
    let margin = 8.;
    let width = 450f32.min(view.width - margin * 2.).max(1.);
    let height = height.min(view.height - margin * 2.).max(1.);
    let x = caret.x.clamp(
        view.x + margin,
        (view.right() - margin - width).max(view.x + margin),
    );
    [caret.y - height - 8., caret.bottom() + 8.]
        .into_iter()
        .map(|y| Rect {
            x,
            y,
            width,
            height,
        })
        .find(|r| {
            r.y >= view.y + margin
                && r.bottom() <= view.bottom() - margin
                && occupied.iter().all(|o| {
                    r.right() + 4. <= o.x
                        || o.right() + 4. <= r.x
                        || r.bottom() + 4. <= o.y
                        || o.bottom() + 4. <= r.y
                })
        })
}
pub fn place(view: Rect, caret: Option<Rect>, size: (f32, f32), documentation: bool) -> Placement {
    let margin = 8.;
    let width = size.0.min((view.width - 2. * margin).max(1.));
    let height = size.1.min((view.height - 2. * margin).max(1.));
    let Some(caret) = caret else {
        return Placement {
            menu: Rect {
                x: view.x + (view.width - width) / 2.,
                y: view.y + (view.height - height) / 3.,
                width,
                height,
            },
            documentation: None,
            above: false,
        };
    };
    let below = (view.bottom() - margin - caret.bottom() - 4.).max(0.);
    let above_space = (caret.y - view.y - margin - 4.).max(0.);
    let above = below < height && above_space > below;
    let height = height.min(if above { above_space } else { below }).max(1.);
    let x = caret.x.clamp(
        view.x + margin,
        (view.right() - margin - width).max(view.x + margin),
    );
    let mut y = if above {
        caret.y - 4. - height
    } else {
        caret.bottom() + 4.
    };
    let height = if above_space.max(below) < 96. {
        y = (caret.bottom() + 4.).clamp(
            view.y + margin,
            (view.bottom() - margin - height).max(view.y + margin),
        );
        (size.1.min(view.height - 2. * margin)).max(1.)
    } else {
        height
    };
    y = y.clamp(
        view.y + margin,
        (view.bottom() - margin - height).max(view.y + margin),
    );
    let menu = Rect {
        x,
        y,
        width,
        height,
    };
    let documentation = if documentation {
        let right = view.right() - margin - menu.right() - 6.;
        let left = menu.x - view.x - margin - 6.;
        let space = right.max(left);
        if space >= 180. {
            let w = space.min(360.);
            Some(Rect {
                x: if right >= left {
                    menu.right() + 6.
                } else {
                    menu.x - 6. - w
                },
                y: y.clamp(
                    view.y + margin,
                    (view.bottom() - margin - 240.).max(view.y + margin),
                ),
                width: w,
                height: 240f32.min((view.height - 2. * margin).max(1.)),
            })
        } else {
            None
        }
    } else {
        None
    };
    Placement {
        menu,
        documentation,
        above,
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Next,
    Previous,
    Accept,
    Dismiss,
    Scroll(i32),
    Pass,
    Block,
}
pub fn route(
    presentation: Presentation,
    key: &str,
    control: bool,
    modified: bool,
    shift: bool,
    enter: bool,
    tab: bool,
) -> Route {
    if key == "escape" {
        return Route::Dismiss;
    }
    if matches!(
        presentation,
        Presentation::Information | Presentation::InformationCentered
    ) {
        return match key {
            "pagedown" => Route::Scroll(1),
            "pageup" => Route::Scroll(-1),
            _ => Route::Pass,
        };
    }
    if control && key == "n" {
        return Route::Next;
    }
    if control && key == "p" {
        return Route::Previous;
    }
    if !modified {
        match key {
            "down" => return Route::Next,
            "up" => return Route::Previous,
            "enter"
                if enter
                    || matches!(
                        presentation,
                        Presentation::Picker | Presentation::ContextMenu
                    ) =>
            {
                return Route::Accept;
            }
            "tab"
                if tab
                    || matches!(
                        presentation,
                        Presentation::Picker | Presentation::ContextMenu
                    ) =>
            {
                return if shift {
                    Route::Previous
                } else {
                    Route::Accept
                };
            }
            _ => {}
        }
    }
    if matches!(
        presentation,
        Presentation::Picker | Presentation::ContextMenu
    ) {
        Route::Block
    } else {
        Route::Pass
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signature_avoids_completion_and_documentation_or_stays_hidden() {
        let view = Rect {
            x: 0.,
            y: 0.,
            width: 1100.,
            height: 600.,
        };
        let caret = Rect {
            x: 120.,
            y: 290.,
            width: 10.,
            height: 22.,
        };
        let completion = place(view, Some(caret), (450., 220.), true);
        let occupied: Vec<_> = std::iter::once(completion.menu)
            .chain(completion.documentation)
            .collect();
        let signature = assistance(view, caret, &occupied, 80.).unwrap();
        assert!(signature.bottom() < caret.y);
        assert!(signature.x >= 8. && signature.right() <= view.right() - 8.);
        assert!(assistance(view, caret, &[view], 80.).is_none());
    }
    #[test]
    fn menu_flips_and_clamps_at_view_edges() {
        let view = Rect {
            x: 250.,
            y: 32.,
            width: 600.,
            height: 400.,
        };
        let p = place(
            view,
            Some(Rect {
                x: 820.,
                y: 390.,
                width: 10.,
                height: 25.,
            }),
            (400., 220.),
            true,
        );
        assert!(p.above);
        assert!(p.menu.x >= 258. && p.menu.right() <= 842.);
        assert!(p.menu.bottom() < 390.);
    }
    #[test]
    fn tiny_and_scaled_viewports_never_overflow() {
        for scale in [0.75, 1., 1.5, 2.] {
            let v = Rect {
                x: 0.,
                y: 0.,
                width: 240. * scale,
                height: 180. * scale,
            };
            let p = place(
                v,
                Some(Rect {
                    x: 120. * scale,
                    y: 90. * scale,
                    width: 8.,
                    height: 20.,
                }),
                (480., 240.),
                true,
            );
            assert!(p.menu.right() <= v.right());
            assert!(p.menu.bottom() <= v.bottom());
            assert!(p.documentation.is_none());
        }
    }
    #[test]
    fn documentation_uses_available_side_and_centered_picker_is_bounded() {
        let v = Rect {
            x: 0.,
            y: 0.,
            width: 1200.,
            height: 700.,
        };
        let p = place(
            v,
            Some(Rect {
                x: 20.,
                y: 100.,
                width: 10.,
                height: 25.,
            }),
            (400., 220.),
            true,
        );
        assert!(p.documentation.unwrap().x > p.menu.right());
        let p = place(v, None, (720., 400.), false);
        assert_eq!(p.menu.x, 240.);
    }
    #[test]
    fn modal_keys_and_configured_acceptance_do_not_leak() {
        assert_eq!(
            route(Presentation::Picker, "i", false, false, false, true, true),
            Route::Block
        );
        assert_eq!(
            route(
                Presentation::Completion,
                "a",
                false,
                false,
                false,
                true,
                true
            ),
            Route::Pass
        );
        assert_eq!(
            route(
                Presentation::Completion,
                "enter",
                false,
                false,
                false,
                false,
                true
            ),
            Route::Pass
        );
        assert_eq!(
            route(Presentation::Completion, "n", true, true, false, true, true),
            Route::Next
        );
        assert_eq!(
            route(
                Presentation::Completion,
                "tab",
                false,
                false,
                true,
                true,
                true
            ),
            Route::Previous
        );
    }
}

//! Native overlay for the existing language-service owner. No second LSP client.
//!
//! Layout: header (identity, scope/trust state) → search and filters → project
//! maintenance → catalog | details → operation/confirmation → shortcut strip.
use super::*;
use crate::{
    lsp_catalog::{self, CATALOG},
    lsp_manager,
    lsp_manager::{Change, Operation},
};

#[derive(Clone, Copy, PartialEq, Default)]
pub(super) enum Filter {
    #[default]
    All,
    Installed,
    Available,
    Updates,
}
#[derive(Default)]
pub(super) struct ManagerUi {
    pub open: bool,
    query: String,
    selected: usize,
    filter: Filter,
    workspace: bool,
    confirmation: Option<Operation>,
    scroll: ScrollHandle,
}
#[derive(Clone)]
enum Action {
    Refresh,
    Updates,
    Install,
    Remove,
    Rollback,
    RepairRuntime,
    Cleanup,
    Enable,
    Select,
    Start,
    Restart,
    Stop,
    Scope,
    Master,
    Custom,
    Source,
    Downloads,
    Trust,
    Confirm,
    Cancel,
}
impl EditorView {
    pub(super) fn open_manager(&mut self, cx: &mut Context<Self>) {
        self.dismiss_popup();
        self.manager_ui.open = true;
        self.manager_ui.workspace = true;
        let paths = self
            .model
            .order
            .iter()
            .filter_map(|id| self.model.get(*id).and_then(|e| e.buffer.path.clone()))
            .collect();
        self.services.manager.set_workspace(&self.services.root);
        self.services.manager.observe(paths);
        cx.notify();
    }
    pub(super) fn manager_ids(&self) -> Vec<&'static lsp_catalog::Entry> {
        let query = self.manager_ui.query.to_lowercase();
        CATALOG
            .iter()
            .filter(|e| {
                let matches = format!("{} {} {}", e.name, e.id, e.languages.join(" "))
                    .to_lowercase()
                    .contains(&query);
                let status = self.services.manager.snapshot.statuses.get(e.id);
                matches
                    && match self.manager_ui.filter {
                        Filter::All => true,
                        Filter::Installed => status.is_some_and(|s| s.launch.is_some()),
                        Filter::Available => status.is_none_or(|s| s.launch.is_none()),
                        Filter::Updates => status.is_some_and(|s| s.update_available()),
                    }
            })
            .collect()
    }
    pub(super) fn manager_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let k = &event.keystroke;
        if self.manager_ui.confirmation.is_some() {
            match k.key.as_str() {
                "escape" => self.manager_action(Action::Cancel, window, cx),
                "enter" => self.manager_action(Action::Confirm, window, cx),
                _ => {}
            }
        } else {
            let count = self.manager_ids().len();
            match k.key.as_str() {
                "escape" => self.manager_ui.open = false,
                "up" => self.manager_ui.selected = self.manager_ui.selected.saturating_sub(1),
                "down" => {
                    self.manager_ui.selected =
                        (self.manager_ui.selected + 1).min(count.saturating_sub(1))
                }
                "backspace" => {
                    self.manager_ui.query.pop();
                    self.manager_ui.selected = 0;
                }
                "f1" | "f2" | "f3" | "f4" => {
                    self.manager_ui.filter = match k.key.as_str() {
                        "f2" => Filter::Installed,
                        "f3" => Filter::Available,
                        "f4" => Filter::Updates,
                        _ => Filter::All,
                    };
                    self.manager_ui.selected = 0;
                }
                "f5" => self.manager_action(Action::Refresh, window, cx),
                "f6" => self.manager_action(Action::Updates, window, cx),
                "f7" => self.manager_action(Action::Custom, window, cx),
                "enter" => self.manager_action(Action::Install, window, cx),
                _ if k.modifiers.control => match k.key.as_str() {
                    "s" => self.manager_action(Action::Scope, window, cx),
                    "r" => self.manager_action(Action::Restart, window, cx),
                    "i" => self.manager_action(Action::Install, window, cx),
                    "u" => self.manager_action(Action::Remove, window, cx),
                    "b" => self.manager_action(Action::Rollback, window, cx),
                    "e" => self.manager_action(Action::Enable, window, cx),
                    "p" => self.manager_action(Action::Select, window, cx),
                    "g" => self.manager_action(Action::Start, window, cx),
                    "x" => self.manager_action(Action::Stop, window, cx),
                    "d" => self.manager_action(Action::Master, window, cx),
                    "t" => self.manager_action(Action::Trust, window, cx),
                    _ => {}
                },
                _ if !k.modifiers.alt && !k.modifiers.platform => {
                    if let Some(text) = &k.key_char {
                        if text.chars().all(|c| !c.is_control())
                            && self.manager_ui.query.len() < 128
                        {
                            self.manager_ui.query.push_str(text);
                            self.manager_ui.selected = 0;
                        }
                    } else if k.key == "space" {
                        self.manager_ui.query.push(' ');
                        self.manager_ui.selected = 0;
                    }
                }
                _ => {}
            }
        }
        cx.stop_propagation();
        window.prevent_default();
        cx.notify();
    }
    fn manager_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        let entry = self.manager_ids().get(self.manager_ui.selected).copied();
        if matches!(action, Action::Cancel) {
            self.manager_ui.confirmation = None;
            self.services.manager.cancel();
            cx.notify();
            return;
        }
        if matches!(action, Action::Scope) {
            self.manager_ui.workspace = !self.manager_ui.workspace;
            cx.notify();
            return;
        }
        if self.services.manager.busy {
            return;
        }
        let operation = match action {
            Action::Refresh => Some(Operation::Refresh),
            Action::Updates => Some(Operation::CheckUpdates),
            Action::RepairRuntime => {
                self.manager_ui.confirmation = Some(Operation::RepairRuntime);
                None
            }
            Action::Cleanup => {
                self.manager_ui.confirmation = Some(Operation::Cleanup);
                None
            }
            Action::Trust => {
                self.manager_ui.confirmation = Some(Operation::TrustWorkspace(
                    !self.services.manager.snapshot.trusted,
                ));
                None
            }
            Action::Master if !self.model.editor.config.lsp.enabled => {
                self.setting("lsp.enabled".into(), "true".into(), cx);
                Some(Operation::Configure {
                    workspace: self.manager_ui.workspace,
                    change: Change::All(false),
                })
            }
            Action::Master => Some(Operation::Configure {
                workspace: self.manager_ui.workspace,
                change: Change::All(self.services.manager.preferences().all_disabled != Some(true)),
            }),
            Action::Confirm => self.manager_ui.confirmation.take(),
            _ => {
                let Some(entry) = entry else {
                    return;
                };
                let id = entry.id.to_owned();
                match action {
                    Action::Install => Some(Operation::Install(id)),
                    Action::Source => {
                        cx.open_url(entry.source);
                        None
                    }
                    Action::Downloads => {
                        cx.open_url(&entry.download_page());
                        None
                    }
                    Action::Remove => {
                        self.manager_ui.confirmation = Some(Operation::Remove(id));
                        None
                    }
                    Action::Rollback
                        if self
                            .services
                            .manager
                            .snapshot
                            .store
                            .previous
                            .contains_key(&id) =>
                    {
                        self.manager_ui.confirmation = Some(Operation::Rollback(id));
                        None
                    }
                    Action::Enable => Some(Operation::Configure {
                        workspace: self.manager_ui.workspace,
                        change: if !entry.complementary
                            && !lsp_manager::selected(entry, &self.services.manager.preferences())
                        {
                            Change::Select(id)
                        } else {
                            Change::Enable(
                                id,
                                !lsp_manager::selected(entry, &self.services.manager.preferences()),
                            )
                        },
                    }),
                    Action::Select => Some(Operation::Configure {
                        workspace: self.manager_ui.workspace,
                        change: Change::Select(id),
                    }),
                    Action::Start | Action::Restart | Action::Stop => {
                        self.services.control(
                            entry.id,
                            !matches!(action, Action::Stop),
                            matches!(action, Action::Restart),
                        );
                        self.services.update(&self.model);
                        None
                    }
                    Action::Custom => {
                        let paths = cx.prompt_for_paths(PathPromptOptions {
                            files: true,
                            directories: false,
                            multiple: false,
                            prompt: Some("Choose language-server executable".into()),
                        });
                        let local = self.manager_ui.workspace;
                        cx.spawn(async move |view, cx| {
                            if let Ok(Ok(Some(paths))) = paths.await
                                && let Some(path) = paths.into_iter().next()
                            {
                                let _ = view.update(cx, |v, cx| {
                                    v.services.manager.submit(Operation::Configure {
                                        workspace: local,
                                        change: Change::Custom(
                                            id,
                                            crate::lsp_manager::Launch {
                                                command: path,
                                                args: entry
                                                    .args
                                                    .iter()
                                                    .map(|s| s.to_string())
                                                    .collect(),
                                            },
                                        ),
                                    });
                                    cx.notify();
                                });
                            }
                        })
                        .detach();
                        None
                    }
                    _ => None,
                }
            }
        };
        if let Some(operation) = operation {
            if let Operation::Remove(id) = &operation {
                self.services.control(id, false, false);
            }
            self.services.manager.submit(operation);
        }
        let _ = window;
        cx.notify();
    }
    fn manager_button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        action: Action,
        enabled: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        self.manager_toned(id, label, action, Tone::Secondary, enabled, cx)
    }
    fn manager_toned(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        action: Action,
        tone: Tone,
        enabled: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        design::button(&self.theme, self.scale(), id, label, tone, enabled, false)
            .when(enabled, |d| {
                d.on_click(cx.listener(move |v, _, w, cx| v.manager_action(action.clone(), w, cx)))
            })
            .into_any_element()
    }
    /// Server state as a status color: running, attention, idle or unavailable.
    fn manager_state(&self, entry: &lsp_catalog::Entry) -> (&'static str, Hsla) {
        let theme = self.theme;
        let status = self.services.manager.snapshot.statuses.get(entry.id);
        if let Some(runtime) = self.services.runtime(entry.id).first() {
            let color = match runtime.state {
                "Running" => theme.success,
                "Failed" => theme.error,
                "Starting" | "Retry scheduled" => theme.warning,
                _ => theme.secondary,
            };
            return (runtime.state, color);
        }
        if self.services.paused(entry.id) {
            ("Stopped", theme.secondary)
        } else if status.is_some_and(|s| s.launch.is_some()) {
            ("Installed, inactive", theme.info)
        } else if status.is_none() {
            if self.services.manager.loaded {
                ("Status unavailable", theme.muted)
            } else {
                ("Checking installation", theme.muted)
            }
        } else if entry.supports_host() {
            ("Not installed", theme.muted)
        } else {
            ("Unsupported platform", theme.muted)
        }
    }
    pub(super) fn manager_popup(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<(AnyElement, Point<Pixels>)> {
        let theme = self.theme;
        let scale = self.scale();
        let viewport = window.viewport_size();
        let width = (f32::from(viewport.width) - space::XXL * 2.).clamp(1., 1040.);
        let height = (f32::from(viewport.height) - space::XXL * 2.).clamp(1., 700.);
        let origin = point(
            (viewport.width - px(width)) / 2.,
            (viewport.height - px(height)) / 2.,
        );
        let entries = self.manager_ids();
        self.manager_ui.selected = self
            .manager_ui
            .selected
            .min(entries.len().saturating_sub(1));
        let prefs = self.services.manager.preferences();
        let busy = self.services.manager.busy;
        let trusted = self.services.manager.snapshot.trusted;
        let rows = entries
            .iter()
            .enumerate()
            .map(|(index, e)| {
                let status = self.services.manager.snapshot.statuses.get(e.id);
                let (state, color) = self.manager_state(e);
                let selected = index == self.manager_ui.selected;
                let installing = busy
                    && matches!(&self.services.manager.operation, Some(Operation::Install(id)) if id == e.id);
                let mut meta = vec![state];
                meta.push(if lsp_manager::selected(e, &prefs) {
                    "enabled"
                } else {
                    "disabled"
                });
                if status.is_some_and(|s| s.update_available()) {
                    meta.push("update available");
                }
                if installing {
                    meta.push("installing…");
                }
                design::row(&theme, ("lsp-entry", index), selected)
                    .flex_shrink_0()
                    .py(px(space::SM))
                    .items_start()
                    .text_color(if selected { theme.text } else { theme.secondary })
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.manager_ui.selected = index;
                        v.manager_ui.scroll.set_offset(point(px(0.), px(0.)));
                        cx.notify();
                    }))
                    .child(
                        div()
                            .pt(px(space::XXS))
                            .child(design::icon(icons::DOT, scale.micro, color)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.))
                            .gap(px(space::XXS))
                            .child(
                                div()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(e.name),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_size(px(scale.micro))
                                    .text_color(theme.muted)
                                    .child(meta.join(" · ")),
                            ),
                    )
                    .when(e.recommended, |d| {
                        d.child(design::badge("rec", theme.accent, scale.micro))
                    })
            })
            .collect::<Vec<_>>();
        // Content is a non-shrinking column inside the scroll container, so its
        // sections keep their height when banners reduce the visible area.
        let mut details = div()
            .flex()
            .flex_none()
            .flex_col()
            .gap(px(space::XL))
            .p(px(space::XL));
        if let Some(entry) = entries.get(self.manager_ui.selected) {
            let status = self
                .services
                .manager
                .snapshot
                .statuses
                .get(entry.id)
                .cloned()
                .unwrap_or_default();
            let installed = status.launch.is_some();
            let enabled = lsp_manager::selected(entry, &prefs);
            let previous = self.services.manager.snapshot.store.previous.get(entry.id);
            let (state, state_color) = self.manager_state(entry);
            let banner = |glyph: &'static str, color: Hsla, text: String| {
                div()
                    .flex()
                    .gap(px(space::SM))
                    .p(px(space::MD))
                    .rounded(px(design::radius::CONTROL))
                    .bg(color.opacity(0.08))
                    .border_1()
                    .border_color(color.opacity(0.3))
                    .text_size(px(scale.small))
                    .text_color(theme.text)
                    .child(design::icon(glyph, scale.small, color))
                    .child(div().flex_1().min_w(px(0.)).child(text))
            };
            let field = |key: &'static str, value: String| {
                div()
                    .flex()
                    .gap(px(space::LG))
                    .child(
                        div()
                            .flex_none()
                            .w(px(scale.small * 9.))
                            .text_color(theme.muted)
                            .child(key),
                    )
                    .child(div().flex_1().min_w(px(0.)).child(value))
            };
            let mut actions = div()
                .flex()
                .flex_wrap()
                .gap(px(space::SM))
                .child(self.manager_toned(
                    "lsp-install",
                    if status.managed {
                        "Update / reinstall"
                    } else {
                        "Install managed copy"
                    },
                    Action::Install,
                    if installed {
                        Tone::Secondary
                    } else {
                        Tone::Primary
                    },
                    !busy && entry.supports_host(),
                    cx,
                ))
                .child(self.manager_button(
                    "lsp-enable",
                    if enabled { "Disable" } else { "Enable" },
                    Action::Enable,
                    !busy,
                    cx,
                ));
            if !entry.complementary {
                actions = actions.child(self.manager_button(
                    "lsp-select",
                    "Use as primary",
                    Action::Select,
                    !busy,
                    cx,
                ));
            }
            actions = actions
                .child(self.manager_button(
                    "lsp-rollback",
                    "Roll back",
                    Action::Rollback,
                    previous.is_some() && !busy,
                    cx,
                ))
                .child(self.manager_button(
                    "lsp-custom",
                    "Custom executable…",
                    Action::Custom,
                    !busy,
                    cx,
                ))
                .child(
                    self.manager_toned(
                        "lsp-uninstall",
                        "Uninstall",
                        Action::Remove,
                        Tone::Danger,
                        self.services
                            .manager
                            .snapshot
                            .store
                            .installed
                            .contains_key(entry.id)
                            && !busy,
                        cx,
                    ),
                );
            details = details
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(space::SM))
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(space::SM))
                                .child(
                                    div()
                                        .text_size(px(scale.ui + 2.))
                                        .text_color(theme.text)
                                        .child(entry.name),
                                )
                                .child(design::badge(state, state_color, scale.micro))
                                .child(design::badge(
                                    if entry.recommended {
                                        "recommended"
                                    } else {
                                        "alternative"
                                    },
                                    if entry.recommended {
                                        theme.accent
                                    } else {
                                        theme.secondary
                                    },
                                    scale.micro,
                                ))
                                .child(design::badge(
                                    if enabled { "enabled" } else { "disabled" },
                                    if enabled {
                                        theme.success
                                    } else {
                                        theme.secondary
                                    },
                                    scale.micro,
                                ))
                                .when(status.update_available(), |d| {
                                    d.child(design::badge(
                                        "update available",
                                        theme.warning,
                                        scale.micro,
                                    ))
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(scale.small))
                                .text_color(theme.secondary)
                                .child(entry.notes),
                        ),
                )
                .child(actions);
            if let Some(warning) = &status.dependency_warning {
                details = details.child(banner(icons::WARNING, theme.warning, warning.clone()));
            }
            if let Some(warning) = &status.warning {
                details = details.child(banner(icons::WARNING, theme.warning, warning.clone()));
            }
            let runtimes = self.services.runtime(entry.id);
            let mut runtime = div().flex().flex_col().gap(px(space::MD)).child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(design::section(&theme, scale, "Runtime"))
                    .child(
                        div()
                            .flex()
                            .gap(px(space::SM))
                            .child(self.manager_button(
                                "lsp-start",
                                "Start",
                                Action::Start,
                                installed && !busy,
                                cx,
                            ))
                            .child(self.manager_button(
                                "lsp-restart",
                                "Restart",
                                Action::Restart,
                                installed && !busy,
                                cx,
                            ))
                            .child(self.manager_button(
                                "lsp-stop",
                                "Stop",
                                Action::Stop,
                                !busy,
                                cx,
                            )),
                    ),
            );
            if runtimes.is_empty() {
                runtime = runtime.child(
                    div()
                        .text_size(px(scale.small))
                        .text_color(theme.muted)
                        .child("No instance. Open a matching file; only selected, enabled servers start on demand."),
                );
            }
            for info in runtimes {
                let color = match info.state {
                    "Running" => theme.success,
                    "Failed" => theme.error,
                    "Starting" | "Retry scheduled" => theme.warning,
                    _ => theme.secondary,
                };
                runtime = runtime.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(space::SM))
                        .p(px(space::MD))
                        .rounded(px(design::radius::CONTROL))
                        .border_1()
                        .border_color(theme.hairline)
                        .bg(theme.hover)
                        .text_size(px(scale.small))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::SM))
                                .child(design::badge(info.state, color, scale.micro))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .text_color(theme.text)
                                        .child(design::display_path(&info.root, &self.explorer.root)),
                                )
                                .child(div().text_color(theme.muted).child(
                                    info.startup.map_or_else(
                                        || "initializing".into(),
                                        |d| format!("ready in {} ms", d.as_millis()),
                                    ),
                                )),
                        )
                        .child(
                            div()
                                .text_color(theme.secondary)
                                .child(format!("{} {:?}", info.command, info.args)),
                        )
                        .when_some(info.failure, |d, failure| {
                            d.child(div().text_color(theme.error).child(failure))
                        })
                        .child(
                            div()
                                .text_size(px(scale.micro))
                                .text_color(theme.muted)
                                .child(format!(
                                    "Advertised capabilities (not a promise of client support):\n{}\nRecent stderr (bounded 32 KiB; refresh to inspect):\n{}",
                                    info.capabilities, info.stderr
                                )),
                        ),
                );
            }
            details = details.child(runtime);
            let mut facts = div()
                .flex()
                .flex_col()
                .gap(px(space::XS))
                .text_size(px(scale.small))
                .text_color(theme.text)
                .child(design::section(&theme, scale, "Details"))
                .child(field("Languages", entry.languages.join(", ")))
                .child(field("Files", entry.extensions.join(", ")))
                .child(field("Root markers", entry.roots.join(", ")))
                .child(field("Dependencies", entry.runtime.to_string()))
                .child(field("Installation", format!("{:?}", entry.method)))
                .child(field(
                    "Platforms",
                    format!(
                        "{} / {}",
                        entry.platforms.join(", "),
                        entry.architectures.join(", ")
                    ),
                ))
                .child(field(
                    "Installed",
                    status
                        .version
                        .clone()
                        .unwrap_or_else(|| "Unknown / not installed".into()),
                ))
                .child(field(
                    "Latest stable",
                    status
                        .available
                        .clone()
                        .unwrap_or_else(|| "Not checked · use Check updates".into()),
                ))
                .child(field(
                    "Ownership",
                    if status.managed {
                        "Filavim-managed".into()
                    } else {
                        "External / unavailable (never removed)".into()
                    },
                ));
            if let Some(previous) = previous {
                facts = facts.child(field(
                    "Previous",
                    format!("{} · Ctrl-B to roll back", previous.version),
                ));
            }
            if let Some(origin) = status.origin {
                use lsp_manager::LaunchOrigin;
                facts = facts.child(field(
                    "Launch source",
                    match origin {
                        LaunchOrigin::Managed => "Filavim-managed generation",
                        LaunchOrigin::Configured => "Explicitly configured executable",
                        LaunchOrigin::Workspace => "Workspace-provided executable",
                        LaunchOrigin::System => "System executable from PATH",
                    }
                    .into(),
                ));
            }
            if let Some(launch) = status.launch {
                facts = facts
                    .child(field("Executable", launch.command.display().to_string()))
                    .child(field("Arguments", format!("{:?}", launch.args)));
            }
            facts = facts
                .child(field("Source", entry.source.to_string()))
                .child(
                    div()
                        .flex()
                        .gap(px(space::SM))
                        .pt(px(space::SM))
                        .child(self.manager_toned(
                            "lsp-source",
                            "Project page",
                            Action::Source,
                            Tone::Ghost,
                            true,
                            cx,
                        ))
                        .child(self.manager_toned(
                            "lsp-downloads",
                            "Downloads",
                            Action::Downloads,
                            Tone::Ghost,
                            true,
                            cx,
                        )),
                );
            details = details.child(facts);
        } else {
            details = details.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .text_color(theme.muted)
                    .child(design::icon(icons::SEARCH, scale.ui, theme.muted))
                    .child("No matching servers."),
            );
        }
        let filters = [
            ("All", "F1", Filter::All),
            ("Installed", "F2", Filter::Installed),
            ("Available", "F3", Filter::Available),
            ("Updates", "F4", Filter::Updates),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (label, key, filter))| {
            let active = self.manager_ui.filter == filter;
            div()
                .id(("lsp-filter", i))
                .flex()
                .items_center()
                .gap(px(space::XS))
                .h(px(scale.control() - space::XS - 2.))
                .px(px(space::MD))
                .rounded(px(design::radius::CONTROL - 1.))
                .text_size(px(scale.small))
                .cursor_pointer()
                .text_color(if active { theme.text } else { theme.secondary })
                .map(|d| {
                    if active {
                        d.bg(theme.pressed)
                    } else {
                        d.hover(|s| s.bg(theme.hover).text_color(theme.text))
                    }
                })
                .child(label)
                .child(
                    div()
                        .text_size(px(scale.micro))
                        .text_color(theme.muted)
                        .child(key),
                )
                .on_click(cx.listener(move |v, _, _, cx| {
                    v.manager_ui.filter = filter;
                    v.manager_ui.selected = 0;
                    cx.notify();
                }))
        })
        .collect::<Vec<_>>();
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(space::MD))
            .h(px(scale.title_bar()))
            .pl(px(space::XL))
            .pr(px(space::SM))
            .border_b_1()
            .border_color(theme.hairline)
            .child(design::heading(
                &theme,
                scale,
                icons::SERVER,
                theme.accent,
                "Language servers",
            ))
            .child(div().flex_1())
            .child(design::badge(
                if self.manager_ui.workspace {
                    "workspace scope"
                } else {
                    "global scope"
                },
                theme.secondary,
                scale.micro,
            ))
            .child(design::badge(
                if trusted {
                    "project trusted"
                } else {
                    "project untrusted"
                },
                if trusted {
                    theme.success
                } else {
                    theme.warning
                },
                scale.micro,
            ))
            .when(prefs.all_disabled == Some(true), |d| {
                d.child(design::badge("LSP disabled", theme.error, scale.micro))
            })
            .child(
                design::icon_button(
                    &theme,
                    "lsp-manager-close",
                    icons::CLOSE,
                    scale.ui,
                    scale.control(),
                )
                .on_click(cx.listener(|v, _, _, cx| {
                    v.manager_ui.open = false;
                    cx.notify();
                })),
            );
        let query = self.manager_ui.query.clone();
        let search = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(space::SM))
            .w(px((width * 0.3).clamp(160., 300.)))
            .h(px(scale.control()))
            .px(px(space::SM))
            .rounded(px(design::radius::CONTROL))
            .border_1()
            .border_color(theme.accent.opacity(0.5))
            .bg(theme.hover)
            .child(design::icon(icons::SEARCH, scale.small, theme.secondary))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .when(query.is_empty(), |d| {
                        d.child(div().text_color(theme.muted).child("Type to filter"))
                    })
                    .when(!query.is_empty(), |d| d.child(query))
                    .child(div().text_color(theme.accent).child("▏")),
            );
        let toolbar = div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(px(space::MD))
            .px(px(space::XL))
            .py(px(space::MD))
            .child(search)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::XXS))
                    .p(px(space::XXS))
                    .rounded(px(design::radius::CONTROL))
                    .border_1()
                    .border_color(theme.hairline)
                    .children(filters),
            )
            .child(div().flex_1())
            .child(self.manager_toned(
                "lsp-check-updates",
                "Check updates",
                Action::Updates,
                Tone::Ghost,
                !busy,
                cx,
            ))
            .child(self.manager_toned(
                "lsp-refresh",
                "Refresh",
                Action::Refresh,
                Tone::Ghost,
                !busy,
                cx,
            ));
        let maintenance = div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(px(space::SM))
            .px(px(space::XL))
            .pb(px(space::MD))
            .child(self.manager_toned(
                "lsp-trust",
                if trusted {
                    "Revoke project trust"
                } else {
                    "Trust project for LSP"
                },
                Action::Trust,
                if trusted {
                    Tone::Secondary
                } else {
                    Tone::Primary
                },
                !busy && self.services.manager.has_snapshot,
                cx,
            ))
            .child(self.manager_button(
                "lsp-scope",
                if self.manager_ui.workspace {
                    "Scope: workspace"
                } else {
                    "Scope: global"
                },
                Action::Scope,
                true,
                cx,
            ))
            .child(self.manager_button(
                "lsp-master",
                if prefs.all_disabled == Some(true) {
                    "Enable LSP"
                } else {
                    "Disable LSP"
                },
                Action::Master,
                !busy,
                cx,
            ))
            .child(self.manager_toned(
                "lsp-repair-runtime",
                "Repair Node runtime",
                Action::RepairRuntime,
                Tone::Ghost,
                !busy,
                cx,
            ))
            .child(self.manager_toned(
                "lsp-cleanup",
                "Clean unused tools",
                Action::Cleanup,
                Tone::Ghost,
                !busy,
                cx,
            ));
        let mut panel = design::typeface(
            design::glass(&theme, design::Elevation::Modal),
            &theme,
            scale.ui,
        )
        .id("lsp-manager")
        .flex()
        .flex_col()
        .w(px(width))
        .h(px(height))
        .overflow_hidden()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(header)
        .child(toolbar)
        .child(maintenance)
        .child(design::divider(&theme))
        .child(
            div()
                .flex()
                .flex_1()
                .min_h(px(0.))
                .child(
                    div()
                        .id("lsp-catalog")
                        .w(px((width * 0.34).min(320.)))
                        .flex_shrink_0()
                        .overflow_y_scroll()
                        .p(px(space::XS))
                        .border_r_1()
                        .border_color(theme.hairline)
                        .flex()
                        .flex_col()
                        .gap(px(space::XXS))
                        .children(rows),
                )
                .child(
                    div()
                        .id("lsp-details")
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_y_scroll()
                        .track_scroll(&self.manager_ui.scroll)
                        .child(details),
                ),
        );
        if busy {
            panel = panel.child(design::divider(&theme)).child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(space::MD))
                    .px(px(space::XL))
                    .py(px(space::MD))
                    .text_size(px(scale.small))
                    .child(design::icon(icons::SYNC, scale.small, theme.accent))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(self.services.manager.progress.clone()),
                    )
                    .child(self.manager_button(
                        "lsp-cancel",
                        "Cancel operation",
                        Action::Cancel,
                        true,
                        cx,
                    )),
            );
        }
        if let Some(error) = &self.services.manager.error {
            panel = panel.child(
                div()
                    .flex()
                    .flex_none()
                    .gap(px(space::SM))
                    .mx(px(space::XL))
                    .mb(px(space::MD))
                    .p(px(space::MD))
                    .max_h(px(96.))
                    .overflow_hidden()
                    .rounded(px(design::radius::CONTROL))
                    .bg(theme.error.opacity(0.08))
                    .border_1()
                    .border_color(theme.error.opacity(0.3))
                    .text_size(px(scale.small))
                    .child(design::icon(icons::ERROR, scale.small, theme.error))
                    .child(div().flex_1().min_w(px(0.)).child(error.clone())),
            );
        }
        if let Some(operation) = &self.manager_ui.confirmation {
            let message = match operation {
                Operation::RepairRuntime => "Download and validate a fresh managed Node runtime for installed npm servers? Existing server receipts are preserved if validation fails.".into(),
                Operation::Cleanup => "Remove unused owned generations? Active and rollback versions and leased generations are retained.".into(),
                Operation::Install(id) => format!(
                    "Install/update {id}? This downloads and runs executable software from its official source. npm lifecycle scripts and pip source builds are disabled. Only Filavim's private tools directory is changed; runtimes are not installed. Existing managed version stays active if validation fails."
                ),
                Operation::Remove(id) => format!(
                    "Remove Filavim-managed {id} and its retained generations? External/project executables remain untouched. Stop any other Filavim instance using it first."
                ),
                Operation::Rollback(id) => format!(
                    "Validate and activate the previous managed {id} version? The active version is preserved if validation fails."
                ),
                Operation::TrustWorkspace(true) => format!(
                    "Trust {} for language services? Servers may execute project tools, build scripts or plugins with your permissions.",
                    design::plain_path(&self.services.root)
                ),
                Operation::TrustWorkspace(false) => format!(
                    "Revoke language-server trust for {}? Running servers for this project will stop.",
                    design::plain_path(&self.services.root)
                ),
                _ => String::new(),
            };
            let destructive = matches!(operation, Operation::Remove(_) | Operation::Cleanup);
            panel = panel.child(
                div()
                    .flex()
                    .flex_none()
                    .flex_col()
                    .gap(px(space::MD))
                    .px(px(space::XL))
                    .py(px(space::LG))
                    .bg(theme.warning.opacity(0.06))
                    .border_t_1()
                    .border_color(theme.warning.opacity(0.6))
                    .child(
                        div()
                            .flex()
                            .gap(px(space::SM))
                            .child(design::icon(icons::WARNING, scale.ui, theme.warning))
                            .child(div().flex_1().min_w(px(0.)).child(message)),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(space::SM))
                            .child(self.manager_button(
                                "lsp-decline",
                                "Cancel · Esc",
                                Action::Cancel,
                                true,
                                cx,
                            ))
                            .child(self.manager_toned(
                                "lsp-confirm",
                                "Confirm · Enter",
                                Action::Confirm,
                                if destructive {
                                    Tone::Danger
                                } else {
                                    Tone::Primary
                                },
                                true,
                                cx,
                            )),
                    ),
            );
        }
        panel = panel.child(design::divider(&theme)).child(
            div()
                .flex_none()
                .px(px(space::XL))
                .py(px(space::SM))
                .child(design::hints(
                    &theme,
                    scale,
                    &[
                        ("↑ ↓", "select"),
                        ("Enter", "install"),
                        ("Ctrl E", "enable"),
                        ("Ctrl P", "primary"),
                        ("Ctrl G R X", "start restart stop"),
                        ("Ctrl U", "uninstall"),
                        ("Ctrl B", "roll back"),
                        ("Ctrl T", "trust"),
                        ("Ctrl S", "scope"),
                        ("Ctrl D", "all LSP"),
                        ("F5 F6 F7", "refresh updates path"),
                        ("Esc", "close"),
                    ],
                )),
        );
        let backdrop = design::scrim(&theme, viewport)
            .id("lsp-manager-backdrop")
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .into_any_element();
        vec![
            (backdrop, point(px(0.), px(0.))),
            (panel.into_any_element(), origin),
        ]
    }
    /// Inline notice above the status bar for missing language tools.
    pub(super) fn manager_notice(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if self.manager_ui.open
            || self.services.manager.busy
            || self.services.manager.snapshot.missing.is_empty()
        {
            return None;
        }
        let theme = self.theme;
        let scale = self.scale();
        let ids = self
            .services
            .manager
            .snapshot
            .missing
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let message = format!("Language tools unavailable: {}", ids.join(", "));
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(space::MD))
                .h(px(scale.status_bar()))
                .px(px(space::LG))
                .bg(theme.warning.opacity(0.08))
                .border_t_1()
                .border_color(theme.warning.opacity(0.35))
                .text_size(px(scale.small))
                .text_color(theme.text)
                .child(design::icon(icons::PACKAGE, scale.small, theme.warning))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(message),
                )
                .child(
                    design::button(
                        &theme,
                        scale,
                        "lsp-missing-manage",
                        "Manage…",
                        Tone::Primary,
                        true,
                        false,
                    )
                    .h(px(scale.status_bar() - space::MD))
                    .on_click(cx.listener(|v, _, _, cx| v.open_manager(cx))),
                )
                .child(
                    design::button(
                        &theme,
                        scale,
                        "lsp-missing-dismiss",
                        "Dismiss",
                        Tone::Ghost,
                        true,
                        false,
                    )
                    .h(px(scale.status_bar() - space::MD))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        for id in &ids {
                            v.services.manager.snapshot.missing.remove(id);
                        }
                        v.services.manager.submit(Operation::Configure {
                            workspace: true,
                            change: Change::Dismiss(ids.clone()),
                        });
                        cx.notify();
                    })),
                )
                .into_any_element(),
        )
    }
}

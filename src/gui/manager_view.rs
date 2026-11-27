//! Native overlay for the existing language-service owner. No second LSP client.
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
    Enable,
    Select,
    Start,
    Restart,
    Stop,
    Scope,
    Master,
    Custom,
    Source,
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
                    "e" => self.manager_action(Action::Enable, window, cx),
                    "p" => self.manager_action(Action::Select, window, cx),
                    "g" => self.manager_action(Action::Start, window, cx),
                    "x" => self.manager_action(Action::Stop, window, cx),
                    "d" => self.manager_action(Action::Master, window, cx),
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
                    Action::Remove => {
                        self.manager_ui.confirmation = Some(Operation::Remove(id));
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
        div()
            .id(id)
            .px(px(8.))
            .py(px(4.))
            .border_1()
            .border_color(self.theme.border)
            .text_color(if enabled {
                self.theme.text
            } else {
                self.theme.muted
            })
            .when(enabled, |d| {
                d.cursor_pointer()
                    .hover(|d| d.bg(self.theme.selection))
                    .on_click(
                        cx.listener(move |v, _, w, cx| v.manager_action(action.clone(), w, cx)),
                    )
            })
            .child(label.into())
            .into_any_element()
    }
    pub(super) fn manager_popup(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<(AnyElement, Point<Pixels>)> {
        let viewport = window.viewport_size();
        let width = (f32::from(viewport.width) - 24.).clamp(1., 1040.);
        let height = (f32::from(viewport.height) - 24.).clamp(1., 680.);
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
        let rows = entries
            .iter()
            .enumerate()
            .map(|(index, e)| {
                let status = self.services.manager.snapshot.statuses.get(e.id);
                let runtime = self.services.runtime(e.id);
                let state =
                    runtime
                        .first()
                        .map(|s| s.state)
                        .unwrap_or(if self.services.paused(e.id) {
                            "Stopped"
                        } else if status.is_some_and(|s| s.launch.is_some()) {
                            "Installed, inactive"
                        } else if status.is_none() {
                            if self.services.manager.loaded {"Status unavailable"} else {"Checking installation"}
                        } else if e.supports_host() { "Not installed" } else { "Unsupported platform" });
                let selected = index == self.manager_ui.selected;
                div()
                    .id(("lsp-entry", index))
                    .p(px(8.))
                    .border_b_1()
                    .border_color(self.theme.border)
                    .bg(if selected {
                        self.theme.selection
                    } else {
                        self.theme.overlay
                    })
                    .cursor_pointer()
                    .hover(|d| d.bg(self.theme.selection))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.manager_ui.selected = index;
                        v.manager_ui.scroll.set_offset(point(px(0.), px(0.)));
                        cx.notify();
                    }))
                    .child(
                        div()
                            .text_color(if selected {
                                self.theme.text
                            } else {
                                self.theme.secondary
                            })
                            .child(e.name),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(self.theme.secondary)
                            .child(format!(
                                "{state}{}{}{}{}",
                                if e.recommended {
                                    " · recommended"
                                } else {
                                    " · alternative"
                                },
                                if lsp_manager::selected(e, &prefs) {
                                    " · enabled"
                                } else {
                                    " · disabled"
                                },
                                if status.is_some_and(|s|s.update_available()) {" · update available"}else{""},
                                if busy && matches!(&self.services.manager.operation,Some(Operation::Install(id)) if id==e.id) {" · installing/updating"}else{""}
                            )),
                    )
            })
            .collect::<Vec<_>>();
        let mut details = div()
            .id("lsp-details")
            .flex_1()
            .min_w(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.manager_ui.scroll)
            .p(px(12.));
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
            let mut actions = div()
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .child(self.manager_button(
                    "lsp-install",
                    if status.managed {
                        "Update / reinstall"
                    } else {
                        "Install managed copy"
                    },
                    Action::Install,
                    !busy && entry.supports_host(),
                    cx,
                ))
                .child(
                    self.manager_button(
                        "lsp-uninstall",
                        "Uninstall",
                        Action::Remove,
                        self.services
                            .manager
                            .snapshot
                            .store
                            .installed
                            .contains_key(entry.id)
                            && !busy,
                        cx,
                    ),
                )
                .child(self.manager_button(
                    "lsp-enable",
                    if lsp_manager::selected(entry, &prefs) {
                        "Disable"
                    } else {
                        "Enable"
                    },
                    Action::Enable,
                    !busy,
                    cx,
                ))
                .child(self.manager_button("lsp-source", "Project", Action::Source, true, cx))
                .child(self.manager_button(
                    "lsp-custom",
                    "Custom executable…",
                    Action::Custom,
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
            details=details.child(div().text_color(self.theme.accent).child(entry.name))
                .child(div().mt(px(8.)).child(entry.notes))
                .child(div().mt(px(8.)).text_color(self.theme.secondary).child(format!("Languages: {}\nFiles: {}\nRoot markers: {}\nDependencies: {}\nSource: {}\nInstallation: {:?}\nPlatforms: {} / {}\nInstalled version: {}\nLast checked stable version (cached): {}\nOwnership: {}",entry.languages.join(", "),entry.extensions.join(", "),entry.roots.join(", "),entry.runtime,entry.source,entry.method,entry.platforms.join(", "),entry.architectures.join(", "),status.version.as_deref().unwrap_or("Unknown / not installed"),status.available.as_deref().unwrap_or("Not checked · use Check updates"),if status.managed{"Filavim-managed"}else{"External / unavailable (never removed)"})))
                .child(div().mt(px(10.)).child(actions));
            if let Some(warning) = &status.dependency_warning {
                details = details.child(
                    div()
                        .mt(px(8.))
                        .text_color(self.theme.warning)
                        .child(warning.clone()),
                );
            }
            if let Some(warning) = &status.warning {
                details = details.child(
                    div()
                        .mt(px(8.))
                        .text_color(self.theme.warning)
                        .child(warning.clone()),
                );
            }
            if let Some(launch) = status.launch {
                details = details.child(div().mt(px(8.)).child(format!(
                    "Resolved executable: {}\nArguments: {:?}",
                    launch.command.display(),
                    launch.args
                )));
            }
            details = details.child(
                div()
                    .mt(px(10.))
                    .flex()
                    .gap(px(6.))
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
                    .child(self.manager_button("lsp-stop", "Stop", Action::Stop, !busy, cx)),
            );
            let runtimes = self.services.runtime(entry.id);
            if runtimes.is_empty() {
                details=details.child(div().mt(px(8.)).text_color(self.theme.secondary).child("No instance. Open a matching file; only selected, enabled servers start on demand."));
            }
            for runtime in runtimes {
                details = details.child(div().mt(px(12.)).child(format!(
                    "{} · {}\n{} {:?}\nInitialize time: {}",
                    runtime.state,
                    runtime.root.display(),
                    runtime.command,
                    runtime.args,
                    runtime.startup.map_or_else(
                        || "Not completed".into(),
                        |d| format!("{} ms", d.as_millis())
                    )
                )));
                if let Some(failure) = runtime.failure {
                    details = details.child(div().text_color(self.theme.error).child(failure));
                }
                details=details.child(div().mt(px(6.)).text_size(px(13.)).text_color(self.theme.secondary).child(format!("Advertised capabilities (not a promise of client support):\n{}\nRecent stderr (bounded 32 KiB; refresh to inspect):\n{}",runtime.capabilities,runtime.stderr)));
            }
        } else {
            details = details.child("No matching servers.");
        }
        let filters = [
            ("All", Filter::All),
            ("Installed", Filter::Installed),
            ("Available", Filter::Available),
            ("Updates", Filter::Updates),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (label, filter))| {
            div()
                .id(("lsp-filter", i))
                .px(px(8.))
                .py(px(4.))
                .bg(if self.manager_ui.filter == filter {
                    self.theme.selection
                } else {
                    self.theme.overlay
                })
                .cursor_pointer()
                .child(label)
                .on_click(cx.listener(move |v, _, _, cx| {
                    v.manager_ui.filter = filter;
                    v.manager_ui.selected = 0;
                    cx.notify();
                }))
        })
        .collect::<Vec<_>>();
        let mut panel = div()
            .id("lsp-manager")
            .flex()
            .flex_col()
            .w(px(width))
            .h(px(height))
            .overflow_hidden()
            .border_1()
            .border_color(self.theme.border)
            .bg(self.theme.overlay)
            .text_color(self.theme.text)
            .font_family(FONT_FAMILY)
            .text_size(px(self.model.editor.config.font_size.min(17.)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p(px(10.))
                    .border_b_1()
                    .border_color(self.theme.border)
                    .child("Language servers")
                    .child(
                        div()
                            .id("lsp-manager-close")
                            .cursor_pointer()
                            .px(px(6.))
                            .child("×")
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.manager_ui.open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .px(px(10.))
                    .py(px(6.))
                    .text_color(self.theme.secondary)
                    .child(format!(
                        "Search: {}▏ · ↑↓ select · Esc close · F1–F4 filters · F5 refresh",
                        self.manager_ui.query
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(4.))
                    .px(px(8.))
                    .children(filters)
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
                        "lsp-check-updates",
                        "Check updates",
                        Action::Updates,
                        !busy,
                        cx,
                    ))
                    .child(self.manager_button(
                        "lsp-refresh",
                        "Refresh",
                        Action::Refresh,
                        !busy,
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
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.))
                    .mt(px(8.))
                    .child(
                        div()
                            .id("lsp-catalog")
                            .w(px((width * 0.34).min(320.)))
                            .flex_shrink_0()
                            .overflow_y_scroll()
                            .border_r_1()
                            .border_color(self.theme.border)
                            .children(rows),
                    )
                    .child(details),
            );
        panel=panel.child(div().px(px(8.)).py(px(4.)).text_size(px(12.)).text_color(self.theme.secondary).child("Ctrl-I install · Ctrl-U uninstall · Ctrl-E enable · Ctrl-P primary · Ctrl-R restart · Ctrl-G start · Ctrl-X stop · Ctrl-S scope · F6 updates · F7 path"));
        if busy {
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p(px(8.))
                    .child(self.services.manager.progress.clone())
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
                    .p(px(8.))
                    .max_h(px(90.))
                    .overflow_hidden()
                    .text_color(self.theme.error)
                    .child(error.clone()),
            );
        }
        if let Some(operation) = &self.manager_ui.confirmation {
            let message = match operation {
                Operation::Install(id) => format!(
                    "Install/update {id}? This downloads and runs executable software from its official source. npm lifecycle scripts and pip source builds are disabled. Only Filavim's private tools directory is changed; runtimes are not installed. Existing managed version stays active if validation fails."
                ),
                Operation::Remove(id) => format!(
                    "Remove Filavim-managed {id} and its retained generations? External/project executables remain untouched. Stop any other Filavim instance using it first."
                ),
                _ => String::new(),
            };
            panel = panel.child(
                div()
                    .p(px(10.))
                    .border_t_1()
                    .border_color(self.theme.warning)
                    .child(message)
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .mt(px(8.))
                            .child(self.manager_button(
                                "lsp-confirm",
                                "Confirm · Enter",
                                Action::Confirm,
                                true,
                                cx,
                            ))
                            .child(self.manager_button(
                                "lsp-decline",
                                "Cancel · Esc",
                                Action::Cancel,
                                true,
                                cx,
                            )),
                    ),
            );
        }
        let backdrop = div()
            .id("lsp-manager-backdrop")
            .absolute()
            .size_full()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .into_any_element();
        vec![
            (backdrop, point(px(0.), px(0.))),
            (panel.into_any_element(), origin),
        ]
    }
    pub(super) fn manager_notice(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if self.manager_ui.open
            || self.services.manager.busy
            || self.services.manager.snapshot.missing.is_empty()
        {
            return None;
        }
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
                .flex_shrink_0()
                .items_center()
                .gap(px(10.))
                .px(px(12.))
                .bg(self.theme.surface)
                .text_size(px(14.))
                .text_color(self.theme.secondary)
                .child(
                    div()
                        .flex_1()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(message),
                )
                .child(
                    div()
                        .id("lsp-missing-manage")
                        .cursor_pointer()
                        .text_color(self.theme.accent)
                        .child("Manage…")
                        .on_click(cx.listener(|v, _, _, cx| v.open_manager(cx))),
                )
                .child(
                    div()
                        .id("lsp-missing-dismiss")
                        .cursor_pointer()
                        .child("Dismiss")
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

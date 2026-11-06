//! The Servers page's Plugins tab, for a server whose engine loads plugins from its `plugins/` folder (a
//! Paper-style engine on Horizon): each plugin with its version, Add plugin…, Reload now, Turn off or on and
//! Delete. Everything goes through `horizon-server plugins`, so it works the same for a server on another
//! computer (over SSH, the library goes on ssh's stdin).
use super::*;

impl Launcher {
    /// Whether the open server has the Plugins tab: its `plugins/` folder exists.
    pub(super) fn has_plugins(&self) -> bool {
        self.servers.plugins.as_ref().is_some_and(|p| p.folder)
    }

    pub(super) fn plugins_tab(&mut self, status: &Status, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let idle = self.servers.busy.is_none();
        let running = status.running();
        let Some(listing) = self.servers.plugins.clone() else {
            return div();
        };
        let add = {
            let b = button("plugin-add", "Add plugin…", Kind::Primary, &t);
            if idle {
                let library = listing.library.clone();
                b.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.choose_plugins(library.clone(), cx);
                }))
            } else {
                disabled(b)
            }
        };
        let mut tab = div().flex().flex_col().gap(px(20.)).child(
            div().flex().items_center().gap(px(16.)).child(add).child(
                text(theme::CAPTION).text_color(t.muted).child(trf(
                    "A plugin is a .{library} file. The server loads a new or changed one while \
                         it runs.",
                    &[("library", &listing.library)],
                )),
            ),
        );
        if listing.plugins.is_empty() {
            return tab.child(
                text(theme::UI)
                    .text_color(t.muted)
                    .child(tr("No plugins yet.")),
            );
        }
        let mut list = div().flex().flex_col();
        for (i, p) in listing.plugins.into_iter().enumerate() {
            let confirming = self.servers.confirm_remove_plugin.as_deref() == Some(p.file.as_str());
            let mut right = div().flex().items_center().gap(px(8.));
            if p.enabled && running && !confirming {
                right = right.child(self.plugin_button(
                    format!("plugin-reload-{i}"),
                    "Reload now",
                    Kind::Ghost,
                    idle,
                    vec!["reload".into(), p.file.clone()],
                    trf("Reloaded {name}.", &[("name", &p.name)]),
                    cx,
                ));
            }
            if !confirming {
                let (label, act, done) = if p.enabled {
                    (
                        "Turn off",
                        "off",
                        trf("{name} is off.", &[("name", &p.name)]),
                    )
                } else {
                    ("Turn on", "on", trf("{name} is on.", &[("name", &p.name)]))
                };
                right = right.child(self.plugin_button(
                    format!("plugin-toggle-{i}"),
                    label,
                    Kind::Ghost,
                    idle,
                    vec![act.into(), p.file.clone()],
                    done,
                    cx,
                ));
            }
            let delete = if confirming {
                self.plugin_button(
                    format!("plugin-delete-{i}"),
                    "Delete it",
                    Kind::Primary,
                    idle,
                    vec!["remove".into(), p.file.clone()],
                    trf("Deleted {name}.", &[("name", &p.name)]),
                    cx,
                )
            } else {
                let file = p.file.clone();
                let b = button(
                    SharedString::from(format!("plugin-delete-{i}")),
                    "Delete",
                    Kind::Ghost,
                    &t,
                );
                if idle {
                    b.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.servers.confirm_remove_plugin = Some(file.clone());
                        cx.notify();
                    }))
                } else {
                    disabled(b)
                }
            };
            right = right.child(delete);
            if confirming {
                right = right.child(
                    button("plugin-delete-cancel", "Cancel", Kind::Ghost, &t).on_click(
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.servers.confirm_remove_plugin = None;
                            cx.notify();
                        }),
                    ),
                );
            }
            let mut detail = vec![p.version.clone().unwrap_or_default(), bytes(p.bytes)];
            if !p.enabled {
                detail.insert(0, tr("Off").to_string());
            }
            detail.retain(|d| !d.is_empty());
            list = list.child(self.list_row(
                None,
                p.name.clone(),
                Some(detail.join(" · ").into_any_element()),
                Some(right.into_any_element()),
            ));
        }
        tab = tab.child(list);
        tab
    }

    /// A button that runs `horizon-server plugins <args>` on the open server and says `done`.
    #[allow(clippy::too_many_arguments)]
    fn plugin_button(
        &self,
        id: String,
        label: &'static str,
        kind: Kind,
        idle: bool,
        args: Vec<String>,
        done: String,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let b = button(SharedString::from(id), label, kind, &self.theme);
        if !idle {
            return disabled(b);
        }
        b.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            this.servers.confirm_remove_plugin = None;
            let Some(dir) = this.servers.open.clone() else {
                return;
            };
            let (args, done) = (args.clone(), done.clone());
            this.server_job(tr("Plugins").to_string(), move |_| {
                let mut all = vec!["plugins"];
                all.extend(args.iter().map(String::as_str));
                servers::run(&dir, &all).map(|_| done)
            });
            cx.notify();
        }))
    }

    /// Asks for plugin files, then adds them one after another.
    fn choose_plugins(&mut self, library: String, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(tr("Add").into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                let _ = this.update(cx, |this, cx| {
                    this.add_plugins(paths, library);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn add_plugins(&mut self, paths: Vec<PathBuf>, library: String) {
        let Some(dir) = self.servers.open.clone() else {
            return;
        };
        let suffix = format!(".{library}");
        if let Some(wrong) = paths
            .iter()
            .find(|p| !p.to_string_lossy().to_lowercase().ends_with(&suffix))
        {
            self.servers.problem = Some(Problem::plain(trf(
                "{file} isn't a plugin for this server: it takes .{library} files.",
                &[
                    (
                        "file",
                        &wrong.file_name().unwrap_or_default().to_string_lossy(),
                    ),
                    ("library", &library),
                ],
            )));
            return;
        }
        self.server_job(tr("Adding plugins").to_string(), move |log| {
            let mut added = Vec::new();
            for path in &paths {
                log(servers::add_plugin(&dir, path)?);
                added.push(
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            Ok(trf(
                "Added {files}. The server loads plugins while it runs.",
                &[("files", &added.join(", "))],
            )
            .to_string())
        });
    }
}

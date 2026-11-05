//! The Add a server on another computer dialog: the computer (`user@host`, or a `Host` from
//! `~/.ssh/config`) and the server's folder there. Add connects over SSH and asks that server for
//! its status before the server joins the list (`horizon_launcher::servers::remote`).
use super::*;
use crate::glass_ui as ui;
use horizon_launcher::servers::remote;
use horizon_theme::glass::{ink, radius};

/// The dialog's state while it is open.
#[derive(Default)]
pub(super) struct RemoteAdd {
    pub(super) checking: bool,
    pub(super) error: Option<String>,
}

impl Launcher {
    pub(super) fn open_remote_add(&mut self, cx: &mut Context<Self>) {
        self.servers.remote_add = Some(RemoteAdd::default());
        if self.servers.value(Field::RemoteDir).is_empty() {
            self.servers
                .text(Field::RemoteDir)
                .set("/srv/minecraft".into());
        }
        self.focus_handle(Field::RemoteHost, cx);
        self.servers.focus_soon = Some(Field::RemoteHost);
    }

    /// Checks the connection and the server there, then adds it (`Event::RemoteChecked`).
    pub(super) fn add_remote(&mut self) {
        if self.servers.remote_add.as_ref().is_none_or(|a| a.checking) {
            return;
        }
        let host = self.servers.value(Field::RemoteHost).trim().to_string();
        let dir = self.servers.value(Field::RemoteDir).trim().to_string();
        let Some(add) = &mut self.servers.remote_add else {
            return;
        };
        if let Err(e) = remote::check_host(&host) {
            add.error = Some(e);
            return;
        }
        add.checking = true;
        add.error = None;
        let tx = self.servers.tx.clone();
        std::thread::spawn(move || {
            let result = remote::check(&host, &dir).and_then(|_| {
                let name = std::path::Path::new(&dir)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| host.clone());
                let entry = Entry {
                    name,
                    dir: dir.into(),
                    host: Some(host),
                };
                servers::add(entry.clone()).map(|()| entry)
            });
            let _ = tx.send(Event::RemoteChecked(result));
        });
    }

    pub(super) fn remote_add_dialog(&mut self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let t = self.theme;
        let (checking, error) = {
            let add = self.servers.remote_add.as_ref()?;
            (add.checking, add.error.clone())
        };
        let look = self.look;
        let close = cx.listener(move |this, _: &MouseDownEvent, _, cx| {
            if !checking {
                this.servers.remote_add = None;
            }
            cx.notify();
        });
        let label = |text: &str| {
            ui::text(14., FontWeight::MEDIUM, ink::FULL)
                .mb(px(6.))
                .child(text.to_string())
        };
        let host = self.field(Field::RemoteHost, "user@address", 504., cx);
        let dir = self.field(Field::RemoteDir, "/srv/minecraft", 504., cx);
        let status = if checking {
            Some(ui::card_note(tr("Connecting…")))
        } else {
            error.map(|e| {
                ui::text(13., FontWeight::NORMAL, ink::FULL)
                    .text_color(ui::danger())
                    .child(e)
            })
        };
        let cancel = ui::quiet("remote-cancel", tr("Cancel"), 44., &t);
        let accent = ui::color(look.accent);
        let add = keyboard(
            div()
                .id("remote-add")
                .flex()
                .items_center()
                .justify_center()
                .h(px(44.))
                .px(px(24.))
                .rounded(px(radius::LARGE))
                .bg(accent)
                .cursor_pointer()
                .hover(|s| s.opacity(0.92))
                .font_family(horizon_theme::glass::FONT)
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ui::color(look.on_accent))
                .child(tr("Add server")),
            &t,
        );
        let buttons = div().flex().justify_end().gap(px(8.)).map(|d| {
            if checking {
                d.opacity(0.5).child(cancel).child(add)
            } else {
                d.child(cancel.on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.servers.remote_add = None;
                    cx.notify();
                })))
                .child(add.on_click(cx.listener(
                    |this, _: &ClickEvent, _, cx| {
                        this.add_remote();
                        cx.notify();
                    },
                )))
            }
        });
        let card = ui::dialog_plate(radius::MODAL)
            .id("remote-card")
            .occlude()
            .flex()
            .flex_col()
            .gap(px(20.))
            .w(px(560.))
            .p(px(28.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        ui::text(22., FontWeight::SEMIBOLD, ink::FULL)
                            .font_family(horizon_theme::glass::HEADING_FONT)
                            .child(tr("Add a server on another computer")),
                    )
                    .child(ui::card_note(tr(
                        "A server set up with horizon-server on a VPS or a home server. The launcher signs in with your SSH key and never asks for or keeps a password.",
                    ))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(label(tr("Computer")))
                    .child(host)
                    .child(
                        ui::card_note(tr("user@address, or a Host name from your ~/.ssh/config"))
                            .mt(px(6.)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(label(tr("Server folder there")))
                    .child(dir),
            )
            .children(status)
            .child(buttons);
        Some(
            div()
                .id("remote-dialog")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .p(px(24.))
                .occlude()
                .child(ui::scrim_backdrop())
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .bg(ui::scrim())
                        .on_mouse_down(gpui::MouseButton::Left, close),
                )
                .child(card),
        )
    }
}

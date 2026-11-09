//! Updates in the app (spec `app-updates`, design D8; ADR-0124..0126): the update question (a card in onboarding,
//! a one-time banner after an upgrade), the notice banner, the status-bar item, "Check for Updates…" and the
//! Settings > General > Updates group.
//!
//! [`UpdateService`] does the work off the UI thread. [`UpdateModel`] keeps its latest [`UpdateStatus`] snapshot
//! (ADR-0019: one coalesced snapshot, no per-file events) and runs its ticks on the app's executor clock, so
//! headless tests can idle for days of app time. Nothing here opens a connection: the service decides, and it
//! sends nothing while the mode is undecided or off (ADR-0029).

use std::time::Instant;

use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ps_model::settings::UpdateMode;
use ps_service::updates::{
    Available, Busy, FailureReason, Kind, ModeCell, Notice, Outcome, TICK, UpdateService,
    UpdateStatus, now_secs,
};
use ui_theme::Tokens;

use crate::assets::IconName;
use crate::settings::AppSettings;
use crate::shell::{Shell, ToastKind};
use crate::views::kit::{self, Tone};

/// The text of the update question (onboarding card and banner), word for word.
pub const QUESTION_TITLE: &str = "Keep PhotoSquirrel up to date";
pub const QUESTION_BODY: &str = "Once a day PhotoSquirrel asks GitHub whether a newer version exists. It sends nothing \
     about you or your photos. Updates are checked for the PhotoSquirrel signature before they are installed, and \
     install when you quit.";
/// After this many closes without an answer, the banner stops asking and updates are off (design D1).
pub const MAX_PROMPT_DISMISSALS: u32 = 3;

/// The app's update model, when this build has one (`main` installs it; tests may).
#[derive(Clone)]
pub struct UpdateHub(pub Entity<UpdateModel>);

impl Global for UpdateHub {}

impl UpdateHub {
    pub fn model(cx: &App) -> Option<Entity<UpdateModel>> {
        cx.try_global::<UpdateHub>().map(|h| h.0.clone())
    }
}

pub struct UpdateModel {
    service: UpdateService,
    mode: ModeCell,
    status: UpdateStatus,
    /// Wall-clock seconds and the executor's instant at start: see [`UpdateModel::now`].
    started: (i64, Instant),
    /// The update question's banner was closed this session.
    question_closed: bool,
    /// The result of the last check or action the user started, shown in Settings.
    result: Option<String>,
    /// The command that installs a saved package (`.deb`), with Copy.
    command: Option<String>,
    /// A check or action the user started is running.
    working: bool,
}

/// Install the update model: ticks every [`TICK`] of app time, follows the saved update mode, and hands a ready
/// update to the installer when the app quits. The caller does not start the service's own thread.
pub fn install(service: UpdateService, mode: ModeCell, cx: &mut App) {
    mode.set(AppSettings::get(cx).updates.mode);
    service.refresh(now_secs());
    let started = (now_secs(), cx.background_executor().now());
    let model = cx.new(|_| UpdateModel {
        status: service.status(),
        service: service.clone(),
        mode,
        started,
        question_closed: false,
        result: None,
        command: None,
        working: false,
    });
    let followed = model.clone();
    cx.observe_global::<AppSettings>(move |cx| {
        let saved = AppSettings::get(cx).updates.mode;
        followed.update(cx, |m, cx| {
            if m.mode.get() != saved {
                m.mode.set(saved);
                let now = m.now(cx);
                m.service.refresh(now);
                m.reload(cx);
            }
        });
    })
    .detach();
    let weak = model.downgrade();
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(TICK).await;
            let Ok((service, now)) = weak.update(cx, |m, cx| (m.service.clone(), m.now(cx))) else {
                break;
            };
            cx.background_executor()
                .spawn(async move { service.tick(now) })
                .await;
            if weak.update(cx, |m, cx| m.reload(cx)).is_err() {
                break;
            }
        }
    })
    .detach();
    cx.on_app_quit(move |_| {
        // Starts the installer helper, which waits for this process to exit (ADR-0126).
        service.on_quit();
        async {}
    })
    .detach();
    cx.set_global(UpdateHub(model));
}

impl UpdateModel {
    pub fn status(&self) -> &UpdateStatus {
        &self.status
    }

    pub fn service(&self) -> &UpdateService {
        &self.service
    }

    /// The result of the last check or action the user started.
    pub fn result(&self) -> Option<&str> {
        self.result.as_deref()
    }

    pub fn working(&self) -> bool {
        self.working
    }

    /// Whether the update question is shown above the central view (an upgraded user, not yet answered).
    pub fn asks_in_banner(&self) -> bool {
        self.status.mode == UpdateMode::Undecided && !self.question_closed
    }

    /// Seconds since the epoch: the wall clock, or the executor's time since start when that is ahead (headless
    /// tests advance it by days; the wall clock wins after a laptop sleeps).
    pub fn now(&self, cx: &App) -> i64 {
        let elapsed = cx
            .background_executor()
            .now()
            .saturating_duration_since(self.started.1);
        let app_time = self.started.0 + i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX / 2);
        now_secs().max(app_time)
    }

    fn reload(&mut self, cx: &mut Context<'_, Self>) {
        let status = self.service.status();
        if status != self.status {
            self.status = status;
            cx.notify();
        }
    }

    /// Run `work` on the background executor, then store its message (if any) and the new status.
    fn run(
        &mut self,
        work: impl FnOnce(UpdateService) -> Option<String> + Send + 'static,
        cx: &mut Context<'_, Self>,
    ) -> Task<Option<String>> {
        self.working = true;
        cx.notify();
        let service = self.service.clone();
        let job = cx.background_executor().spawn(async move { work(service) });
        cx.spawn(async move |this, cx| {
            let message = job.await;
            let _ = this.update(cx, |m, cx| {
                m.working = false;
                if message.is_some() {
                    m.result = message.clone();
                }
                m.reload(cx);
                cx.notify();
            });
            message
        })
    }

    /// "Check for updates": one check now, whatever the mode. Resolves to the message to show.
    pub fn check_now(&mut self, cx: &mut Context<'_, Self>) -> Task<Option<String>> {
        let installed = self.status.installed.clone();
        self.run(
            move |s| {
                Some(match s.check_now() {
                    Ok(Outcome::UpToDate) => format!("PhotoSquirrel {installed} is up to date."),
                    Ok(Outcome::Available(m)) => {
                        format!("PhotoSquirrel {} is available. You have {installed}.", m.version)
                    }
                    Err(e) => format!(
                        "Couldn't check for updates: {e}. You're still on PhotoSquirrel {installed}."
                    ),
                })
            },
            cx,
        )
    }

    /// Answer the update question: "Keep me up to date" (automatic) or "No thanks" (off). Saved at once.
    pub fn answer(&mut self, keep: bool, cx: &mut Context<'_, Self>) {
        let mode = if keep {
            UpdateMode::Automatic
        } else {
            UpdateMode::Off
        };
        let _ = AppSettings::update(cx, |s| s.updates.set_mode(mode));
        cx.notify();
    }

    /// The question's banner was closed without an answer: asked again next start, at most three times in all.
    pub fn close_question(&mut self, cx: &mut Context<'_, Self>) {
        self.question_closed = true;
        let _ = AppSettings::update(cx, |s| {
            s.updates.prompt_dismissals = s.updates.prompt_dismissals.saturating_add(1);
            if s.updates.prompt_dismissals >= MAX_PROMPT_DISMISSALS {
                s.updates.set_mode(UpdateMode::Off);
            }
        });
        cx.notify();
    }

    /// Hide the current notice for its version.
    pub fn dismiss(&mut self, cx: &mut Context<'_, Self>) {
        self.service.dismiss();
        self.reload(cx);
    }

    /// "Skip this version".
    pub fn skip(&mut self, version: &str, cx: &mut Context<'_, Self>) {
        self.service.skip(version);
        self.reload(cx);
    }

    /// "Install on quit" (notify mode): download now, install when the app quits.
    pub fn install_on_quit(&mut self, cx: &mut Context<'_, Self>) -> Task<Option<String>> {
        self.run(
            |s| {
                s.install_on_quit()
                    .err()
                    .map(|e| format!("Couldn't download the update: {e}."))
            },
            cx,
        )
    }

    /// "Download" for a `.deb`: save the verified package in Downloads, reveal it and show the command.
    pub fn save_for_manual_install(&mut self, cx: &mut Context<'_, Self>) -> Task<Option<String>> {
        let task = self.run(
            |s| {
                Some(match s.save_for_manual_install() {
                    Ok(m) => match m.command {
                        Some(c) => format!(
                            "Saved and verified {}. Install it with: {c}",
                            m.file.display()
                        ),
                        None => format!("Saved and verified {}.", m.file.display()),
                    },
                    Err(e) => format!("Couldn't download the update: {e}."),
                })
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let message = task.await;
            let command = message
                .as_deref()
                .and_then(|m| m.split_once("Install it with: "))
                .map(|(_, c)| c.to_owned());
            let _ = this.update(cx, |m, cx| {
                m.command = command;
                cx.notify();
            });
            message
        })
    }

    pub fn command(&self) -> Option<&str> {
        self.command.as_deref()
    }
}

/// The release page of `version`.
pub fn release_page(version: &str) -> String {
    format!("https://github.com/szehnder/photosquirrel2/releases/tag/v{version}")
}

/// "3 minutes ago", "5 hours ago", "2 days ago".
pub fn ago(at: i64, now: i64) -> String {
    let s = (now - at).max(0);
    let (n, unit) = match s {
        0..60 => return "just now".into(),
        60..3600 => (s / 60, "minute"),
        3600..86_400 => (s / 3600, "hour"),
        _ => (s / 86_400, "day"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

// ---- the shell's part --------------------------------------------------------------------------------------

impl Shell {
    /// "Check for Updates…" from the palette: one check, the result as a toast.
    pub fn check_for_updates(&mut self, cx: &mut Context<'_, Self>) {
        let Some(model) = UpdateHub::model(cx) else {
            self.show_toast(
                ToastKind::Info,
                "Updates",
                "This build does not check for updates.",
                true,
                cx,
            );
            return;
        };
        let task = model.update(cx, |m, cx| m.check_now(cx));
        cx.spawn(async move |this, cx| {
            let message = task.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.show_toast(ToastKind::Info, "Check for updates", message, true, cx);
            });
        })
        .detach();
    }

    /// "Restart now" (or "Go back to"): hand the install to the helper, then quit as the Quit command does.
    pub fn restart_for_update(
        &mut self,
        go_back: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(model) = UpdateHub::model(cx) else {
            return;
        };
        let service = model.read(cx).service.clone();
        let job = cx.background_executor().spawn(async move {
            if go_back {
                service.go_back()
            } else {
                service.restart_now()
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    this.save_now(window, cx);
                    cx.quit();
                }
                Err(e) => {
                    this.show_toast(ToastKind::Warning, "Not restarted", e.to_string(), true, cx)
                }
            });
        })
        .detach();
    }
}

fn notice_body(a: &Available) -> String {
    // The first lines of the notes; What's new opens the rest.
    let mut body: String = a
        .notes
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join(" ");
    if body.chars().count() > 240 {
        body = body.chars().take(239).collect::<String>() + "…";
    }
    body
}

fn frame(
    id: &'static str,
    icon: IconName,
    urgent: bool,
    children: Vec<AnyElement>,
    cx: &App,
) -> AnyElement {
    let colors = *ui_theme::colors(cx);
    h_flex()
        .id(id)
        .flex_none()
        .flex_wrap()
        .mx(Tokens::space(3.0))
        .mt(Tokens::space(3.0))
        .px(Tokens::space(3.0))
        .py(Tokens::space(2.0))
        .gap(Tokens::space(3.0))
        .rounded(Tokens::RADIUS_LG)
        .bg(colors.accent_soft)
        .border_1()
        .border_color(if urgent {
            colors.warning
        } else {
            colors.accent.opacity(0.4)
        })
        .child(
            Icon::new(icon)
                .size(Tokens::icon_md())
                .text_color(colors.accent_text),
        )
        .children(children)
        .test_support()
        .into_any_element()
}

fn texts(title_id: &'static str, title: String, body: Option<String>, cx: &App) -> Div {
    let colors = *ui_theme::colors(cx);
    v_flex()
        .flex_1()
        .min_w(Tokens::size(240.0))
        .child(
            div()
                .id(title_id)
                .test_support()
                .aria_label(SharedString::from(title.clone()))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text)
                .child(title),
        )
        .children(body.filter(|b| !b.is_empty()).map(|b| {
            div()
                .text_size(Tokens::text_sm())
                .text_color(colors.text_muted)
                .child(b)
        }))
}

fn close(id: &'static str, on_click: kit::ClickHandler, cx: &App) -> AnyElement {
    let colors = *ui_theme::colors(cx);
    div()
        .id(id)
        .test_support()
        .role(Role::Button)
        .aria_label("Dismiss")
        .cursor_pointer()
        .text_color(colors.text_faint)
        .hover(|d| d.text_color(colors.text))
        .on_click(move |e, w, cx| on_click(e, w, cx))
        .child(Icon::new(IconName::X).size(Tokens::icon_sm()))
        .into_any_element()
}

/// The two answers to the update question; "Keep me up to date" is the preselected primary action.
fn answers(model: &Entity<UpdateModel>, prefix: &'static str, cx: &App) -> Div {
    let (yes, no) = (model.clone(), model.clone());
    h_flex()
        .gap(Tokens::space(2.0))
        .child(kit::button(
            format!("{prefix}-yes"),
            "Keep me up to date",
            Some(IconName::CircleArrowUp),
            Tone::Primary,
            kit::on(move |_, _, cx| yes.update(cx, |m, cx| m.answer(true, cx))),
            cx,
        ))
        .child(kit::button(
            format!("{prefix}-no"),
            "No thanks",
            None,
            Tone::Secondary,
            kit::on(move |_, _, cx| no.update(cx, |m, cx| m.answer(false, cx))),
            cx,
        ))
}

/// The update question in the onboarding view, until it is answered.
pub(crate) fn question_card(cx: &App) -> Option<AnyElement> {
    let model = UpdateHub::model(cx)?;
    if model.read(cx).status.mode != UpdateMode::Undecided {
        return None;
    }
    let colors = *ui_theme::colors(cx);
    Some(
        v_flex()
            .id("updates-question")
            .test_support()
            .w(Tokens::prose_width())
            .max_w_full()
            .gap(Tokens::space(3.0))
            .p(Tokens::space(4.0))
            .rounded(Tokens::RADIUS_LG)
            .bg(colors.panel)
            .border_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .gap(Tokens::space(2.0))
                    .child(
                        Icon::new(IconName::CircleArrowUp)
                            .size(Tokens::icon_md())
                            .text_color(colors.accent_text),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text)
                            .child(QUESTION_TITLE),
                    ),
            )
            .child(
                div()
                    .text_size(Tokens::text_sm())
                    .line_height(relative(1.5))
                    .text_color(colors.text_muted)
                    .child(QUESTION_BODY),
            )
            .child(answers(&model, "updates-question", cx))
            .into_any_element(),
    )
}

/// The banner above the central view: the update question for an upgraded user, else the current notice.
pub(crate) fn banner(shell: &Shell, cx: &mut Context<'_, Shell>) -> Option<AnyElement> {
    let model = UpdateHub::model(cx)?;
    let m = model.read(cx);
    if m.asks_in_banner() {
        // A first launch asks in the onboarding view instead.
        if shell.shows_onboarding() || shell.summary().ready().is_none() {
            return None;
        }
        let closer = model.clone();
        return Some(frame(
            "updates-question-banner",
            IconName::CircleArrowUp,
            false,
            vec![
                texts(
                    "updates-question-banner-title",
                    QUESTION_TITLE.into(),
                    Some(QUESTION_BODY.into()),
                    cx,
                )
                .into_any_element(),
                answers(&model, "updates-question-banner", cx).into_any_element(),
                close(
                    "updates-question-banner-close",
                    std::rc::Rc::new(move |_, _, cx| {
                        closer.update(cx, |m, cx| m.close_question(cx))
                    }),
                    cx,
                ),
            ],
            cx,
        ));
    }
    let notice = m.status.notice.clone()?;
    let status = m.status.clone();
    let shell_entity = cx.entity().downgrade();
    let mut actions: Vec<AnyElement> = Vec::new();
    let notes = |version: &str, cx: &App| {
        let link = release_page(version);
        kit::button(
            "update-whats-new",
            "What's new",
            Some(IconName::ExternalLink),
            Tone::Ghost,
            kit::on(move |_, _, cx| cx.open_url(&link)),
            cx,
        )
    };
    let dismiss = {
        let model = model.clone();
        std::rc::Rc::new(move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            model.update(cx, |m, cx| m.dismiss(cx))
        }) as kit::ClickHandler
    };
    let (icon, title, body) = match &notice {
        Notice::Failed(f) => {
            actions.push(kit::button(
                "update-details",
                "Details",
                None,
                Tone::Secondary,
                kit::on(|_, w, cx| w.dispatch_action(Box::new(crate::actions::OpenSettings), cx)),
                cx,
            ));
            let body = match f.reason {
                FailureReason::StartKept => None,
                _ => Some(format!(
                    "{} Details are in Settings > General > Updates.",
                    f.message
                )),
            };
            (IconName::TriangleAlert, notice.text(), body)
        }
        Notice::Updated(v) => {
            actions.push(notes(v, cx));
            (IconName::CircleCheck, notice.text(), None)
        }
        Notice::Ready(a) => {
            let blocked = status.restart_blocked_by.or_else(|| m.service.blocked_by());
            let label = match blocked {
                Some(op) => format!("Available when {op} finishes"),
                None => "Restart now".into(),
            };
            actions.push(kit::button(
                "update-restart",
                label,
                Some(IconName::RotateCcw),
                Tone::Primary,
                if blocked.is_none() {
                    let shell = shell_entity.clone();
                    kit::on(move |_, w, cx| {
                        if let Some(shell) = shell.upgrade() {
                            shell.update(cx, |s, cx| s.restart_for_update(false, w, cx));
                        }
                    })
                } else {
                    None
                },
                cx,
            ));
            actions.push(notes(&a.version, cx));
            let title = if a.urgent {
                format!("Important update. {}", notice.text())
            } else {
                notice.text()
            };
            (IconName::CircleArrowUp, title, Some(notice_body(a)))
        }
        Notice::Available(a) => {
            let (install, version) = (model.clone(), a.version.clone());
            actions.push(kit::button(
                "update-install-on-quit",
                "Install on quit",
                Some(IconName::Download),
                Tone::Primary,
                (!m.working).then(|| {
                    std::rc::Rc::new(move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        install.update(cx, |m, cx| m.install_on_quit(cx)).detach();
                    }) as kit::ClickHandler
                }),
                cx,
            ));
            actions.push(notes(&a.version, cx));
            let skipper = model.clone();
            actions.push(kit::button(
                "update-skip",
                "Skip this version",
                None,
                Tone::Ghost,
                kit::on(move |_, _, cx| skipper.update(cx, |m, cx| m.skip(&version, cx))),
                cx,
            ));
            let title = if a.urgent {
                format!("Important update. {}", notice.text())
            } else {
                notice.text()
            };
            (IconName::CircleArrowUp, title, Some(notice_body(a)))
        }
        Notice::Manual(a) => {
            if status.kind == Kind::Deb {
                let saver = model.clone();
                actions.push(kit::button(
                    "update-download",
                    if m.working {
                        "Downloading…"
                    } else {
                        "Download"
                    },
                    Some(IconName::Download),
                    Tone::Primary,
                    (!m.working).then(|| {
                        std::rc::Rc::new(move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                            saver
                                .update(cx, |m, cx| m.save_for_manual_install(cx))
                                .detach();
                        }) as kit::ClickHandler
                    }),
                    cx,
                ));
                if let Some(command) = m.command.clone() {
                    actions.push(kit::button(
                        "update-copy-command",
                        "Copy command",
                        Some(IconName::Copy),
                        Tone::Secondary,
                        kit::on(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(command.clone()))
                        }),
                        cx,
                    ));
                }
            } else {
                let link = release_page(&a.version);
                actions.push(kit::button(
                    "update-release-page",
                    "Release page",
                    Some(IconName::ExternalLink),
                    Tone::Primary,
                    kit::on(move |_, _, cx| cx.open_url(&link)),
                    cx,
                ));
            }
            let body = match (&m.result, &status.cannot_install) {
                (Some(r), _) if m.command.is_some() => r.clone(),
                (_, Some(why)) => {
                    format!("{why}: this copy is updated by hand. {}", notice_body(a))
                }
                _ => notice_body(a),
            };
            (IconName::CircleArrowUp, notice.text(), Some(body))
        }
    };
    let urgent =
        matches!(&notice, Notice::Ready(a) | Notice::Available(a) | Notice::Manual(a) if a.urgent);
    Some(frame(
        "update-banner",
        icon,
        urgent,
        vec![
            texts("update-banner-title", title, body, cx).into_any_element(),
            h_flex()
                .flex_wrap()
                .gap(Tokens::space(2.0))
                .children(actions)
                .into_any_element(),
            close("update-dismiss", dismiss, cx),
        ],
        cx,
    ))
}

/// The status-bar item: a ready update, a download, or failing scheduled checks.
pub(crate) fn status_item(cx: &App) -> Option<AnyElement> {
    let model = UpdateHub::model(cx)?;
    let s = model.read(cx).status.clone();
    let colors = *ui_theme::colors(cx);
    let (text, icon): (String, IconName) = if let Some(v) = &s.ready {
        (format!("Update ready: {v}"), IconName::CircleArrowUp)
    } else if s.busy == Some(Busy::Downloading) {
        ("Downloading update…".into(), IconName::Download)
    } else if s.failing {
        ("Update checks failing".into(), IconName::TriangleAlert)
    } else {
        return None;
    };
    Some(
        h_flex()
            .id("status-update")
            .test_support()
            .aria_label(SharedString::from(text.clone()))
            .gap(Tokens::space(1.5))
            .items_center()
            .text_size(Tokens::text_xs())
            .text_color(colors.accent_text)
            .cursor_pointer()
            .on_click(|_, w, cx| w.dispatch_action(Box::new(crate::actions::OpenSettings), cx))
            .child(Icon::new(icon).size(Tokens::icon_sm()))
            .child(text)
            .into_any_element(),
    )
}

// ---- Settings > General > Updates ----------------------------------------------------------------------------

/// The Updates group of Settings > General: version, installation, the last check, "Check for updates" and,
/// when a previous version is kept, "Go back to <version>". The mode itself is the `updates.mode` setting row.
pub(crate) fn settings_group(cx: &App) -> Option<AnyElement> {
    let model = UpdateHub::model(cx)?;
    let m = model.read(cx);
    let s = m.status.clone();
    let colors = *ui_theme::colors(cx);
    let now = m.now(cx);
    let row = |id: &'static str, label: &'static str, value: String| {
        h_flex()
            .id(id)
            .test_support()
            .aria_label(SharedString::from(format!("{label}: {value}")))
            .w_full()
            .gap(Tokens::space(3.0))
            .text_size(Tokens::text_sm())
            .child(
                div()
                    .w(Tokens::size(140.0))
                    .flex_none()
                    .text_color(colors.text_muted)
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(colors.text)
                    .child(value),
            )
    };
    let install = match &s.cannot_install {
        None => format!("{}, updates itself", s.kind.label()),
        Some(why) => format!("{}; it does not update itself ({why})", s.kind.label()),
    };
    let last = match &s.last_check {
        Some(c) => format!("{}: {}", ago(c.at, now), c.message),
        None => "Never".into(),
    };
    let checker = model.clone();
    let check = kit::button(
        "settings-updates-check",
        if m.working {
            "Checking…"
        } else {
            "Check for updates"
        },
        Some(IconName::RefreshCw),
        Tone::Secondary,
        (!m.working).then(|| {
            std::rc::Rc::new(move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                checker.update(cx, |m, cx| m.check_now(cx)).detach();
            }) as kit::ClickHandler
        }),
        cx,
    );
    let go_back = s.kept.clone().map(|kept| {
        let blocked = m.service.blocked_by();
        kit::button(
            "settings-updates-go-back",
            format!("Go back to {kept}"),
            Some(IconName::Undo2),
            Tone::Secondary,
            blocked.is_none().then(|| {
                std::rc::Rc::new(move |_: &ClickEvent, w: &mut Window, cx: &mut App| {
                    if let Some(shell) = Shell::for_window(w, cx) {
                        shell.update(cx, |s, cx| s.restart_for_update(true, w, cx));
                    }
                }) as kit::ClickHandler
            }),
            cx,
        )
    });
    let history = m.service.history(5);
    Some(
        v_flex()
            .id("settings-updates")
            .test_support()
            .w_full()
            .gap(Tokens::space(2.0))
            .py(Tokens::space(3.0))
            .border_b_1()
            .border_color(colors.border)
            .child(crate::widgets::eyebrow("Updates", cx))
            .child(row(
                "settings-updates-version",
                "Version",
                format!("PhotoSquirrel {}", s.installed),
            ))
            .child(row(
                "settings-updates-mode",
                "Updates",
                s.mode.label().into(),
            ))
            .child(row(
                "settings-updates-installation",
                "Installed as",
                install,
            ))
            .child(row("settings-updates-last-check", "Last check", last))
            .when(s.failing, |d| {
                d.child(row(
                    "settings-updates-last-success",
                    "Last success",
                    s.last_success_at
                        .map_or_else(|| "Never".into(), |at| ago(at, now)),
                ))
            })
            .children(m.result.clone().map(|r| {
                div()
                    .id("settings-updates-result")
                    .test_support()
                    .aria_label(SharedString::from(r.clone()))
                    .text_size(Tokens::text_sm())
                    .text_color(colors.text)
                    .child(r)
            }))
            .child(
                h_flex()
                    .gap(Tokens::space(2.0))
                    .child(check)
                    .children(go_back),
            )
            .when(!history.is_empty(), |d| {
                d.child(
                    v_flex()
                        .id("settings-updates-history")
                        .test_support()
                        .gap(Tokens::space(0.5))
                        .text_size(Tokens::text_xs())
                        .text_color(colors.text_muted)
                        .children(history.into_iter().map(|h| {
                            format!(
                                "{} · {}{}",
                                ago(h.at, now),
                                h.version.map(|v| format!("{v}: ")).unwrap_or_default(),
                                h.detail
                            )
                        })),
                )
            })
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use ::core::prelude::v1::test;

    use super::*;

    #[test]
    fn ages_read_naturally() {
        assert_eq!(ago(100, 110), "just now");
        assert_eq!(ago(0, 60), "1 minute ago");
        assert_eq!(ago(0, 7_200), "2 hours ago");
        assert_eq!(ago(0, 3 * 86_400), "3 days ago");
        assert_eq!(ago(10, 0), "just now");
    }

    #[test]
    fn release_pages_name_the_tag() {
        assert_eq!(
            release_page("0.4.1"),
            "https://github.com/szehnder/photosquirrel2/releases/tag/v0.4.1"
        );
    }
}

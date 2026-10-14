//! Teach edit: on-board action editing and sync-controller teaching.
//!
//! Both panels share the same buttons and state machine; only the commands and a few
//! labels differ, so one [`Panel`] type drives both.

use crate::protocol::{Response, TeachStatus, cmd};
use crate::ui::state::{AppState, DeviceEvent};
use crate::ui::widgets::section;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, h_flex, v_flex};
use gpui_kit::*;

/// Commands and labels that distinguish the two panels.
struct Kind {
    id: &'static str,
    title: &'static str,
    enter_label: &'static str,
    exit_label: &'static str,
    clear_label: &'static str,
    /// Status shown while in the mode but neither recording nor playing.
    editing: &'static str,
    enter: u8,
    exit: u8,
    rec_start: u8,
    rec_stop: u8,
    play: u8,
    play_stop: u8,
    clear: u8,
    query: u8,
}

const ACTION: Kind = Kind {
    id: "ae",
    title: "grp_action_edit",
    enter_label: "btn_teach_enter",
    exit_label: "btn_teach_exit",
    clear_label: "btn_teach_clear",
    editing: "lbl_teach_editing",
    enter: cmd::ACTION_EDIT_ENTER,
    exit: cmd::ACTION_EDIT_EXIT,
    rec_start: cmd::ACTION_EDIT_START,
    rec_stop: cmd::ACTION_EDIT_STOP,
    play: cmd::ACTION_EDIT_PLAY,
    play_stop: cmd::ACTION_EDIT_PLAY_STOP,
    clear: cmd::ACTION_EDIT_CLEAR,
    query: cmd::ACTION_EDIT_QUERY,
};

const SYNC: Kind = Kind {
    id: "st",
    title: "grp_sync_teach",
    enter_label: "btn_sync_enter",
    exit_label: "btn_sync_exit",
    clear_label: "btn_sync_clear",
    editing: "lbl_sync_editing",
    enter: cmd::SYNC_TEACH_ENTER,
    exit: cmd::SYNC_TEACH_EXIT,
    rec_start: cmd::SYNC_TEACH_REC_START,
    rec_stop: cmd::SYNC_TEACH_REC_STOP,
    play: cmd::SYNC_TEACH_PLAY,
    play_stop: cmd::SYNC_TEACH_PLAY_STOP,
    clear: cmd::SYNC_TEACH_CLEAR,
    query: cmd::SYNC_TEACH_QUERY,
};

#[derive(Clone, Copy, PartialEq)]
enum Act {
    Enter,
    Exit,
    RecStart,
    RecStop,
    Play,
    PlayStop,
    Clear,
    Query,
}

struct Panel {
    kind: &'static Kind,
    editing: bool,
    recording: bool,
    playing: bool,
    /// String key of the status line, so it re-translates on language change.
    status: &'static str,
    frames: Option<u16>,
}

impl Panel {
    fn new(kind: &'static Kind) -> Self {
        Self { kind, editing: false, recording: false, playing: false, status: "lbl_teach_idle", frames: None }
    }

    /// Optimistic local state change after sending `act`, as the original does.
    fn apply(&mut self, act: Act) {
        let k = self.kind;
        match act {
            Act::Enter => {
                self.editing = true;
                self.status = k.editing;
            }
            Act::Exit => {
                (self.editing, self.recording, self.playing) = (false, false, false);
                self.status = "lbl_teach_idle";
            }
            Act::RecStart => {
                self.recording = true;
                self.status = "lbl_teach_recording";
            }
            Act::RecStop => {
                self.recording = false;
                self.status = k.editing;
            }
            Act::Play => {
                self.playing = true;
                self.status = "lbl_teach_playing";
            }
            Act::PlayStop => {
                self.playing = false;
                self.status = k.editing;
            }
            // Action-edit clear resets the status line; sync clear resets the frame count.
            Act::Clear if std::ptr::eq(k, &SYNC) => self.frames = Some(0),
            Act::Clear => self.status = k.editing,
            Act::Query => {}
        }
    }

    fn command(&self, act: Act) -> u8 {
        let k = self.kind;
        match act {
            Act::Enter => k.enter,
            Act::Exit => k.exit,
            Act::RecStart => k.rec_start,
            Act::RecStop => k.rec_stop,
            Act::Play => k.play,
            Act::PlayStop => k.play_stop,
            Act::Clear => k.clear,
            Act::Query => k.query,
        }
    }

    fn on_status(&mut self, s: &TeachStatus) {
        (self.editing, self.recording, self.playing) = (s.mode != 0, s.recording != 0, s.playing != 0);
        self.status = if s.overflow.is_some_and(|o| o != 0) {
            "lbl_sync_overflow"
        } else if self.playing {
            "lbl_teach_playing"
        } else if self.recording {
            "lbl_teach_recording"
        } else if self.editing {
            self.kind.editing
        } else {
            "lbl_teach_idle"
        };
        self.frames = Some(s.count);
    }

    fn enabled(&self, act: Act) -> bool {
        let (e, r, p) = (self.editing, self.recording, self.playing);
        match act {
            Act::Enter => !e,
            Act::Exit => e && !r,
            Act::RecStart | Act::Play | Act::Clear => e && !r && !p,
            Act::RecStop => r,
            Act::PlayStop => p,
            Act::Query => true,
        }
    }
}

pub struct TeachPage {
    app: Entity<AppState>,
    action: Panel,
    sync: Panel,
    _subs: Vec<Subscription>,
}

impl TeachPage {
    pub fn new(app: Entity<AppState>, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let subs = vec![
            cx.observe(&app, |_, _, cx| cx.notify()),
            cx.subscribe(&app, |this, _, ev: &DeviceEvent, cx| match ev {
                DeviceEvent::Response(Response::ActionEdit(s)) => {
                    this.action.on_status(s);
                    cx.notify();
                }
                DeviceEvent::Response(Response::SyncTeach(s)) => {
                    this.sync.on_status(s);
                    cx.notify();
                }
                _ => {}
            }),
        ];
        Self { app, action: Panel::new(&ACTION), sync: Panel::new(&SYNC), _subs: subs }
    }

    pub fn on_shown(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    fn act(&mut self, sync: bool, act: Act, cx: &mut Context<Self>) {
        let panel = if sync { &mut self.sync } else { &mut self.action };
        let command = panel.command(act);
        panel.apply(act);
        self.app.update(cx, |a, cx| a.send_sys(command, [], cx));
        cx.notify();
    }

    fn render_panel(&self, sync: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let p = if sync { &self.sync } else { &self.action };
        let k = p.kind;
        let lang = self.app.read(cx).lang;
        let t = |key: &str| crate::i18n::tr(lang, key);
        let frames = p.frames.map_or("--".to_string(), |n| n.to_string());

        let button = |act: Act, label: &str, color: u8, cx: &mut Context<Self>| {
            let b = Button::new(SharedString::from(format!("{}-{}", k.id, act as u8)))
                .w_full()
                .label(t(label))
                .disabled(!p.enabled(act))
                .on_click(cx.listener(move |this, _, _, cx| this.act(sync, act, cx)));
            let b = match color {
                0 => b.info(),
                1 => b.warning(),
                2 => b.danger(),
                3 => b.success(),
                _ => b,
            };
            div().flex_1().child(b)
        };
        let row = |a, b| h_flex().gap_2().child(a).child(b);

        section(t(k.title)).child(
            v_flex()
                .gap_2()
                .child(div().text_center().font_weight(FontWeight::BOLD).child(t(p.status)))
                .child(
                    div()
                        .text_center()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{}: {frames}", t("lbl_frames"))),
                )
                .child(row(button(Act::Enter, k.enter_label, 0, cx), button(Act::Exit, k.exit_label, 4, cx)))
                .child(row(
                    button(Act::RecStart, "btn_teach_rec_start", 1, cx),
                    button(Act::RecStop, "btn_teach_rec_stop", 2, cx),
                ))
                .child(row(
                    button(Act::Play, "btn_teach_play", 3, cx),
                    button(Act::PlayStop, "btn_teach_play_stop", 2, cx),
                ))
                .child(row(button(Act::Clear, k.clear_label, 2, cx), button(Act::Query, "btn_teach_query", 4, cx))),
        )
    }
}

impl Render for TeachPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("teach-page")
            .size_full()
            .p_3()
            .gap_4()
            .overflow_y_scroll()
            .child(self.render_panel(false, cx))
            .child(self.render_panel(true, cx))
    }
}

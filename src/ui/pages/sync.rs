//! Synchronizer: service tool for the leader arm (Nex_Arm_sys firmware), ported from
//! `sync_host_gui.exe`. The exchanges live in [`crate::sync`]; this page collects the
//! inputs, runs one task at a time on a worker thread and shows its progress in the log.

use crate::i18n::tr;
use crate::sync::{LinkBus, Note, SERVO_IDS, Session, Task as SyncTask};
use crate::ui::state::AppState;
use crate::ui::widgets::{self, NumState, section};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{InputEvent, NumberInput};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;

pub struct SyncPage {
    app: Entity<AppState>,
    center_pos: NumState,
    sid: NumState,
    offsets: [NumState; 6],
    /// Set while a task runs; only one runs at a time.
    running: bool,
    _task: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl SyncPage {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let s = app.read(cx).settings.sync.clone();
        let center_pos = widgets::num_input(s.center_pos.min(4095) as f64, 0., 4095., 1., window, cx);
        let sid = widgets::num_input(s.selected_servo.clamp(1, 6) as f64, 1., 6., 1., window, cx);
        let offsets: [NumState; 6] = std::array::from_fn(|i| {
            widgets::num_input(s.offsets[i].clamp(-2047, 2047) as f64, -2047., 2047., 1., window, cx)
        });
        let mut subs = vec![cx.observe(&app, |_, _, cx| cx.notify())];
        // Keep typed values even when no task runs (the original saved them on close).
        for input in [&center_pos, &sid].into_iter().chain(&offsets) {
            subs.push(cx.subscribe(input, |this, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    this.save_settings(cx);
                }
            }));
        }
        Self { app, center_pos, sid, offsets, running: false, _task: None, _subs: subs }
    }

    fn t(&self, key: &str, cx: &App) -> String {
        tr(self.app.read(cx).lang, key)
    }

    fn center_pos(&self, cx: &App) -> u16 {
        widgets::read_i(&self.center_pos, cx).clamp(0, 4095) as u16
    }

    fn selected(&self, cx: &App) -> u8 {
        widgets::read_i(&self.sid, cx).clamp(1, 6) as u8
    }

    fn offset(&self, sid: u8, cx: &App) -> i16 {
        widgets::read_i(&self.offsets[sid as usize - 1], cx).clamp(-2047, 2047) as i16
    }

    /// Run `task` on a worker thread; `label_key` names it in the log.
    fn start(&mut self, task: SyncTask, label_key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let Some((link, frames)) = self.app.update(cx, |a, _| a.open_frame_tap()) else { return };
        let label = self.t(label_key, cx);
        let (done, failed) = (self.t("msg_sync_task_done", cx), self.t("msg_sync_task_failed", cx));
        self.app.update(cx, |a, cx| a.log_text(format!("[TASK] {label} …"), cx));
        self.running = true;
        cx.notify();

        let (tx, rx) = flume::unbounded::<Note>();
        let worker = std::thread::Builder::new().name("nexarm-sync".into()).spawn(move || {
            let res = Session::new(LinkBus { link, frames }, |n| {
                let _ = tx.send(n);
            })
            .run(&task);
            let _ = tx.send(Note::Log(match res {
                Ok(()) => format!("[TASK] {label} {done}"),
                Err(e) => format!("[TASK] {label} {failed}: {e:#}"),
            }));
        });
        if let Err(e) = worker {
            self.running = false;
            return widgets::notify_error(e.to_string(), window, cx);
        }

        // The channel closes when the worker ends, including by panic.
        self._task = Some(cx.spawn_in(window, async move |this, cx| {
            while let Ok(note) = rx.recv_async().await {
                if this.update_in(cx, |p, window, cx| p.on_note(note, window, cx)).is_err() {
                    return;
                }
            }
            let _ = this.update(cx, |p, cx| p.finish(cx));
        }));
    }

    fn on_note(&mut self, note: Note, window: &mut Window, cx: &mut Context<Self>) {
        match note {
            Note::Log(text) => self.app.update(cx, |a, cx| a.log_text(text, cx)),
            Note::Offset(sid, v) if SERVO_IDS.contains(&sid) => {
                widgets::set_num(&self.offsets[sid as usize - 1], v as f64, window, cx)
            }
            Note::Offset(..) => {}
        }
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        self.running = false;
        self.save_settings(cx);
        cx.notify();
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        let s = crate::settings::SyncSettings {
            center_pos: self.center_pos(cx),
            selected_servo: self.selected(cx),
            offsets: std::array::from_fn(|i| self.offset(i as u8 + 1, cx)),
        };
        if self.app.read(cx).settings.sync == s {
            return;
        }
        self.app.update(cx, |a, _| {
            a.settings.sync = s;
            a.persist();
        });
    }

    /// Calibration rewrites the servo's centre in EEPROM, so it asks first.
    fn confirm_calibrate(&mut self, sid: Option<u8>, window: &mut Window, cx: &mut Context<Self>) {
        let (task, key, msg) = match sid {
            Some(id) => (
                SyncTask::CalibrateOne(id),
                "btn_sync_cali",
                self.t("msg_sync_cali_one", cx).replace("{id}", &id.to_string()),
            ),
            None => (SyncTask::CalibrateAll, "btn_sync_cali_all", self.t("msg_cali_confirm", cx)),
        };
        let this = cx.entity().downgrade();
        widgets::confirm(self.t(key, cx), msg, window, cx, move |window, cx| {
            let _ = this.update(cx, |p, cx| p.start(task.clone(), key, window, cx));
        });
    }

    fn button(
        &self,
        id: impl Into<ElementId>,
        label_key: &str,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Button {
        let enabled = !self.running && self.app.read(cx).is_connected();
        Button::new(id)
            .small()
            .label(self.t(label_key, cx))
            .disabled(!enabled)
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
    }

    /// Button that runs a task built from the current inputs.
    fn task_button(
        &self,
        id: impl Into<ElementId>,
        label_key: &'static str,
        cx: &mut Context<Self>,
        task: impl Fn(&Self, &App) -> SyncTask + 'static,
    ) -> Button {
        self.button(id, label_key, cx, move |this, window, cx| {
            let t = task(this, cx);
            this.start(t, label_key, window, cx)
        })
    }

    fn offset_row(&self, sid: u8, cx: &mut Context<Self>) -> Div {
        let i = sid as usize;
        h_flex()
            .gap_2()
            .items_center()
            .child(div().w(px(60.)).text_sm().child(sid.to_string()))
            .child(div().w(px(140.)).child(NumberInput::new(&self.offsets[i - 1]).small()))
            .child(self.task_button(("sync-read", i), "btn_sync_read", cx, move |_, _| SyncTask::ReadOffset(sid)))
            .child(self.task_button(("sync-write", i), "btn_sync_write", cx, move |p, cx| {
                SyncTask::WriteOffset(sid, p.offset(sid, cx))
            }))
            .child(self.task_button(("sync-zero", i), "btn_sync_zero", cx, move |_, _| SyncTask::WriteOffset(sid, 0)))
            .child(self.button(("sync-cali", i), "btn_sync_cali", cx, move |p, window, cx| {
                p.confirm_calibrate(Some(sid), window, cx)
            }))
            .child(self.task_button(("sync-center", i), "btn_sync_center", cx, move |p, cx| {
                SyncTask::CenterOne(sid, p.center_pos(cx))
            }))
    }
}

impl Render for SyncPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let lang = self.app.read(cx).lang;
        let t = |k: &str| tr(lang, k);
        let row = || h_flex().gap_3().items_center().flex_wrap();
        let num = |state: &NumState| div().w(px(140.)).child(NumberInput::new(state).small());

        let quick = section(t("grp_sync_quick")).child(
            row()
                .child(widgets::field(t("lbl_center_pos"), num(&self.center_pos), cx))
                .child(self.task_button("sync-baud", "btn_sync_baud_1m", cx, |_, _| SyncTask::RecoverBaud1M))
                .child(self.task_button("sync-center-all", "btn_sync_center_all", cx, |p, cx| {
                    SyncTask::CenterAll(p.center_pos(cx))
                }))
                .child(self.button("sync-cali-all", "btn_sync_cali_all", cx, |p, window, cx| {
                    p.confirm_calibrate(None, window, cx)
                }))
                .child(self.task_button("sync-on", "btn_sync_torque_on", cx, |_, _| SyncTask::Torque(true)))
                .child(self.task_button("sync-off", "btn_sync_torque_off", cx, |_, _| SyncTask::Torque(false)))
                .child(self.task_button("sync-pos", "btn_sync_read_pos", cx, |_, _| SyncTask::ReadPositions)),
        );

        let single = section(t("grp_sync_single")).child(
            row()
                .child(widgets::field(t("lbl_servo_id"), num(&self.sid), cx))
                .child(self.task_button("sync-one-center", "btn_sync_center", cx, |p, cx| {
                    SyncTask::CenterOne(p.selected(cx), p.center_pos(cx))
                }))
                .child(self.button("sync-one-cali", "btn_sync_cali", cx, |p, window, cx| {
                    let sid = p.selected(cx);
                    p.confirm_calibrate(Some(sid), window, cx)
                }))
                .child(self.task_button("sync-one-read", "btn_get_offset", cx, |p, cx| {
                    SyncTask::ReadOffset(p.selected(cx))
                }))
                .child(self.task_button("sync-one-write", "btn_set_offset", cx, |p, cx| {
                    let sid = p.selected(cx);
                    SyncTask::WriteOffset(sid, p.offset(sid, cx))
                })),
        );

        let header = h_flex()
            .gap_2()
            .text_sm()
            .text_color(muted)
            .child(div().w(px(60.)).child(t("lbl_servo_id")))
            .child(div().w(px(140.)).child(t("lbl_sync_offset")));
        let offsets = section(t("grp_sync_offsets"))
            .child(header)
            .children(SERVO_IDS.map(|sid| self.offset_row(sid, cx)))
            .child(
                row()
                    .pt_2()
                    .child(self.task_button("sync-read-all", "btn_sync_read_all", cx, |_, _| SyncTask::ReadAllOffsets))
                    .child(self.task_button("sync-write-all", "btn_sync_write_all", cx, |p, cx| {
                        SyncTask::WriteAllOffsets(std::array::from_fn(|i| p.offset(i as u8 + 1, cx)))
                    }))
                    .child(self.task_button("sync-zero-all", "btn_sync_zero_all", cx, |_, _| SyncTask::ZeroAllOffsets)),
            );

        v_flex()
            .id("sync-page")
            .size_full()
            .p_3()
            .gap_3()
            .overflow_y_scroll()
            .child(div().text_sm().text_color(muted).child(t("sync_hint")))
            .child(quick)
            .child(single)
            .child(offsets)
    }
}


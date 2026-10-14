//! Coordinate control: XY direction pad, Z/pitch jog, absolute move, live pose.

use crate::protocol::{self, cmd};
use crate::ui::state::{AppState, DeviceEvent};
use crate::ui::widgets::{self, NumState, field, section};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::NumberInput;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use std::time::Duration;

/// Fields of the absolute-move form: (name, default, min, max, step).
const FIELDS: [(&str, f64, f64, f64, f64); 7] = [
    ("X", 200., 0., 550., 1.),
    ("Y", 0., -550., 550., 1.),
    ("Z", 200., 100., 570., 1.),
    ("Pitch", 0., -1000., 1000., 0.1),
    ("Roll", 0., -90., 90., 1.),
    ("Claw", 0., -60., 30., 0.1),
    ("Time", 1000., 0., 1_000_000., 1.),
];
const JOG_TIME_MS: u16 = 200;
const JOG_SIZE: Pixels = px(64.);
/// Hold-to-repeat timing (first delay, interval) in ms: pad 150/150, Z and pitch 400/200.
const PAD_REPEAT: (u64, u64) = (150, 150);
const ZP_REPEAT: (u64, u64) = (400, 200);

#[derive(Clone, Copy, PartialEq)]
enum Axis {
    X,
    Y,
    Z,
    Pitch,
}

pub struct CoordPage {
    app: Entity<AppState>,
    inputs: Vec<NumState>,
    step: NumState,
    speed: NumState,
    /// Hold-to-repeat jog in progress; dropping it stops the repeat.
    repeat: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl CoordPage {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let inputs = FIELDS.iter().map(|&(_, v, lo, hi, st)| widgets::num_input(v, lo, hi, st, window, cx)).collect();
        let subs = vec![cx.observe(&app, |_, _, cx| cx.notify()), cx.subscribe(&app, |_, _, ev: &DeviceEvent, cx| {
            if matches!(ev, DeviceEvent::Response(protocol::Response::Pose(_))) {
                cx.notify();
            }
        })];
        Self {
            app,
            inputs,
            step: widgets::num_input(10., 1., 100., 1., window, cx),
            speed: widgets::num_input(20., 10., 2000., 1., window, cx),
            repeat: None,
            _subs: subs,
        }
    }

    pub fn on_shown(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.app.update(cx, |a, cx| a.send_sys(cmd::GET_CUR_COORDS, [], cx));
    }

    fn value(&self, i: usize, cx: &App) -> f64 {
        let (_, _, lo, hi, _) = FIELDS[i];
        widgets::read_num(&self.inputs[i], cx).clamp(lo, hi)
    }

    fn send_move(&mut self, cx: &mut Context<Self>) {
        let v: Vec<f64> = (0..FIELDS.len()).map(|i| self.value(i, cx)).collect();
        let req = protocol::coordinate_set(
            v[0] as i16,
            v[1] as i16,
            v[2] as i16,
            v[3],
            v[4] as i16,
            v[5] as i16,
            v[6] as u16,
        );
        self.app.update(cx, |a, cx| a.send(req, cx));
    }

    fn jog(&mut self, axis: Axis, dir: i16, cx: &mut Context<Self>) {
        let step = widgets::read_num(&self.step, cx).clamp(1., 100.) as i16 * dir;
        let (mut dx, mut dy, mut dz, mut dp) = (0, 0, 0, 0.0);
        match axis {
            Axis::X => dx = step,
            Axis::Y => dy = step,
            Axis::Z => dz = step,
            Axis::Pitch => dp = step as f64,
        }
        let req = protocol::arm_move_inc(dx, dy, dz, dp, 0, 0, JOG_TIME_MS);
        self.app.update(cx, |a, cx| a.send(req, cx));
    }

    /// Jog once now, then keep jogging every `interval` after `delay` while held.
    fn start_jog(&mut self, axis: Axis, dir: i16, delay: Duration, interval: Duration, cx: &mut Context<Self>) {
        self.jog(axis, dir, cx);
        self.repeat = Some(cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(delay).await;
            loop {
                if this.update(cx, |p, cx| p.jog(axis, dir, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(interval).await;
            }
        }));
    }

    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (i, &(_, v, ..)) in FIELDS.iter().enumerate().take(5) {
            widgets::set_num(&self.inputs[i], v, window, cx);
        }
        self.send_move(cx);
    }

    /// Hold-to-repeat jog button.
    fn jog_button(
        &self,
        id: &'static str,
        label: &'static str,
        axis: Axis,
        dir: i16,
        (delay_ms, interval_ms): (u64, u64),
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .id(id)
            .size(JOG_SIZE)
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .bg(theme.secondary)
            .border_1()
            .border_color(theme.border)
            .font_weight(FontWeight::BOLD)
            .cursor_pointer()
            .hover(|d| d.bg(rgb(0xFA8F01)).text_color(white()))
            .child(label)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    let (d, i) = (Duration::from_millis(delay_ms), Duration::from_millis(interval_ms));
                    this.start_jog(axis, dir, d, i, cx)
                }),
            )
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.repeat = None))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, _| this.repeat = None))
            .on_hover(cx.listener(|this, hovered: &bool, _, _| {
                if !*hovered {
                    this.repeat = None;
                }
            }))
    }

    fn render_pad(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = JOG_SIZE;
        let reset_label = self.app.read(cx).t("btn_reset_view");
        let cell = || div().size(s);
        // Same layout as the original wheel: X+ up, Y- right, X- down, Y+ left.
        v_flex()
            .gap_1()
            .child(h_flex().gap_1().child(cell()).child(self.jog_button("pad-x+", "X+", Axis::X, 1, PAD_REPEAT, cx)).child(cell()))
            .child(
                h_flex()
                    .gap_1()
                    .child(self.jog_button("pad-y+", "Y+", Axis::Y, 1, PAD_REPEAT, cx))
                    .child(
                        div()
                            .id("pad-reset")
                            .size(s)
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .border_2()
                            .border_color(cx.theme().foreground)
                            .text_sm()
                            .cursor_pointer()
                            .hover(|d| d.bg(cx.theme().secondary_hover))
                            .child(reset_label)
                            .on_click(cx.listener(|this, _, window, cx| this.reset(window, cx))),
                    )
                    .child(self.jog_button("pad-y-", "Y-", Axis::Y, -1, PAD_REPEAT, cx)),
            )
            .child(h_flex().gap_1().child(cell()).child(self.jog_button("pad-x-", "X-", Axis::X, -1, PAD_REPEAT, cx)).child(cell()))
    }

    fn render_jog_column(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_4()
            .child(
                v_flex()
                    .gap_4()
                    .child(self.jog_button("z+", "Z+", Axis::Z, 1, ZP_REPEAT, cx))
                    .child(self.jog_button("z-", "Z-", Axis::Z, -1, ZP_REPEAT, cx)),
            )
            .child(
                v_flex()
                    .gap_4()
                    .child(self.jog_button("p+", "P+", Axis::Pitch, 1, ZP_REPEAT, cx))
                    .child(self.jog_button("p-", "P-", Axis::Pitch, -1, ZP_REPEAT, cx)),
            )
    }
}

impl Render for CoordPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (lang, pose) = {
            let app = self.app.read(cx);
            (app.lang, app.pose.clone())
        };
        let t = |k: &str| crate::i18n::tr(lang, k);
        let assets = crate::assets_dir();

        let visual = section(t("grp_ik"))
            .flex_1()
            .min_w(px(420.))
            .child(
                h_flex()
                    .justify_end()
                    .child(
                        Button::new("open-3d")
                            .outline()
                            .label(t("btn_open_3d"))
                            .on_click(cx.listener(|this, _, _, cx| this.app.update(cx, |a, cx| a.open_3d_viewer(cx)))),
                    ),
            )
            .child(
                h_flex()
                    .gap_6()
                    .justify_around()
                    .child(img(assets.join("0.842_old.png")).size(px(280.)).object_fit(ObjectFit::Contain))
                    .child(img(assets.join("coord_axes.png")).size(px(220.)).object_fit(ObjectFit::Contain)),
            )
            .child(h_flex().gap_6().justify_around().py_4().child(self.render_pad(cx)).child(self.render_jog_column(cx)))
            .child(
                h_flex()
                    .gap_4()
                    .child(field(t("lbl_step_mm"), NumberInput::new(&self.step).small(), cx).w(px(260.)))
                    .child(field(t("lbl_speed_mms"), NumberInput::new(&self.speed).small(), cx).w(px(260.))),
            );

        let params = section(t("grp_coordinates"))
            .child(v_flex().gap_1().children(
                FIELDS.iter().enumerate().map(|(i, (name, ..))| field(*name, NumberInput::new(&self.inputs[i]).small(), cx)),
            ))
            .child(
                Button::new("send-coord")
                    .primary()
                    .w_full()
                    .label(t("btn_send_coord"))
                    .on_click(cx.listener(|this, _, _, cx| this.send_move(cx))),
            );

        let coord_vals: [(&str, String); 6] = match &pose {
            Some(p) => [
                ("X", p.x.to_string()),
                ("Y", p.y.to_string()),
                ("Z", p.z.to_string()),
                ("Pitch", format!("{:.1}", p.pitch)),
                ("Roll", p.roll.to_string()),
                ("Claw", format!("{:.1}", p.claw as f32)),
            ],
            None => [
                ("X", "200".into()),
                ("Y", "0".into()),
                ("Z", "200".into()),
                ("Pitch", "0.0".into()),
                ("Roll", "0".into()),
                ("Claw", "0.0".into()),
            ],
        };
        let servo_txt = |i: usize| match &pose {
            Some(p) => format!("ID{}: {}", i + 1, p.servos[i]),
            None => format!("ID{}: --", i + 1),
        };
        let grid = |items: Vec<String>, cols: usize| {
            v_flex().gap_1().children(items.chunks(cols).map(|row| {
                h_flex().children(row.iter().map(|s| div().flex_1().text_center().child(s.clone())))
            }).collect::<Vec<_>>())
        };

        let stat = section(t("grp_realtime_stat"))
            .child(
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .child(Button::new("get-coord").small().label(t("btn_get_coord")).on_click(
                        cx.listener(|this, _, _, cx| this.app.update(cx, |a, cx| a.send_sys(cmd::GET_CUR_COORDS, [], cx))),
                    ))
                    .child(Button::new("home-all").small().label(t("btn_home_all")).on_click(
                        cx.listener(|this, _, _, cx| this.app.update(cx, |a, cx| a.send(protocol::arm_reset(1000), cx))),
                    ))
                    .child(Button::new("read-servos").small().label(t("btn_read_servos")).on_click(
                        cx.listener(|this, _, _, cx| this.app.update(cx, |a, cx| a.send_sys(cmd::READ_ALL_SERVOS, [], cx))),
                    )),
            )
            .child(grid(coord_vals.iter().map(|(n, v)| format!("{n}: {v}")).collect(), 2))
            .child(div().text_center().text_color(cx.theme().muted_foreground).child(t("lbl_realtime_servo")))
            .child(grid((0..6).map(servo_txt).collect(), 3));

        h_flex()
            .id("coord-page")
            .size_full()
            .p_3()
            .gap_3()
            .items_start()
            .overflow_y_scroll()
            .child(visual)
            .child(v_flex().w(px(340.)).flex_none().gap_3().child(params).child(stat))
    }
}

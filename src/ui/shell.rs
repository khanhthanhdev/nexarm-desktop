//! Main window: sidebar (logo, page list, connection panel, language) + page area + log.

use super::engine_state::EngineState;
use super::log_panel::LogPanel;
use super::pages::{
    ai::AiPage, coord::CoordPage, peripheral::PeripheralPage, servo::ServoPage, servo_adv::ServoAdvPage,
    lab_session::LabSessionPage, sync::SyncPage, system::SystemPage, teach::TeachPage,
};
use super::state::{AppState, ConnStatus, DeviceEvent};
use super::widgets::{self, StrSelect};
use crate::link::list_serial_ports;
use crate::settings::{self, ConnMethod};
use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectEvent};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

const NAV_KEYS: [&str; 9] = [
    "tab_servo",
    "tab_coord",
    "tab_peripheral",
    "tab_system",
    "tab_ai",
    "tab_servo_adv",
    "tab_teach",
    "tab_sync",
    "tab_lab_session",
];
/// Index of the first Lab page in [`NAV_KEYS`]; Lab pages live in `Shell::lab`.
const LAB_FIRST: usize = 8;
const BAUDS: [u32; 11] = [9600, 19200, 38400, 57600, 76800, 115200, 128000, 250000, 500000, 1000000, 2000000];
const ACCENT: u32 = 0xFA8F01;

struct Pages {
    servo: Entity<ServoPage>,
    coord: Entity<CoordPage>,
    peripheral: Entity<PeripheralPage>,
    system: Entity<SystemPage>,
    ai: Entity<AiPage>,
    servo_adv: Entity<ServoAdvPage>,
    teach: Entity<TeachPage>,
    sync: Entity<SyncPage>,
}

pub struct Shell {
    app: Entity<AppState>,
    nav: usize,
    pages: Pages,
    lab: Vec<AnyView>,
    log: Entity<LogPanel>,
    method: Entity<StrSelect>,
    port: Entity<StrSelect>,
    baud: Entity<StrSelect>,
    ssid: Entity<InputState>,
    ssid_list: Entity<StrSelect>,
    pwd: Entity<InputState>,
    show_pwd: bool,
    scanning: bool,
    _subs: Vec<Subscription>,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = cx.new(AppState::new);
        let s = app.read(cx).settings.clone();
        let lang = app.read(cx).lang;
        let tr = |k: &str| crate::i18n::tr(lang, k);

        let pages = Pages {
            servo: cx.new(|cx| ServoPage::new(app.clone(), window, cx)),
            coord: cx.new(|cx| CoordPage::new(app.clone(), window, cx)),
            peripheral: cx.new(|cx| PeripheralPage::new(app.clone(), window, cx)),
            system: cx.new(|cx| SystemPage::new(app.clone(), window, cx)),
            ai: cx.new(|cx| AiPage::new(app.clone(), window, cx)),
            servo_adv: cx.new(|cx| ServoAdvPage::new(app.clone(), window, cx)),
            teach: cx.new(|cx| TeachPage::new(app.clone(), window, cx)),
            sync: cx.new(|cx| SyncPage::new(app.clone(), window, cx)),
        };
        let engine = cx.new(|cx| EngineState::new(app.clone(), cx));
        let lab: Vec<AnyView> = vec![cx.new(|cx| LabSessionPage::new(app.clone(), engine.clone(), window, cx)).into()];
        let log = cx.new(|cx| LogPanel::new(app.clone(), cx));

        let method_ix = if s.method == ConnMethod::Wifi { 1 } else { 0 };
        let method = widgets::select(vec![tr("method_serial").into(), tr("method_wifi").into()], method_ix, window, cx);
        let ports = list_serial_ports();
        let port_ix = s.serial_port.as_ref().and_then(|p| ports.iter().position(|x| x == p)).unwrap_or(0);
        let port = widgets::select(ports.into_iter().map(Into::into).collect(), port_ix, window, cx);
        let baud_ix = BAUDS.iter().position(|b| *b == s.baud).unwrap_or(9);
        let baud = widgets::select(BAUDS.iter().map(|b| b.to_string().into()).collect(), baud_ix, window, cx);
        let ssid = widgets::text_input(&s.last_ssid, "SSID", window, cx);
        let ssid_list = widgets::select(vec![], 0, window, cx);
        let saved_pwd = settings::load_wifi_password(&s.last_ssid).unwrap_or_default();
        let pwd = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Password (if needed)")
                .default_value(saved_pwd)
        });

        let subs = vec![
            cx.observe(&app, |_, _, cx| cx.notify()),
            cx.subscribe_in(&app, window, |this, _, ev: &DeviceEvent, window, cx| {
                if matches!(ev, DeviceEvent::LangChanged) {
                    this.relabel(window, cx);
                }
            }),
            cx.subscribe_in(&method, window, |this, _, _: &SelectEvent<_>, _, cx| {
                let ix = widgets::selected_index(&this.method, cx);
                this.app.update(cx, |a, _| {
                    a.settings.method = if ix == 1 { ConnMethod::Wifi } else { ConnMethod::Serial };
                    a.persist();
                });
                cx.notify();
            }),
            cx.subscribe_in(&ssid_list, window, |this, _, ev: &SelectEvent<_>, window, cx| {
                let SelectEvent::Confirm(Some(v)) = ev else { return };
                let v: SharedString = v.clone();
                widgets::set_text(&this.ssid, v.clone(), window, cx);
                this.load_password(&v, window, cx);
            }),
            cx.subscribe_in(&ssid, window, |this, state, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    let v = state.read(cx).value();
                    this.load_password(&v, window, cx);
                }
            }),
        ];

        Self {
            app,
            nav: 0,
            pages,
            lab,
            log,
            method,
            port,
            baud,
            ssid,
            ssid_list,
            pwd,
            show_pwd: false,
            scanning: false,
            _subs: subs,
        }
    }

    fn load_password(&mut self, ssid: &str, window: &mut Window, cx: &mut Context<Self>) {
        if ssid.is_empty() {
            return;
        }
        let pw = settings::load_wifi_password(ssid).unwrap_or_default();
        widgets::set_text(&self.pwd, pw, window, cx);
    }

    fn relabel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (a, b) = {
            let app = self.app.read(cx);
            (app.t("method_serial"), app.t("method_wifi"))
        };
        let ix = widgets::selected_index(&self.method, cx);
        widgets::set_items(&self.method, vec![a.into(), b.into()], ix, window, cx);
        let title = self.app.read(cx).t("window_title");
        window.set_window_title(&title);
    }

    fn select_page(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.nav = ix;
        match ix {
            0 => self.pages.servo.update(cx, |p, cx| p.on_shown(window, cx)),
            1 => self.pages.coord.update(cx, |p, cx| p.on_shown(window, cx)),
            _ => {}
        }
        cx.notify();
    }

    fn refresh_ports(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = widgets::selected_text(&self.port, cx);
        let ports = list_serial_ports();
        let ix = current.and_then(|c| ports.iter().position(|p| *p == c)).unwrap_or(0);
        widgets::set_items(&self.port, ports.into_iter().map(Into::into).collect(), ix, window, cx);
    }

    fn scan_wifi(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.scanning = true;
        cx.notify();
        let task = cx.background_executor().spawn(async { crate::wifi::scan_ssids() });
        cx.spawn_in(window, async move |this, cx| {
            let res = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.scanning = false;
                match res {
                    Ok(list) if !list.is_empty() => {
                        let current = widgets::read_text(&this.ssid, cx);
                        let ix = list.iter().position(|s| *s == current).unwrap_or(0);
                        widgets::set_items(&this.ssid_list, list.into_iter().map(Into::into).collect(), ix, window, cx);
                    }
                    Ok(_) => this.app.update(cx, |a, cx| a.log_text("No WiFi networks found.", cx)),
                    Err(e) => this.app.update(cx, |a, cx| a.log_text(format!("WiFi scan: {e}"), cx)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_connect(&mut self, cx: &mut Context<Self>) {
        let (connected, connecting) = {
            let a = self.app.read(cx);
            (a.is_connected(), matches!(a.status, ConnStatus::Connecting(_)))
        };
        if connecting {
            return;
        }
        if connected {
            return self.app.update(cx, |a, cx| a.disconnect(cx));
        }
        if widgets::selected_index(&self.method, cx) == 0 {
            let Some(port) = widgets::selected_text(&self.port, cx) else {
                return self.app.update(cx, |a, cx| a.log_text("Connection Error: No port selected", cx));
            };
            let baud = widgets::selected_text(&self.baud, cx).and_then(|b| b.parse().ok()).unwrap_or(1_000_000);
            self.app.update(cx, |a, cx| a.connect_serial(port, baud, cx));
        } else {
            let ssid = widgets::read_text(&self.ssid, cx).trim().to_string();
            if ssid.is_empty() {
                return;
            }
            let pwd = widgets::read_text(&self.pwd, cx);
            self.app.update(cx, |a, cx| a.connect_wifi_ap(ssid, pwd, cx));
        }
    }

    fn page_view(&self) -> AnyView {
        match self.nav {
            0 => self.pages.servo.clone().into(),
            1 => self.pages.coord.clone().into(),
            2 => self.pages.peripheral.clone().into(),
            3 => self.pages.system.clone().into(),
            4 => self.pages.ai.clone().into(),
            5 => self.pages.servo_adv.clone().into(),
            6 => self.pages.teach.clone().into(),
            7 => self.pages.sync.clone().into(),
            i => self.lab[i - LAB_FIRST].clone(),
        }
    }

    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.read(cx);
        let theme = cx.theme();
        v_flex().gap_1().px_3().children(NAV_KEYS.iter().enumerate().map(|(i, key)| {
            let active = i == self.nav;
            div()
                .id(("nav", i))
                .px_4()
                .py_2()
                .rounded_md()
                .cursor_pointer()
                .when(active, |d| d.bg(rgb(ACCENT)).text_color(white()))
                .when(!active, |d| d.hover(|d| d.bg(theme.secondary_hover)))
                .child(app.t(key))
                .on_click(cx.listener(move |this, _, window, cx| this.select_page(i, window, cx)))
        }))
    }

    fn render_connection(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.read(cx);
        let muted = cx.theme().muted_foreground;
        let caption = |k: &str| div().text_sm().text_color(muted).child(app.t(k));
        let (dot, status) = match &app.status {
            ConnStatus::Connected(_) => (rgb(0x4CAF50), app.t("status_connected")),
            ConnStatus::Connecting(what) => (rgb(ACCENT), format!("… {what}")),
            ConnStatus::Disconnected => (rgb(0xF44336), app.t("status_disconnected")),
        };
        let connected = app.is_connected();
        let wifi = widgets::selected_index(&self.method, cx) == 1;
        let _ = window;

        let serial_panel = v_flex()
            .gap_1()
            .child(caption("port"))
            .child(
                h_flex()
                    .gap_1()
                    .child(div().flex_1().min_w_0().child(Select::new(&self.port).small()))
                    .child(
                        Button::new("refresh-ports")
                            .small()
                            .outline()
                            .label(app.t("refresh"))
                            .on_click(cx.listener(|this, _, window, cx| this.refresh_ports(window, cx))),
                    ),
            )
            .child(caption("lbl_baud_rate"))
            .child(Select::new(&self.baud).small());

        let wifi_panel = v_flex()
            .gap_1()
            .child(caption("ssid"))
            .child(
                h_flex()
                    .gap_1()
                    .child(div().flex_1().min_w_0().child(Input::new(&self.ssid).small()))
                    .child(
                        Button::new("scan-wifi")
                            .small()
                            .outline()
                            .loading(self.scanning)
                            .disabled(self.scanning)
                            .label(app.t("search"))
                            .on_click(cx.listener(|this, _, window, cx| this.scan_wifi(window, cx))),
                    ),
            )
            .child(Select::new(&self.ssid_list).small().placeholder("…"))
            .child(caption("password"))
            .child(
                h_flex().gap_2().child(div().flex_1().min_w_0().child(Input::new(&self.pwd).small())).child(
                    Checkbox::new("show-pwd").label(app.t("chk_show_pwd")).checked(self.show_pwd).on_click(
                        cx.listener(|this, checked: &bool, window, cx| {
                            this.show_pwd = *checked;
                            this.pwd.update(cx, |s, cx| s.set_masked(!*checked, window, cx));
                            cx.notify();
                        }),
                    ),
                ),
            );

        v_flex()
            .gap_2()
            .px_5()
            .py_2()
            .child(h_flex().gap_2().items_center().child(div().text_color(dot).child("●")).child(status.clone()))
            .child(caption("comm_method"))
            .child(Select::new(&self.method).small())
            .child(if wifi { wifi_panel } else { serial_panel })
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_sm().child(app.t("lbl_toggle_off")))
                    .child(
                        Switch::new("connect")
                            .checked(connected)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_connect(cx))),
                    )
                    .child(div().text_sm().child(app.t("lbl_toggle_on"))),
            )
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (lang_label, title) = {
            let app = self.app.read(cx);
            let l = if app.lang == crate::i18n::Lang::En { "Tiếng Việt" } else { "English" };
            (l, app.t(NAV_KEYS[self.nav]))
        };
        let theme = cx.theme().clone();
        let logo = crate::assets_dir().join("nexarm_icon.png");

        let sidebar = v_flex()
            .w(px(240.))
            .h_full()
            .flex_none()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .child(h_flex().justify_center().py_2().child(img(logo).size(px(90.))))
            .child(div().id("nav-scroll").flex_1().min_h_0().overflow_y_scroll().child(self.render_nav(cx)))
            .child(self.render_connection(window, cx))
            .child(
                div().px_5().pb_3().child(
                    Button::new("lang")
                        .w_full()
                        .label(lang_label)
                        .on_click(cx.listener(|this, _, _, cx| this.app.update(cx, |a, cx| a.toggle_lang(cx)))),
                ),
            );

        let content = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                div()
                    .px_4()
                    .py_3()
                    .bg(theme.title_bar)
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .child(title),
            )
            .child(div().flex_1().min_h_0().child(self.page_view()))
            .child(div().h(px(170.)).flex_none().border_t_1().border_color(theme.border).p_2().child(self.log.clone()));

        h_flex().size_full().bg(theme.background).text_color(theme.foreground).child(sidebar).child(content)
    }
}

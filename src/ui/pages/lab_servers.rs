//! GPU server profiles for remote training: host, user and SSH details, saved in the app settings.
//! Keys are never stored here; the OS SSH agent (or an identity file you name) provides them.

use crate::engine::api::RemoteServer;
use crate::i18n::tr;
use crate::ui::engine_state::EngineState;
use crate::ui::lab_util::call;
use crate::ui::state::AppState;
use crate::ui::widgets::{self, NumState, StrSelect, section};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputState, NumberInput};
use gpui_kit::component::select::{Select, SelectEvent};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;

pub struct ServersPanel {
    app: Entity<AppState>,
    engine: Entity<EngineState>,
    pick: Entity<StrSelect>,
    name: Entity<InputState>,
    host: Entity<InputState>,
    user: Entity<InputState>,
    key: Entity<InputState>,
    ssh_port: NumState,
    engine_port: NumState,
    status: String,
    testing: bool,
    _subs: Vec<Subscription>,
}

/// `Ok(server)` when the fields describe a usable profile, else why not (an i18n key).
pub fn validate(server: &RemoteServer) -> Result<(), &'static str> {
    if server.name.trim().is_empty() || server.host.trim().is_empty() || server.user.trim().is_empty() {
        return Err("lab_server_need_fields");
    }
    let host_ok = server.host.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && server.host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    let user_ok = server.user.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        && !server.user.starts_with('-');
    if host_ok && user_ok { Ok(()) } else { Err("lab_server_bad_fields") }
}

impl ServersPanel {
    pub fn new(app: Entity<AppState>, engine: Entity<EngineState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pick = widgets::select(vec![], 0, window, cx);
        let subs = vec![
            cx.observe(&app, |_, _, cx| cx.notify()),
            cx.observe(&engine, |_, _, cx| cx.notify()),
            cx.subscribe_in(&pick, window, |this, _, _: &SelectEvent<_>, window, cx| this.load_fields(window, cx)),
        ];
        let mut panel = Self {
            name: widgets::text_input("", "gpu1", window, cx),
            host: widgets::text_input("", "gpu.example.com", window, cx),
            user: widgets::text_input("", "ubuntu", window, cx),
            key: widgets::text_input("", "(optional) path to an SSH key", window, cx),
            ssh_port: widgets::num_input(22., 1., 65535., 1., window, cx),
            engine_port: widgets::num_input(8765., 1., 65535., 1., window, cx),
            status: String::new(),
            testing: false,
            pick,
            app,
            engine,
            _subs: subs,
        };
        panel.reload(window, cx);
        panel
    }

    fn t(&self, key: &str, cx: &App) -> String {
        tr(self.app.read(cx).lang, key)
    }

    fn servers(&self, cx: &App) -> Vec<RemoteServer> {
        self.app.read(cx).settings.servers.clone()
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let servers = self.servers(cx);
        let ix = widgets::selected_index(&self.pick, cx);
        widgets::set_items(&self.pick, servers.iter().map(|s| SharedString::from(s.name.clone())).collect(), ix, window, cx);
        self.load_fields(window, cx);
    }

    fn load_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let servers = self.servers(cx);
        let Some(s) = widgets::selected_text(&self.pick, cx).and_then(|n| servers.into_iter().find(|s| s.name == n)) else {
            return;
        };
        widgets::set_text(&self.name, s.name, window, cx);
        widgets::set_text(&self.host, s.host, window, cx);
        widgets::set_text(&self.user, s.user, window, cx);
        widgets::set_text(&self.key, s.key_path.unwrap_or_default(), window, cx);
        widgets::set_num(&self.ssh_port, s.ssh_port as f64, window, cx);
        widgets::set_num(&self.engine_port, s.engine_port as f64, window, cx);
        self.status.clear();
        cx.notify();
    }

    fn form(&self, cx: &App) -> RemoteServer {
        let key = widgets::read_text(&self.key, cx).trim().to_string();
        RemoteServer {
            name: widgets::read_text(&self.name, cx).trim().to_string(),
            host: widgets::read_text(&self.host, cx).trim().to_string(),
            user: widgets::read_text(&self.user, cx).trim().to_string(),
            ssh_port: widgets::read_i(&self.ssh_port, cx).clamp(1, 65535) as u16,
            key_path: (!key.is_empty()).then_some(key),
            engine_port: widgets::read_i(&self.engine_port, cx).clamp(1, 65535) as u16,
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let server = self.form(cx);
        if let Err(key) = validate(&server) {
            return widgets::notify_error(self.t(key, cx), window, cx);
        }
        self.app.update(cx, |a, _| {
            match a.settings.servers.iter_mut().find(|s| s.name == server.name) {
                Some(slot) => *slot = server,
                None => a.settings.servers.push(server),
            }
            a.persist();
        });
        self.reload(window, cx);
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = widgets::read_text(&self.name, cx).trim().to_string();
        self.app.update(cx, |a, _| {
            a.settings.servers.retain(|s| s.name != name);
            a.persist();
        });
        self.reload(window, cx);
    }

    fn test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let server = self.form(cx);
        if let Err(key) = validate(&server) {
            return widgets::notify_error(self.t(key, cx), window, cx);
        }
        self.testing = true;
        self.status = self.t("lab_server_testing", cx);
        cx.notify();
        let client = self.engine.read(cx).client.clone();
        call(client, window, cx, move |c| c.test_server(&server), |this, res, _, cx| {
            this.testing = false;
            this.status = match res {
                Ok(r) if r.ssh_ok => match r.engine {
                    Some(h) => this.t("lab_server_ok", cx).replace("{version}", &h.version),
                    None => r.detail,
                },
                Ok(r) => r.detail,
                Err(e) => e.to_string(),
            };
            cx.notify();
        });
    }
}

impl Render for ServersPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.engine.read(cx).is_ready();
        let muted = cx.theme().muted_foreground;
        let num = |s: &NumState| div().w(px(120.)).child(NumberInput::new(s).small());
        v_flex().child(
            section(self.t("lab_servers", cx))
                .child(div().text_sm().text_color(muted).child(self.t("lab_servers_hint", cx)))
                .child(widgets::field(self.t("lab_server_pick", cx), Select::new(&self.pick).small().placeholder("…"), cx))
                .child(widgets::field(self.t("lab_profile_name", cx), Input::new(&self.name).small(), cx))
                .child(widgets::field(self.t("lab_server_host", cx), Input::new(&self.host).small(), cx))
                .child(widgets::field(self.t("lab_server_user", cx), Input::new(&self.user).small(), cx))
                .child(widgets::field(self.t("lab_server_key", cx), Input::new(&self.key).small(), cx))
                .child(
                    h_flex()
                        .gap_3()
                        .flex_wrap()
                        .child(widgets::field(self.t("lab_server_ssh_port", cx), num(&self.ssh_port), cx))
                        .child(widgets::field(self.t("lab_server_engine_port", cx), num(&self.engine_port), cx)),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(Button::new("server-save").small().label(self.t("lab_server_save", cx)).on_click(cx.listener(
                            |this, _, window, cx| this.save(window, cx),
                        )))
                        .child(Button::new("server-delete").small().outline().label(self.t("btn_delete", cx)).on_click(cx.listener(
                            |this, _, window, cx| this.delete(window, cx),
                        )))
                        .child(
                            Button::new("server-test")
                                .small()
                                .outline()
                                .label(self.t("lab_server_test", cx))
                                .loading(self.testing)
                                .disabled(!ready || self.testing)
                                .on_click(cx.listener(|this, _, window, cx| this.test(window, cx))),
                        ),
                )
                .child(div().text_sm().child(self.status.clone()))
                .child(div().text_sm().text_color(muted).child(self.t("lab_server_bootstrap", cx))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::validate;
    use crate::engine::api::RemoteServer;

    fn server(host: &str, user: &str) -> RemoteServer {
        RemoteServer { name: "gpu".into(), host: host.into(), user: user.into(), ..Default::default() }
    }

    #[test]
    fn plain_hosts_and_users_pass() {
        assert_eq!(validate(&server("gpu.example.com", "ubuntu")), Ok(()));
        assert_eq!(validate(&server("10.0.0.5", "ml_user")), Ok(()));
    }

    #[test]
    fn empty_and_option_like_values_are_refused() {
        assert_eq!(validate(&server("", "u")), Err("lab_server_need_fields"));
        assert_eq!(validate(&server("-oProxyCommand=x", "u")), Err("lab_server_bad_fields"));
        assert_eq!(validate(&server("a b", "u")), Err("lab_server_bad_fields"));
        assert_eq!(validate(&server("host", "root; rm")), Err("lab_server_bad_fields"));
        assert_eq!(validate(&server("host", "-x")), Err("lab_server_bad_fields"));
    }
}

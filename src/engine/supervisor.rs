//! Engine supervisor: attach to a running engine or start one, and stop only an engine this app started.

use super::API_VERSION;
use super::api::Health;
use super::client::EngineClient;
use serde::{Deserialize, Serialize};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineSettings {
    /// Where the engine listens. Keep it on loopback unless you run a remote engine on purpose.
    pub url: String,
    /// Command that starts the engine, split on spaces (quotes group). The app appends `--host` and `--port`.
    pub command: String,
    /// Working directory for the command (for example a lerobot-nexarm checkout used with `uv run`).
    pub working_dir: String,
    pub auto_start: bool,
    /// Name of the robot profile the Lab pages use.
    pub profile: String,
}

impl Default for EngineSettings {
    fn default() -> Self {
        Self {
            url: "http://127.0.0.1:8765".into(),
            command: "lerobot-nexarm-service".into(),
            working_dir: String::new(),
            auto_start: true,
            profile: String::new(),
        }
    }
}

/// What a probe of the configured URL found.
#[derive(Clone, Debug, PartialEq)]
pub enum Attach {
    /// An engine with a matching API version answers.
    Ready(Health),
    /// An engine answers but speaks another API version; it is refused.
    Mismatch { engine: String },
    /// Nothing usable answers.
    Unreachable(String),
}

pub fn probe(client: &EngineClient) -> Attach {
    match client.health() {
        Ok(h) if h.api_version == API_VERSION => Attach::Ready(h),
        Ok(h) => Attach::Mismatch { engine: h.api_version },
        Err(e) => Attach::Unreachable(e.to_string()),
    }
}

/// Split a command line on whitespace; double quotes keep a segment together.
pub fn split_command(line: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let (mut cur, mut quoted, mut any) = (String::new(), false, false);
    for ch in line.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    parts.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        parts.push(cur);
    }
    parts
}

/// Port of an `http://host:port` URL.
pub fn url_port(url: &str) -> Option<u16> {
    let rest = url.split("://").nth(1)?;
    rest.split('/').next()?.rsplit(':').next()?.parse().ok()
}

#[derive(Default)]
pub struct Supervisor {
    child: Option<Child>,
}

const STARTUP_WAIT: Duration = Duration::from_secs(90);
const STOP_WAIT: Duration = Duration::from_secs(40);

impl Supervisor {
    /// True when this app started the running engine (and so may stop it).
    pub fn owns_engine(&self) -> bool {
        self.child.is_some()
    }

    /// Start the engine process. Output goes to `engine.log` in the config directory.
    pub fn spawn(&mut self, settings: &EngineSettings) -> anyhow::Result<()> {
        use anyhow::Context as _;
        anyhow::ensure!(self.child.is_none(), "this app already started an engine");
        let parts = split_command(&settings.command);
        let (program, args) = parts.split_first().context("the engine command is empty")?;
        let port = url_port(&settings.url).context("the engine URL has no port")?;
        let log = std::fs::File::create(crate::settings::config_dir().join("engine.log")).context("cannot create engine.log")?;
        let mut cmd = Command::new(program);
        cmd.args(args).args(["--host", "127.0.0.1", "--port", &port.to_string()]);
        if !settings.working_dir.trim().is_empty() {
            cmd.current_dir(settings.working_dir.trim());
        }
        cmd.stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        self.child = Some(cmd.spawn().with_context(|| format!("cannot start `{}`", settings.command))?);
        Ok(())
    }

    /// Wait until the engine answers; fails early if the process exited.
    pub fn wait_ready(&mut self, client: &EngineClient) -> anyhow::Result<Attach> {
        let deadline = Instant::now() + STARTUP_WAIT;
        loop {
            if let Some(child) = &mut self.child
                && let Some(status) = child.try_wait()?
            {
                self.child = None;
                anyhow::bail!("the engine exited during start-up ({status}); see engine.log in the settings folder");
            }
            match probe(client) {
                a @ (Attach::Ready(_) | Attach::Mismatch { .. }) => return Ok(a),
                Attach::Unreachable(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(500)),
                Attach::Unreachable(e) => anyhow::bail!("the engine did not answer within {}s: {e}", STARTUP_WAIT.as_secs()),
            }
        }
    }

    /// Stop the engine, but only one this app started. A running session is stopped first so the arm
    /// returns to rest; then the engine is asked to exit, and killed only if it does not.
    pub fn stop(&mut self, client: &EngineClient) {
        let Some(mut child) = self.child.take() else { return };
        if let Ok(Some(session)) = client.current_session() {
            let _ = client.stop_session(&session.id);
        }
        let _ = client.shutdown();
        let deadline = Instant::now() + STOP_WAIT;
        while Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Answers every request with `body` as JSON until the test ends.
    fn mock_engine(body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { return };
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(reply.as_bytes());
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    #[test]
    fn a_matching_engine_is_attached() {
        let url = mock_engine(r#"{"status":"ok","version":"0.6.1","api_version":"1","capabilities":["sessions"],"pid":7}"#);
        match probe(&EngineClient::new(url)) {
            Attach::Ready(h) => assert_eq!((h.version.as_str(), h.pid), ("0.6.1", 7)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_engine_with_another_api_version_is_refused() {
        let url = mock_engine(r#"{"status":"ok","version":"9.0","api_version":"2","capabilities":[],"pid":1}"#);
        assert_eq!(probe(&EngineClient::new(url)), Attach::Mismatch { engine: "2".into() });
    }

    #[test]
    fn nothing_listening_is_unreachable() {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert!(matches!(probe(&EngineClient::new(format!("http://127.0.0.1:{port}"))), Attach::Unreachable(_)));
    }

    #[test]
    fn commands_split_with_quotes() {
        assert_eq!(split_command("uv run lerobot-nexarm-service"), ["uv", "run", "lerobot-nexarm-service"]);
        assert_eq!(split_command(r#""C:\Program Files\py\python.exe" -m svc"#), [r"C:\Program Files\py\python.exe", "-m", "svc"]);
        assert!(split_command("   ").is_empty());
    }

    #[test]
    fn the_port_comes_from_the_url() {
        assert_eq!(url_port("http://127.0.0.1:8765"), Some(8765));
        assert_eq!(url_port("http://127.0.0.1:9000/x"), Some(9000));
        assert_eq!(url_port("http://localhost"), None);
    }

    #[test]
    fn only_an_engine_this_app_started_is_stopped() {
        let url = mock_engine(r#"null"#);
        let mut s = Supervisor::default();
        assert!(!s.owns_engine());
        s.stop(&EngineClient::new(url)); // no child: must not touch the engine
        assert!(!s.owns_engine());
    }
}

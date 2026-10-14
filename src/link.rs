//! Serial / TCP transport. A reader thread decodes frames and reports [`LinkEvent`]s.

use crate::protocol::{Decoded, Decoder, Frame, Request, printable_text};
use anyhow::Context as _;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug)]
pub enum LinkEvent {
    /// Decoded frame with a valid checksum.
    Frame(Frame),
    /// Delimited frame whose checksum failed; shown in the hex log only.
    BadFrame(Vec<u8>),
    /// Plain-text output printed by the firmware.
    Text(String),
    /// Bytes written by us (for the hex log).
    Tx(Vec<u8>),
    /// The link closed. `None` when closed by us, otherwise the reason.
    Closed(Option<String>),
}

type Writer = Arc<Mutex<Box<dyn Write + Send>>>;

pub struct Link {
    writer: Writer,
    stop: Arc<AtomicBool>,
    events: flume::Sender<LinkEvent>,
    pub description: String,
}

const POLL: Duration = Duration::from_millis(10);

pub fn list_serial_ports() -> Vec<String> {
    let mut ports: Vec<String> = serialport::available_ports()
        .map(|v| v.into_iter().map(|p| p.port_name).collect())
        .unwrap_or_default();
    ports.sort();
    ports
}

impl Link {
    pub fn open_serial(port: &str, baud: u32, events: flume::Sender<LinkEvent>) -> anyhow::Result<Link> {
        let sp = serialport::new(port, baud)
            .timeout(POLL)
            .open()
            .with_context(|| format!("cannot open {port} (busy or no permission?)"))?;
        let reader = sp.try_clone().context("cannot clone serial handle")?;
        Ok(Self::start(Box::new(sp), reader, format!("{port} @ {baud}"), events))
    }

    pub fn open_tcp(host: &str, port: u16, events: flume::Sender<LinkEvent>) -> anyhow::Result<Link> {
        let addr = (host, port)
            .to_socket_addrs()?
            .next()
            .with_context(|| format!("cannot resolve {host}"))?;
        let s = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
            .with_context(|| format!("cannot connect to {host}:{port}"))?;
        s.set_nodelay(true)?;
        s.set_read_timeout(Some(POLL))?;
        let reader = s.try_clone()?;
        Ok(Self::start(Box::new(s), reader, format!("{host}:{port}"), events))
    }

    fn start(
        writer: Box<dyn Write + Send>,
        mut reader: impl Read + Send + 'static,
        description: String,
        events: flume::Sender<LinkEvent>,
    ) -> Link {
        let stop = Arc::new(AtomicBool::new(false));
        let (stop2, ev) = (stop.clone(), events.clone());
        std::thread::Builder::new()
            .name("nexarm-rx".into())
            .spawn(move || {
                let mut dec = Decoder::default();
                let mut buf = [0u8; 2048];
                let reason = loop {
                    if stop2.load(Ordering::Relaxed) {
                        break None;
                    }
                    match reader.read(&mut buf) {
                        Ok(0) => break Some("connection closed by device".to_string()),
                        Ok(n) => {
                            let chunk = &buf[..n];
                            if let Some(t) = printable_text(chunk) {
                                let _ = ev.send(LinkEvent::Text(t));
                            }
                            dec.push(chunk);
                            while let Some(d) = dec.next_decoded() {
                                let _ = ev.send(match d {
                                    Decoded::Frame(f) => LinkEvent::Frame(f),
                                    Decoded::BadChecksum(raw) => LinkEvent::BadFrame(raw),
                                });
                            }
                        }
                        Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                        Err(e) => break Some(e.to_string()),
                    }
                };
                let reason = if stop2.load(Ordering::Relaxed) { None } else { reason };
                let _ = ev.send(LinkEvent::Closed(reason));
            })
            .expect("spawn reader thread");
        Link { writer: Arc::new(Mutex::new(writer)), stop, events, description }
    }

    pub fn send(&self, req: &Request) -> anyhow::Result<()> {
        let bytes = req.bytes();
        {
            let mut w = self.writer.lock().map_err(|_| anyhow::anyhow!("writer poisoned"))?;
            w.write_all(&bytes)?;
            w.flush()?;
        }
        let _ = self.events.send(LinkEvent::Tx(bytes));
        Ok(())
    }

    pub fn close(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Response, cmd, encode, parse};
    use std::net::TcpListener;

    #[test]
    fn tcp_roundtrip_reports_frames_text_and_close() {
        let srv = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = srv.local_addr().unwrap().port();
        let (tx, rx) = flume::unbounded();
        let link = Link::open_tcp("127.0.0.1", port, tx).unwrap();
        let (mut peer, _) = srv.accept().unwrap();

        link.send(&Request::sys(cmd::CHECK_BAT_LEVEL, [])).unwrap();
        let mut got = [0u8; 6];
        peer.read_exact(&mut got).unwrap();
        assert_eq!(got, [0xFF, 0xFF, 0xFF, 0x02, 0x02, 0xFC]);

        peer.write_all(b"boot ok\n").unwrap();
        // Text is only shown for chunks without frame bytes; keep the writes apart.
        std::thread::sleep(Duration::from_millis(50));
        peer.write_all(&encode(0xFF, cmd::CHECK_BAT_LEVEL, &[0x10, 0x27])).unwrap();
        drop(peer);

        let evs: Vec<LinkEvent> = rx.iter().take_while(|e| !matches!(e, LinkEvent::Closed(_))).collect();
        assert!(evs.iter().any(|e| matches!(e, LinkEvent::Tx(_))));
        assert!(evs.iter().any(|e| matches!(e, LinkEvent::Text(t) if t.contains("boot ok"))));
        assert!(evs.iter().any(|e| matches!(e, LinkEvent::Frame(f) if parse(f) == Some(Response::Battery(10000)))));
    }
}

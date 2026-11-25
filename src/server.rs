//! Built-in LAN server for the phones: the player page, a server-sent-events stream with the
//! game state, and a small JSON API for join / leave / answer. Standard library + tiny_http.

use crate::game::{valid_token, Game};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::UdpSocket;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Request, Response};

const PLAYER_HTML: &str = include_str!("player.html");
const FONT: &[u8] = include_bytes!("../assets/BeVietnamPro-SemiBold.ttf");
const FONT_BOLD: &[u8] = include_bytes!("../assets/BeVietnamPro-Bold.ttf");
const MAX_STREAMS: usize = 100;
const MAX_BODY: usize = 4096;

pub struct Shared {
    pub game: Mutex<Game>,
    cv: Condvar,
    start: Instant,
    streams: AtomicUsize,
}

impl Shared {
    pub fn new(game: Game) -> Arc<Shared> {
        Arc::new(Shared { game: Mutex::new(game), cv: Condvar::new(), start: Instant::now(), streams: AtomicUsize::new(0) })
    }

    pub fn now(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    /// Change the game; wakes every phone stream if something changed.
    pub fn with<R>(&self, f: impl FnOnce(&mut Game, u64) -> R) -> R {
        let now = self.now();
        let mut g = self.game.lock().unwrap();
        let v = g.version;
        let r = f(&mut g, now);
        if g.version != v {
            self.cv.notify_all();
        }
        r
    }
}

/// Start the server on the first free port from 3000 and the game timer thread.
pub fn start(sh: Arc<Shared>) -> Result<u16, String> {
    let t = sh.clone();
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(40));
        t.with(|g, now| g.tick(now));
    });
    for port in 3000..=3010 {
        if let Ok(srv) = tiny_http::Server::http(("0.0.0.0", port)) {
            thread::spawn(move || {
                for req in srv.incoming_requests() {
                    let sh = sh.clone();
                    thread::spawn(move || handle(req, sh));
                }
            });
            return Ok(port);
        }
    }
    Err("ports 3000–3010 are all busy".into())
}

/// This machine's address on the local network (no packet is sent).
pub fn lan_ip() -> Option<String> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then(|| ip.to_string())
}

fn header(k: &str, v: &str) -> Header {
    Header::from_bytes(k, v).unwrap()
}

fn send(req: Request, code: u16, ctype: &str, body: Vec<u8>, cache: &str) {
    let r = Response::from_data(body)
        .with_status_code(code)
        .with_header(header("Content-Type", ctype))
        .with_header(header("Cache-Control", cache));
    let _ = req.respond(r);
}

fn handle(req: Request, sh: Arc<Shared>) {
    let url = req.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    match (req.method(), path) {
        (Method::Get, "/") => send(req, 200, "text/html; charset=utf-8", PLAYER_HTML.into(), "no-cache"),
        (Method::Get, "/font.ttf") => send(req, 200, "font/ttf", FONT.into(), "max-age=86400"),
        (Method::Get, "/font-bold.ttf") => send(req, 200, "font/ttf", FONT_BOLD.into(), "max-age=86400"),
        (Method::Get, "/events") => {
            let token = query.split('&').find_map(|kv| kv.strip_prefix("t=")).unwrap_or("").to_string();
            if !valid_token(&token) {
                return send(req, 400, "text/plain", b"bad token".to_vec(), "no-store");
            }
            if sh.streams.fetch_add(1, Ordering::SeqCst) >= MAX_STREAMS {
                sh.streams.fetch_sub(1, Ordering::SeqCst);
                return send(req, 503, "text/plain", b"busy".to_vec(), "no-store");
            }
            events(req, &sh, &token);
            sh.streams.fetch_sub(1, Ordering::SeqCst);
        }
        (Method::Post, "/api") => api(req, &sh),
        _ => send(req, 404, "text/plain", b"Not found".to_vec(), "no-store"),
    }
}

/// Server-sent events: the phone's view of the game, pushed on every change.
fn events(req: Request, sh: &Shared, token: &str) {
    let mut w = req.into_writer();
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nX-Accel-Buffering: no\r\nConnection: close\r\n\r\nretry: 1500\n\n";
    if w.write_all(head.as_bytes()).and_then(|_| w.flush()).is_err() {
        return;
    }
    sh.with(|g, _| g.connect(token));
    let mut seen = u64::MAX;
    loop {
        let msg = {
            let g = sh.game.lock().unwrap();
            let (g, _) = sh.cv.wait_timeout_while(g, Duration::from_secs(8), |g| g.version == seen).unwrap();
            (g.version != seen).then(|| {
                seen = g.version;
                g.player_view(token, sh.now()).to_string()
            })
        };
        let r = match msg {
            Some(s) => write!(w, "data: {s}\n\n"),
            None => w.write_all(b": ping\n\n"), // keep-alive; also detects phones that left
        };
        if r.and_then(|_| w.flush()).is_err() {
            break;
        }
    }
    sh.with(|g, now| g.disconnect(token, now));
}

fn api(mut req: Request, sh: &Shared) {
    if req.body_length().is_some_and(|n| n > MAX_BODY) {
        return send(req, 413, "text/plain", b"too large".to_vec(), "no-store");
    }
    let mut body = Vec::new();
    let _ = req.as_reader().take(MAX_BODY as u64).read_to_end(&mut body);
    let reply = match serde_json::from_slice::<Value>(&body) {
        Ok(m) => action(&m, sh),
        Err(_) => json!({ "error": "bad request" }),
    };
    send(req, 200, "application/json", reply.to_string().into_bytes(), "no-store");
}

fn action(m: &Value, sh: &Shared) -> Value {
    let token = m["token"].as_str().unwrap_or("");
    if !valid_token(token) {
        return json!({ "error": "bad token" });
    }
    sh.with(|g, now| match m["t"].as_str() {
        Some("join") => {
            let team = m["team"].as_u64().unwrap_or(0) as u32;
            match g.join(token, team, m["avatar"].as_str().unwrap_or("")) {
                Ok(()) => json!({ "ok": true }),
                Err(e) => json!({ "error": e }),
            }
        }
        Some("leave") => {
            g.leave(token);
            json!({ "ok": true })
        }
        Some("answer") => {
            let c = m["choice"].as_u64().unwrap_or(99) as usize;
            json!({ "ok": g.answer(token, c, now) })
        }
        _ => json!({ "error": "unknown action" }),
    })
}

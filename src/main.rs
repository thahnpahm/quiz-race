#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Quiz Race – host screen (projector) with a built-in LAN server for the players' phones.

mod audio;
mod emoji_list;
mod game;
mod server;
mod tunnel;

use audio::Sfx;
use game::{Game, Phase, Quiz, Ranked};
use macroquad::audio::{load_sound_from_bytes, play_sound, play_sound_once, set_sound_volume, stop_sound, PlaySoundParams, Sound};
use macroquad::miniquad;
use macroquad::prelude::*;
use qrcodegen::{QrCode, QrCodeEcc};
use server::Shared;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

const FONT: &[u8] = include_bytes!("../assets/BeVietnamPro-SemiBold.ttf");
const FONT_BOLD: &[u8] = include_bytes!("../assets/BeVietnamPro-Bold.ttf");
const BASE_W: f32 = 1280.0;
/// Text is rasterised only at these sizes (then scaled to the exact size asked for).
const SNAP_SIZES: [f32; 12] = [12.0, 14.0, 16.0, 18.0, 20.0, 22.0, 24.0, 26.0, 30.0, 34.0, 40.0, 46.0];
const COUNTDOWN_SIZE: f32 = 120.0;

fn snap(size: f32) -> f32 {
    SNAP_SIZES.iter().copied().find(|&s| s >= size - 0.5).unwrap_or(size.round())
}

fn raster(size: f32, px: f32) -> u16 {
    (size * px).round().max(1.0) as u16
}

/// Every character the game can show: ASCII, punctuation and everything in the questions file.
fn quiz_chars(quiz: &Quiz) -> Vec<char> {
    let mut chars: Vec<char> = (' '..='~').chain("·—–…×’‘“”".chars()).collect();
    let texts = [quiz.title.as_str(), quiz.subtitle.as_str()].into_iter().chain(
        quiz.parts.iter().flat_map(|p| std::iter::once(p.title.as_str()).chain(p.questions.iter().flat_map(|q| std::iter::once(q.q.as_str()).chain(q.options.iter().map(|o| o.as_str()))))),
    );
    chars.extend(texts.flat_map(|t| t.chars()));
    chars.sort_unstable();
    chars.dedup();
    chars
}

/// One font object per (weight, pixel size), with its glyphs rendered up front.
/// macroquad keeps one glyph atlas per font and its atlas can blow up (black boxes, or a
/// crash) when it has to grow mid-game with mixed sizes; one small atlas per size avoids that.
fn make_font(bold: bool, raster_px: u16, chars: &[char]) -> Font {
    let f = load_ttf_font_from_bytes(if bold { FONT_BOLD } else { FONT }).unwrap();
    if raster_px > 100 {
        f.populate_font_cache(&('0'..='9').collect::<Vec<_>>(), raster_px); // countdown
    } else {
        f.populate_font_cache(chars, raster_px);
    }
    f
}
const BASE_H: f32 = 720.0;
const PAD: f32 = 22.0;

fn hex(h: u32) -> Color {
    Color::from_rgba((h >> 16) as u8, (h >> 8) as u8, h as u8, 255)
}

// ---------- colours: solid, no gradients ----------

#[derive(Clone, Copy)]
struct Theme {
    bg: Color,
    card: Color,
    cardb: Color,
    accent: Color,
}

fn theme(part: Option<usize>) -> Theme {
    let t = |bg, card, cardb, accent| Theme { bg: hex(bg), card: hex(card), cardb: hex(cardb), accent: hex(accent) };
    match part.map(|p| p % 3) {
        None => t(0x3a1c71, 0x563c85, 0x654d91, 0xffd23f),
        Some(0) => t(0x20222e, 0x3f414b, 0x4f5159, 0xff4d3d),
        Some(1) => t(0x0a5fa8, 0x2c75b4, 0x3d81bb, 0xffe066),
        _ => t(0x2e5b3f, 0x4b725a, 0x5a7e67, 0xffd166),
    }
}

fn mix(a: Color, b: Color, k: f32) -> Color {
    Color::new(a.r + (b.r - a.r) * k, a.g + (b.g - a.g) * k, a.b + (b.b - a.b) * k, a.a + (b.a - a.a) * k)
}

const TEAM_COLORS: [u32; 12] = [0xff595e, 0xff924c, 0xffca3a, 0x8ac926, 0x2ec4b6, 0x4cc9f0, 0x9d7bff, 0xf15bb5, 0xe0e0e0, 0xa0e426, 0xff8fab, 0xb8f2e6];
const OPT_COLORS: [u32; 4] = [0xe21b3c, 0x1368ce, 0xd89e00, 0x26890c];
const LETTERS: [&str; 4] = ["A", "B", "C", "D"];
const DARK: Color = Color::new(0.106, 0.106, 0.106, 1.0);

struct PartTheme {
    name: &'static str,
    emoji: &'static str,
    finish: &'static str,
    pre: &'static str,
}
const PARTS: [PartTheme; 3] = [
    PartTheme { name: "Grand Prix", emoji: "🏎️", finish: "", pre: "" },
    PartTheme { name: "Splash Cup", emoji: "🏊", finish: "🏅", pre: "💦" },
    PartTheme { name: "Summit Trek", emoji: "🥾", finish: "🏔️", pre: "🎒" },
];
fn pt(part: usize) -> &'static PartTheme {
    &PARTS[part % 3]
}
fn cap(part: usize) -> String {
    format!("Part {} · The {}", part + 1, pt(part).name)
}
fn fmt_n(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}
fn ease(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    1.0 - (1.0 - x).powi(3)
}

// ---------- drawing helpers ----------

struct Ui {
    s: f32,
    px: f32,
    vw: f32,
    vh: f32,
    fonts: std::cell::RefCell<HashMap<(bool, u16), Font>>,
    chars: Vec<char>,
    emoji: HashMap<&'static str, Texture2D>,
    alpha: f32,
    mouse: Vec2,
    click: bool,
    th: Theme,
    t: f32,
    oy: f32,
}

impl Ui {
    fn c(&self, c: Color) -> Color {
        Color { a: c.a * self.alpha, ..c }
    }
    fn rect(&self, x: f32, y: f32, w: f32, h: f32, c: Color) {
        draw_rectangle(x, y, w, h, self.c(c));
    }
    /// Rounded rectangle as one triangle fan (no overlapping parts, so fading stays clean).
    fn rrect(&self, x: f32, y: f32, w: f32, h: f32, r: f32, c: Color) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let r = r.min(w / 2.0).min(h / 2.0);
        let col = self.c(c);
        let mut v = vec![Vertex::new(x + w / 2.0, y + h / 2.0, 0.0, 0.0, 0.0, col)];
        let corners = [(x + w - r, y + h - r, 0.0f32), (x + r, y + h - r, 90.0), (x + r, y + r, 180.0), (x + w - r, y + r, 270.0)];
        for (cx, cy, a0) in corners {
            for k in 0..=6 {
                let a = (a0 + k as f32 * 15.0).to_radians();
                v.push(Vertex::new(cx + r * a.cos(), cy + r * a.sin(), 0.0, 0.0, 0.0, col));
            }
        }
        let n = v.len() as u16;
        let mut idx = Vec::with_capacity(3 * n as usize);
        for i in 1..n {
            idx.extend_from_slice(&[0, i, if i + 1 < n { i + 1 } else { 1 }]);
        }
        draw_mesh(&Mesh { vertices: v, indices: idx, texture: None });
    }
    fn card(&self, x: f32, y: f32, w: f32, h: f32) {
        self.rrect(x, y, w, h, 20.0, self.th.cardb);
        self.rrect(x + 2.0, y + 2.0, w - 4.0, h - 4.0, 18.0, self.th.card);
    }
    /// Tick mark (the font has no ✓).
    fn check(&self, cx: f32, cy: f32, size: f32) {
        let c = self.c(WHITE);
        let (a, b, e) = (vec2(cx - size * 0.4, cy), vec2(cx - size * 0.1, cy + size * 0.3), vec2(cx + size * 0.42, cy - size * 0.32));
        draw_line(a.x, a.y, b.x, b.y, size * 0.16, c);
        draw_line(b.x, b.y, e.x, e.y, size * 0.16, c);
        draw_circle(b.x, b.y, size * 0.08, c);
    }
    fn circle(&self, x: f32, y: f32, r: f32, c: Color) {
        draw_circle(x, y, r, self.c(c));
    }
    /// Font for this size: (font, raster pixel size, scale that gives exactly `size`).
    fn font(&self, size: f32, bold: bool) -> (Font, u16, f32) {
        let r = raster(snap(size), self.px);
        let f = self.fonts.borrow_mut().entry((bold, r)).or_insert_with(|| make_font(bold, r, &self.chars)).clone();
        (f, r, size / r as f32)
    }
    fn tw(&self, t: &str, size: f32, bold: bool) -> f32 {
        let (f, r, sc) = self.font(size, bold);
        measure_text(t, Some(&f), r, sc).width
    }
    /// Text vertically centred on `cy`; `align` 0 = left, 0.5 = centre, 1 = right of `x`.
    fn text(&self, t: &str, x: f32, cy: f32, size: f32, c: Color, bold: bool, align: f32) {
        self.text_zoom(t, x, cy, size, 1.0, c, bold, align);
    }
    /// Text drawn `zoom` times bigger without rasterising a new font size (for animations).
    #[allow(clippy::too_many_arguments)]
    fn text_zoom(&self, t: &str, x: f32, cy: f32, size: f32, zoom: f32, c: Color, bold: bool, align: f32) {
        let (f, r, sc) = self.font(size, bold);
        let p = TextParams { font: Some(&f), font_size: r, font_scale: sc * zoom, color: self.c(c), ..Default::default() };
        let w = if align > 0.0 { measure_text(t, p.font, p.font_size, p.font_scale).width } else { 0.0 };
        draw_text_ex(t, x - w * align, cy + size * zoom * 0.36, p);
    }
    fn wrap(&self, t: &str, maxw: f32, size: f32, bold: bool) -> Vec<String> {
        let mut lines = vec![];
        let mut cur = String::new();
        for word in t.split_whitespace() {
            let next = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
            if self.tw(&next, size, bold) > maxw && !cur.is_empty() {
                lines.push(std::mem::replace(&mut cur, word.to_string()));
            } else {
                cur = next;
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        lines
    }
    /// Shorten with "…" to fit.
    fn fit(&self, t: &str, maxw: f32, size: f32, bold: bool) -> String {
        if self.tw(t, size, bold) <= maxw {
            return t.to_string();
        }
        let mut s: String = t.to_string();
        while !s.is_empty() && self.tw(&format!("{s}…"), size, bold) > maxw {
            s.pop();
        }
        format!("{}…", s.trim_end())
    }
    fn emo(&self, e: &str, cx: f32, cy: f32, size: f32) {
        if let Some(t) = self.emoji.get(e) {
            draw_texture_ex(t, cx - size / 2.0, cy - size / 2.0, self.c(WHITE), DrawTextureParams { dest_size: Some(vec2(size, size)), ..Default::default() });
        }
    }
    fn badge(&self, avatar: &str, id_color: Color, cx: f32, cy: f32, d: f32) {
        self.circle(cx, cy, d / 2.0, id_color);
        self.circle(cx, cy, d / 2.0 - (d * 0.08).max(2.0), WHITE);
        self.emo(avatar, cx, cy, d * 0.62);
    }
    fn pill(&self, x: f32, cy: f32, emoji: &str, label: &str, size: f32, align: f32) -> f32 {
        let ew = if emoji.is_empty() { 0.0 } else { size * 1.2 + 6.0 };
        let w = ew + self.tw(label, size, true) + 28.0;
        let x = x - w * align;
        let h = size * 1.9;
        self.rrect(x, cy - h / 2.0, w, h, h / 2.0, self.th.cardb);
        if !emoji.is_empty() {
            self.emo(emoji, x + 14.0 + size * 0.6, cy, size * 1.2);
        }
        self.text(label, x + 14.0 + ew, cy, size, WHITE, true, 0.0);
        w
    }
    fn hover(&self, x: f32, y: f32, w: f32, h: f32) -> bool {
        let m = self.mouse - vec2(0.0, self.oy);
        m.x >= x && m.x <= x + w && m.y >= y && m.y <= y + h
    }
    /// Button; returns true when clicked. `primary` = accent colour.
    fn button(&self, label: &str, x: f32, y: f32, w: f32, h: f32, primary: bool, enabled: bool) -> bool {
        let hov = enabled && self.hover(x, y, w, h);
        let (bg, fg) = if primary { (self.th.accent, DARK) } else { (self.th.cardb, WHITE) };
        let bg = if hov { mix(bg, WHITE, 0.15) } else { bg };
        let a = if enabled { 1.0 } else { 0.45 };
        let lift = if primary && hov && is_mouse_button_down(MouseButton::Left) { 3.0 } else { 0.0 };
        if primary {
            self.rrect(x, y + 5.0, w, h, 14.0, Color { a, ..mix(bg, BLACK, 0.35) });
        }
        self.rrect(x, y + lift, w, h, 14.0, Color { a, ..bg });
        self.text(label, x + w / 2.0, y + lift + h / 2.0, if primary { 20.0 } else { 16.0 }, Color { a, ..fg }, true, 0.5);
        hov && self.click
    }
}

// ---------- app ----------

enum Act {
    Start,
    Skip,
    Next,
    Auto(bool),
    Kick(u32),
    Reset,
}

/// Background music: the lobby loop, plus a separate loop for the quiz (built-in music only;
/// your own song plays all game long).
struct Music {
    lobby: Sound,
    quiz: Option<Sound>,
    vol: (f32, f32),
}

/// Sounds prepared off the main thread.
enum Load {
    Sfx(Vec<(u8, Vec<u8>)>),
    Music { lobby: Vec<u8>, quiz: Option<Vec<u8>>, err: Option<String> },
}

const MUSIC_EXTS: [&str; 6] = ["mp3", "m4a", "aac", "ogg", "flac", "wav"];

fn music_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Quiz Race"))
}

/// The song chosen with "Music…", kept as %APPDATA%\Quiz Race\music.<ext>.
fn own_music() -> Option<PathBuf> {
    let d = music_dir()?;
    MUSIC_EXTS.iter().map(|e| d.join(format!("music.{e}"))).find(|p| p.is_file())
}

/// Prepare the background music (own song if there is one, else the built-in loops).
fn load_music(tx: mpsc::Sender<Load>) {
    std::thread::spawn(move || {
        let synth = || (audio::wav(&audio::music_lobby()), Some(audio::wav(&audio::music_quiz())));
        let msg = match own_music().map(|p| audio::decode_song(&p)) {
            Some(Ok(song)) => Load::Music { lobby: song, quiz: None, err: None },
            Some(Err(e)) => {
                let (lobby, quiz) = synth();
                Load::Music { lobby, quiz, err: Some(format!("Music file not used: {e}")) }
            }
            None => {
                let (lobby, quiz) = synth();
                Load::Music { lobby, quiz, err: None }
            }
        };
        let _ = tx.send(msg);
    });
}

struct Confetti {
    p: Vec2,
    v: Vec2,
    rot: f32,
    spin: f32,
    color: Color,
}

struct Floater {
    team: u32,
    text: String,
    born: f32,
}

struct App {
    sh: Arc<Shared>,
    lan_url: Option<String>,
    port: u16,
    tunnel: Option<tunnel::Tunnel>,
    reload_fonts: bool,
    server_err: Option<String>,
    qr: Option<(String, QrCode)>,
    quiz_src: String,
    quiz_err: Option<String>,
    sfx: Option<HashMap<u8, Sound>>,
    music: Option<Music>,
    own_music: bool,
    loads: (mpsc::Sender<Load>, mpsc::Receiver<Load>),
    muted: bool,
    fullscreen: bool,
    lanes: HashMap<u32, f32>,
    lane_pos: HashMap<u32, Vec2>,
    lane_part: usize,
    screen: (Phase, usize, usize),
    screen_t: f32,
    prev: (Phase, usize, usize, usize, u64, u64),
    confetti: Vec<Confetti>,
    floaters: Vec<Floater>,
    reset_confirm: f32,
    kick_confirm: Option<(u32, f32)>,
    toast: Option<(String, f32)>,
    th: Theme,
}

fn quiz_file() -> Option<PathBuf> {
    std::env::args_os().nth(1).map(PathBuf::from).or_else(|| std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Quiz Race").join("questions.json")))
}

/// Questions: file given on the command line, else %APPDATA%\Quiz Race\questions.json, else built-in.
fn load_quiz() -> (Quiz, String, Option<String>) {
    match quiz_file().filter(|p| p.exists()) {
        Some(p) => match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|s| Quiz::parse(&s)) {
            Ok(q) => (q, p.file_name().unwrap_or_default().to_string_lossy().into_owned(), None),
            Err(e) => (Quiz::builtin(), "built-in".into(), Some(format!("questions.json not used: {e}"))),
        },
        None => (Quiz::builtin(), "built-in".into(), None),
    }
}

fn sfx_id(s: Sfx) -> u8 {
    s as u8
}

impl App {
    fn now(&self) -> u64 {
        self.sh.now()
    }

    fn play(&self, s: Sfx) {
        if let (Some(sfx), false) = (&self.sfx, self.muted) {
            if let Some(x) = sfx.get(&sfx_id(s)) {
                play_sound_once(x);
            }
        }
    }

    fn join_url(&self) -> String {
        match self.tunnel.as_ref().map(|t| t.state()) {
            Some(tunnel::State::Ready(u)) => format!("{u}/"),
            _ => self.lan_url.clone().unwrap_or_else(|| format!("http://localhost:{}/", self.port)),
        }
    }

    fn apply(&mut self, acts: Vec<Act>) {
        for a in acts {
            match a {
                Act::Reset => {
                    let (q, src, err) = load_quiz();
                    self.quiz_src = src;
                    self.quiz_err = err;
                    self.sh.with(|g, _| g.reset(Some(q)));
                    self.reload_fonts = true; // the new questions may use new characters
                    self.lanes.clear();
                    self.toast("Game reset — scores cleared, questions reloaded");
                }
                a => self.sh.with(|g, now| match a {
                    Act::Start => g.start(now),
                    Act::Skip => g.skip(now),
                    Act::Next => g.next(now),
                    Act::Auto(v) => g.set_auto(v, now),
                    Act::Kick(id) => g.kick(id, now),
                    Act::Reset => {}
                }),
            }
        }
    }

    fn toast(&mut self, msg: &str) {
        self.toast = Some((msg.to_string(), get_time() as f32));
    }

    /// "Music…": pick a song file; it is copied next to the questions file and played.
    fn pick_music(&mut self) {
        let Some(src) = rfd::FileDialog::new().set_title("Choose a song for the game").add_filter("Music", &MUSIC_EXTS).pick_file() else { return };
        let Some(dir) = music_dir() else { return };
        let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("mp3").to_lowercase();
        let _ = std::fs::create_dir_all(&dir);
        for e in MUSIC_EXTS {
            let _ = std::fs::remove_file(dir.join(format!("music.{e}")));
        }
        match std::fs::copy(&src, dir.join(format!("music.{ext}"))) {
            Ok(_) => {
                self.toast("Loading your music…");
                load_music(self.loads.0.clone());
            }
            Err(e) => self.toast(&format!("Couldn't copy the song: {e}")),
        }
    }

    /// Back to the built-in music.
    fn remove_music(&mut self) {
        if let Some(dir) = music_dir() {
            for e in MUSIC_EXTS {
                let _ = std::fs::remove_file(dir.join(format!("music.{e}")));
            }
        }
        self.toast("Back to the built-in music");
        load_music(self.loads.0.clone());
    }

    fn open_questions_file(&mut self) {
        let Some(p) = quiz_file() else { return };
        if !p.exists() {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(&p, game::BUILTIN_QUIZ);
        }
        #[cfg(windows)]
        let _ = std::process::Command::new("explorer").arg(format!("/select,{}", p.display())).spawn();
        self.toast("Edit questions.json, then press Reset game to load it");
    }


    /// Sounds, music, confetti and score pop-ups triggered by game changes.
    fn react(&mut self, g: &Game, now: u64) {
        let answered = g.teams.keys().filter(|&&id| g.answered(id)).count();
        let cd = if g.phase == Phase::Countdown { g.cd_end.saturating_sub(now).div_ceil(1000).min(3) } else { 0 };
        let qs = if g.phase == Phase::Question { g.q_end.saturating_sub(now).div_ceil(1000) } else { 0 };
        let cur = (g.phase, g.part, g.qi, answered, cd, qs);
        let prev = std::mem::replace(&mut self.prev, cur);
        let t = get_time() as f32;
        if cur.0 != prev.0 {
            match g.phase {
                Phase::Countdown | Phase::Waiting => self.play(Sfx::Whoosh),
                Phase::Question => self.play(Sfx::Go),
                Phase::Reveal => {
                    let r = g.reveal.as_ref().unwrap();
                    self.play(if r.timed_out && !r.results.values().any(|x| x.correct) { Sfx::TimeUp } else { Sfx::Correct });
                    for (id, x) in &r.results {
                        if x.points > 0 {
                            self.floaters.push(Floater { team: *id, text: format!("+{}", fmt_n(x.points)), born: t });
                        }
                    }
                }
                Phase::Results => {
                    self.play(Sfx::Fanfare);
                    self.burst(60);
                }
                Phase::Final => {
                    self.play(Sfx::Fanfare);
                    self.burst(220);
                }
            }
        } else if g.phase == Phase::Question && cur.2 != prev.2 {
            self.play(Sfx::Go);
        } else if g.phase == Phase::Question && answered > prev.3 {
            self.play(Sfx::Blip);
        }
        if (cd != prev.4 && cd > 0) || (qs != prev.5 && qs > 0 && qs <= 5 && g.phase == Phase::Question && prev.0 == Phase::Question) {
            self.play(Sfx::Tick);
        }
        // music: cheerful loop in the lobby / results, driving loop while quizzing
        if let Some(m) = &mut self.music {
            let quiz = matches!(g.phase, Phase::Countdown | Phase::Question | Phase::Reveal);
            let target = match (self.muted, &m.quiz, quiz) {
                (true, _, _) => (0.0, 0.0),
                (_, Some(_), true) => (0.0, 0.32),
                (_, None, true) => (0.25, 0.0), // own song: a little softer while people think
                _ => (0.38, 0.0),
            };
            let k = (get_frame_time() * 2.5).min(1.0);
            m.vol.0 += (target.0 - m.vol.0) * k;
            m.vol.1 += (target.1 - m.vol.1) * k;
            set_sound_volume(&m.lobby, m.vol.0);
            if let Some(q) = &m.quiz {
                set_sound_volume(q, m.vol.1);
            }
        }
    }

    fn burst(&mut self, n: usize) {
        let w = screen_width();
        for i in 0..n {
            let x = rand::gen_range(0.0, 1.0);
            self.confetti.push(Confetti {
                p: vec2(x * w / self.scale(), -20.0 - rand::gen_range(0.0, 200.0)),
                v: vec2(rand::gen_range(-60.0, 60.0), rand::gen_range(60.0, 240.0)),
                rot: rand::gen_range(0.0, std::f32::consts::TAU),
                spin: rand::gen_range(-6.0, 6.0),
                color: hex(TEAM_COLORS[i % TEAM_COLORS.len()]),
            });
        }
    }

    fn scale(&self) -> f32 {
        (screen_width() / BASE_W).min(screen_height() / BASE_H)
    }
}

fn team_color(g: &Game, id: u32) -> Color {
    let i = g.quiz.teams.iter().position(|&t| t == id).unwrap_or(8);
    hex(TEAM_COLORS[i % TEAM_COLORS.len()])
}

// ---------- screens ----------

fn draw_header(ui: &Ui, g: &Game, now: u64) {
    let cy = 42.0;
    ui.emo("🏁", PAD + 18.0, cy, 36.0);
    let title = ui.fit(&g.quiz.title, 330.0, 26.0, true);
    ui.text(&title, PAD + 46.0, cy, 26.0, WHITE, true, 0.0);
    let left_end = PAD + 46.0 + ui.tw(&title, 26.0, true) + 20.0;
    let lobby = g.phase == Phase::Final || (g.part == 0 && g.phase == Phase::Waiting);
    let (em, label) = if g.phase == Phase::Final {
        ("🏆", "Final results".to_string())
    } else if lobby {
        ("🎉", "Lobby".to_string())
    } else {
        (pt(g.part).emoji, format!("{} — {}", cap(g.part), g.quiz.parts[g.part].title))
    };
    let side = left_end.max(130.0);
    let label = ui.fit(&label, ui.vw - 2.0 * side - 70.0, 20.0, true);
    ui.pill(ui.vw / 2.0, cy, em, &label, 20.0, 0.5);
    if g.phase == Phase::Question {
        let left = g.q_end.saturating_sub(now).div_ceil(1000);
        let (w, h) = (96.0, 54.0);
        let x = ui.vw - PAD - w;
        let urgent = left <= 5 && (ui.t * 4.0).fract() < 0.5;
        ui.rrect(x, cy - h / 2.0, w, h, 16.0, if urgent { WHITE } else { ui.th.accent });
        ui.text(&left.to_string(), x + w / 2.0, cy, 40.0, DARK, true, 0.5);
    }
}

fn lobby_slots(ui: &Ui, g: &Game, x: f32, y: f32, w: f32, h: f32, kick: &Option<(u32, f32)>, acts: &mut Vec<Act>, kick_req: &mut Option<u32>) {
    let n = g.quiz.teams.len();
    let cols = 3;
    let rows = n.div_ceil(cols);
    let gap = 14.0;
    let sw = (w - gap * (cols as f32 - 1.0)) / cols as f32;
    let sh = ((h - gap * (rows as f32 - 1.0)) / rows as f32).min(150.0);
    for (i, &id) in g.quiz.teams.iter().enumerate() {
        let sx = x + (i % cols) as f32 * (sw + gap);
        let sy = y + (i / cols) as f32 * (sh + gap);
        let c = team_color(g, id);
        match g.teams.get(&id) {
            Some(t) => {
                let on = g.online(id);
                ui.rrect(sx, sy, sw, sh, 20.0, c);
                ui.rrect(sx + 3.0, sy + 3.0, sw - 6.0, sh - 6.0, 18.0, ui.th.card);
                let d = (sh * 0.45).min(64.0);
                let pop = 1.0 + 0.06 * (ui.t * 3.0 + id as f32).sin();
                ui.badge(&t.avatar, if on { c } else { GRAY }, sx + sw / 2.0, sy + sh * 0.36, d * pop);
                ui.text(&format!("Group {id}"), sx + sw / 2.0, sy + sh * 0.72, 20.0, WHITE, true, 0.5);
                ui.text(if on { "ready" } else { "offline" }, sx + sw / 2.0, sy + sh * 0.88, 14.0, Color::new(1.0, 1.0, 1.0, 0.75), false, 0.5);
                let confirming = kick.is_some_and(|(k, until)| k == id && ui.t < until);
                let (bw, bx) = if confirming { (86.0, sx + sw - 92.0) } else { (28.0, sx + sw - 34.0) };
                let hov = ui.hover(bx, sy + 6.0, bw, 28.0);
                ui.rrect(bx, sy + 6.0, bw, 28.0, 14.0, if confirming { hex(0xc0392b) } else if hov { ui.th.cardb } else { ui.th.bg });
                ui.text(if confirming { "Remove?" } else { "×" }, bx + bw / 2.0, sy + 20.0, 14.0, WHITE, true, 0.5);
                if hov && ui.click {
                    if confirming {
                        acts.push(Act::Kick(id));
                    } else {
                        *kick_req = Some(id);
                    }
                }
            }
            None => {
                ui.rrect(sx, sy, sw, sh, 20.0, ui.th.cardb);
                ui.rrect(sx + 2.0, sy + 2.0, sw - 4.0, sh - 4.0, 18.0, ui.th.bg);
                let a = Color::new(1.0, 1.0, 1.0, 0.45);
                ui.text(&format!("Group {id}"), sx + sw / 2.0, sy + sh * 0.45, 20.0, a, true, 0.5);
                ui.text("waiting…", sx + sw / 2.0, sy + sh * 0.65, 14.0, a, false, 0.5);
            }
        }
    }
}

fn draw_qr(ui: &Ui, qr: &QrCode, cx: f32, y: f32, size: f32) {
    ui.rrect(cx - size / 2.0, y, size, size, 14.0, WHITE);
    let n = qr.size();
    let m = (size - 20.0) / n as f32;
    let x0 = cx - m * n as f32 / 2.0;
    let y0 = y + 10.0;
    for yy in 0..n {
        for xx in 0..n {
            if qr.get_module(xx, yy) {
                ui.rect(x0 + xx as f32 * m, y0 + yy as f32 * m, m + 0.3, m + 0.3, DARK);
            }
        }
    }
}

/// Leaderboard table. `cols`: (header, width, value).
#[allow(clippy::type_complexity)]
fn table(ui: &Ui, g: &Game, rows: &[Ranked], x: f32, y: f32, w: f32, row_h: f32, cols: &[(&str, f32, &dyn Fn(&Ranked) -> String)]) {
    let fs = (row_h * 0.48).clamp(13.0, 20.0);
    let hy = y + 10.0;
    ui.text("#", x + 26.0, hy, 12.0, Color::new(1.0, 1.0, 1.0, 0.7), true, 0.5);
    ui.text("TEAM", x + 62.0, hy, 12.0, Color::new(1.0, 1.0, 1.0, 0.7), true, 0.0);
    let mut cx = x + w;
    let mut col_x = vec![];
    for (_, cw, _) in cols.iter().rev() {
        cx -= cw;
        col_x.push(cx + cw / 2.0);
    }
    col_x.reverse();
    for (i, (h, _, _)) in cols.iter().enumerate() {
        ui.text(&h.to_uppercase(), col_x[i], hy, 12.0, Color::new(1.0, 1.0, 1.0, 0.7), true, 0.5);
    }
    for (i, r) in rows.iter().enumerate() {
        let ry = y + 22.0 + i as f32 * (row_h + 5.0);
        ui.rrect(x, ry, w, row_h, 12.0, ui.th.cardb);
        let cy = ry + row_h / 2.0;
        match r.rank {
            1..=3 => ui.emo(["🥇", "🥈", "🥉"][r.rank - 1], x + 26.0, cy, row_h * 0.75),
            k => ui.text(&k.to_string(), x + 26.0, cy, fs, WHITE, true, 0.5),
        }
        if let Some(t) = g.teams.get(&r.id) {
            ui.badge(&t.avatar, team_color(g, r.id), x + 66.0, cy, row_h * 0.78);
        }
        ui.text(&format!("Group {}", r.id), x + 66.0 + row_h * 0.5 + 8.0, cy, fs, WHITE, true, 0.0);
        for (i, (_, _, f)) in cols.iter().enumerate() {
            ui.text(&f(r), col_x[i], cy, fs, WHITE, i + 1 == cols.len(), 0.5);
        }
    }
}

fn track_height(ui: &Ui, n: usize) -> f32 {
    (n as f32 * 32.0 + 24.0).clamp(110.0, (ui.vh * 0.34).max(140.0))
}

fn draw_track(ui: &Ui, g: &Game, app_lanes: &mut HashMap<u32, f32>, lane_pos: &mut HashMap<u32, Vec2>, x: f32, y: f32, w: f32, h: f32) {
    ui.card(x, y, w, h);
    let ids: Vec<u32> = g.quiz.teams.iter().copied().filter(|id| g.teams.contains_key(id)).collect();
    if ids.is_empty() {
        ui.text("No teams yet", x + w / 2.0, y + h / 2.0, 18.0, WHITE, true, 0.5);
        return;
    }
    let part = g.part;
    let max = g.max_per_part(part) as f32;
    let n = ids.len() as f32;
    let gap = 5.0;
    let lh = ((h - 20.0 - gap * (n - 1.0)) / n).min(56.0);
    let top = y + (h - (lh * n + gap * (n - 1.0))) / 2.0;
    let dt = get_frame_time();
    let theme = part % 3;
    for (i, id) in ids.iter().enumerate() {
        let t = &g.teams[id];
        let c = team_color(g, *id);
        let ly = top + i as f32 * (lh + gap);
        let cy = ly + lh / 2.0;
        let on = g.online(*id);
        // label
        let lx = x + 14.0;
        ui.rrect(lx, cy - 12.0, 44.0, 24.0, 8.0, c);
        ui.text(&format!("G{id}"), lx + 22.0, cy, 15.0, DARK, true, 0.5);
        ui.text(&fmt_n(t.scores[part]), lx + 112.0, cy, 14.0, WHITE, true, 1.0);
        // run
        let rx = lx + 122.0;
        let rw = x + w - 14.0 - rx;
        let lane_bg = [hex(0x2d303c), hex(0x3ab8e4), hex(0x6a8f4e)][theme];
        ui.rrect(rx, ly, rw, lh, 10.0, lane_bg);
        match theme {
            0 => {
                ui.rect(rx + 6.0, ly, rw - 12.0, 3.0, hex(0xff4d3d));
                ui.rect(rx + 6.0, ly + lh - 3.0, rw - 12.0, 3.0, WHITE);
                let mut dx = rx + 10.0;
                while dx < rx + rw - 40.0 {
                    ui.rect(dx, cy - 1.5, 16.0, 3.0, hex(0x6b6e7a));
                    dx += 28.0;
                }
                // chequered finish
                let sq = (lh / 6.0).max(4.0);
                let fx = rx + rw - 3.0 * sq - 4.0;
                for yy in 0..6 {
                    for xx in 0..3 {
                        let fill = if (xx + yy) % 2 == 0 { BLACK } else { WHITE };
                        ui.rect(fx + xx as f32 * sq, ly + yy as f32 * (lh / 6.0), sq, lh / 6.0, fill);
                    }
                }
            }
            1 => {
                ui.rect(rx + 6.0, ly, rw - 12.0, 4.0, hex(0xff6b6b));
                ui.rect(rx + 6.0, ly + lh - 4.0, rw - 12.0, 4.0, WHITE);
                let off = (ui.t * 40.0) % 60.0;
                let mut dx = rx + 8.0 + off;
                while dx < rx + rw - 50.0 {
                    ui.rrect(dx, cy + 6.0 * ((dx * 0.05).sin()), 22.0, 3.0, 1.5, hex(0x7fd3f2));
                    dx += 60.0;
                }
                ui.rrect(rx + rw - 30.0, ly + 4.0, 30.0, lh - 8.0, 4.0, hex(0xe8f6fc));
                ui.emo(pt(1).finish, rx + rw - 15.0, cy, lh * 0.55);
            }
            _ => {
                let mut dx = rx + 10.0;
                while dx < rx + rw - 40.0 {
                    ui.circle(dx, cy + lh * 0.08, 1.8, hex(0xc9dbb8));
                    dx += 12.0;
                }
                ui.emo(pt(2).finish, rx + rw - 18.0, cy, lh * 0.8);
            }
        }
        // mover glides to its score
        let target = (t.scores[part] as f32 / max).min(1.0);
        let p = app_lanes.entry(*id).or_insert(0.0);
        *p += (target - *p) * (dt * 2.2).min(1.0);
        let mv = (lh * 0.78).min(40.0);
        let mw = if theme == 0 { mv * 1.9 } else { mv * 1.7 };
        let mx = rx + 4.0 + (rw - mw - 40.0) * *p;
        let bob = match theme {
            0 => (ui.t * 40.0 + *id as f32).sin() * 1.2,
            1 => (ui.t * 6.0 + *id as f32).sin() * 3.0,
            _ => (ui.t * 9.0 + *id as f32).sin().abs() * -3.0,
        };
        let my = cy + bob;
        let a = if on { 1.0 } else { 0.5 };
        if theme == 0 {
            // kart: body, wheels, headlight, driver
            let (kw, kh) = (mv * 1.9, mv);
            let bx = mx;
            let by = my - kh / 2.0;
            ui.rrect(bx, by + kh * 0.40, kw, kh * 0.44, 8.0, Color { a, ..c });
            ui.rect(bx + 6.0, by + kh * 0.74, kw - 12.0, kh * 0.08, Color { a, ..mix(c, BLACK, 0.3) });
            ui.circle(bx + kw - 2.0, by + kh * 0.55, 3.0, hex(0xffe27a));
            for wx in [bx + kw * 0.22, bx + kw * 0.78] {
                ui.circle(wx, by + kh * 0.86, kh * 0.17, hex(0x888888));
                ui.circle(wx, by + kh * 0.86, kh * 0.12, hex(0x111111));
            }
            ui.badge(&t.avatar, c, bx + kw * 0.45, by + kh * 0.30, mv * 0.72);
            lane_pos.insert(*id, vec2(bx + kw / 2.0, ly));
        } else {
            ui.emo(pt(theme).pre, mx + mv * 0.35, my, mv * 0.62);
            ui.badge(&t.avatar, if on { c } else { GRAY }, mx + mv * 0.7 + mv / 2.0, my, mv);
            lane_pos.insert(*id, vec2(mx + mv, ly));
        }
    }
}


#[allow(clippy::too_many_arguments)]
fn draw_question(ui: &Ui, g: &Game, now: u64, x: f32, y: f32, w: f32, h: f32, since: f32) {
    let q = g.question();
    let reveal = g.reveal.as_ref();
    let n_teams = g.teams.len();
    // top line: question number · who answered (or each team's result) · count
    let cy = y + 16.0;
    let lw = ui.pill(x, cy, pt(g.part).emoji, &format!("Question {} / {}", g.qi + 1, g.q_count()), 16.0, 0.0);
    let right = match reveal {
        None => format!("{} / {} answered", g.teams.keys().filter(|&&id| g.answered(id)).count(), n_teams),
        Some(r) => format!("{} / {} correct", r.results.values().filter(|x| x.correct).count(), n_teams),
    };
    let rw = ui.pill(x + w, cy, if reveal.is_some() { "✅" } else { "✋" }, &right, 16.0, 1.0);
    let ids: Vec<u32> = g.quiz.teams.iter().copied().filter(|id| g.teams.contains_key(id)).collect();
    let labels: Vec<String> = ids
        .iter()
        .map(|id| match reveal.and_then(|r| r.results.get(id)) {
            Some(x) if x.correct => format!("+{}", x.points),
            Some(x) if x.choice.is_some() => "×".into(),
            Some(_) => "—".into(),
            None => String::new(),
        })
        .collect();
    let item_w: Vec<f32> = labels.iter().map(|l| if reveal.is_some() { 38.0 + ui.tw(l, 14.0, true) } else { 32.0 }).collect();
    let total: f32 = item_w.iter().sum::<f32>() + 6.0 * ids.len().saturating_sub(1) as f32;
    let mid_l = x + lw + 12.0;
    let mid_r = x + w - rw - 12.0;
    let mut ax = ((mid_l + mid_r) / 2.0 - total / 2.0).max(mid_l);
    for (i, &id) in ids.iter().enumerate() {
        let t = &g.teams[&id];
        let c = team_color(g, id);
        if reveal.is_none() {
            let yes = g.answered(id);
            ui.badge(&t.avatar, if yes { c } else { mix(c, ui.th.bg, 0.65) }, ax + 15.0, cy, if yes { 32.0 } else { 24.0 });
        } else {
            ui.rrect(ax, cy - 15.0, item_w[i], 30.0, 15.0, c);
            ui.rrect(ax + 2.0, cy - 13.0, item_w[i] - 4.0, 26.0, 13.0, ui.th.cardb);
            ui.badge(&t.avatar, c, ax + 15.0, cy, 24.0);
            ui.text(&labels[i], ax + 31.0, cy, 14.0, WHITE, true, 0.0);
        }
        ax += item_w[i] + 6.0;
    }
    // time bar
    let by = y + 40.0;
    ui.rrect(x, by, w, 10.0, 5.0, ui.th.cardb);
    let frac = if g.phase == Phase::Question { g.q_end.saturating_sub(now) as f32 / (g.quiz.question_seconds * 1000) as f32 } else { 0.0 };
    ui.rrect(x, by, w * frac, 10.0, 5.0, ui.th.accent);
    // question + options shrink until they fit above the track
    let n = q.options.len();
    let rows = n.div_ceil(2);
    let ow = (w - 12.0) / 2.0;
    let mut fs = 30.0;
    let mut ofs = 21.0;
    let (lines, wrapped, qh, row_h) = loop {
        let lines = ui.wrap(&q.q, w - 60.0, fs, true);
        let wrapped: Vec<Vec<String>> = q.options.iter().map(|o| ui.wrap(o, ow - 130.0, ofs, true)).collect();
        let qh = lines.len() as f32 * fs * 1.25 + 22.0;
        let row_h: Vec<f32> = (0..rows)
            .map(|r| {
                let ln = (0..2).filter_map(|c| wrapped.get(r * 2 + c)).map(|l| l.len()).max().unwrap_or(1) as f32;
                (ln * ofs * 1.22 + 18.0).max(50.0)
            })
            .collect();
        let need = 70.0 + qh + 12.0 + row_h.iter().sum::<f32>() + 10.0 * (rows as f32 - 1.0);
        if need <= h || fs <= 18.0 {
            break (lines, wrapped, qh, row_h);
        }
        fs -= 2.0;
        ofs = (ofs - 1.0).max(15.0);
    };
    let qy = by + 20.0;
    ui.card(x, qy, w, qh);
    for (i, l) in lines.iter().enumerate() {
        ui.text(l, x + w / 2.0, qy + 11.0 + fs * 0.62 + i as f32 * fs * 1.25, fs, WHITE, true, 0.5);
    }
    let mut ry = qy + qh + 12.0;
    for (r, &rh) in row_h.iter().enumerate() {
        for c in 0..2 {
            let i = r * 2 + c;
            if i >= n {
                continue;
            }
            let appear = ease((since - i as f32 * 0.06) / 0.3);
            let ox = x + c as f32 * (ow + 12.0);
            let yy = ry + (1.0 - appear) * 16.0;
            let right_ans = reveal.map(|r| r.correct == i);
            let col = hex(OPT_COLORS[i]);
            let dim = right_ans == Some(false);
            let col = if dim { mix(col, ui.th.bg, 0.7) } else { col };
            if right_ans == Some(true) {
                let glow = 4.0 + 2.0 * (ui.t * 5.0).sin();
                ui.rrect(ox - glow, yy - glow, ow + 2.0 * glow, rh + 2.0 * glow, 18.0 + glow, WHITE);
            }
            ui.rrect(ox, yy, ow, rh, 16.0, col);
            let lc = mix(col, BLACK, 0.25);
            ui.rrect(ox + 14.0, yy + rh / 2.0 - 19.0, 38.0, 38.0, 12.0, lc);
            if right_ans == Some(true) {
                ui.check(ox + 33.0, yy + rh / 2.0, 22.0);
            } else {
                ui.text(LETTERS[i], ox + 33.0, yy + rh / 2.0, 20.0, WHITE, true, 0.5);
            }
            let tl = &wrapped[i];
            let fg = if dim { Color::new(1.0, 1.0, 1.0, 0.55) } else { WHITE };
            for (k, l) in tl.iter().enumerate() {
                let ly = yy + rh / 2.0 + (k as f32 - (tl.len() as f32 - 1.0) / 2.0) * ofs * 1.22;
                ui.text(l, ox + 66.0, ly, ofs, fg, true, 0.0);
            }
            if let Some(r) = reveal {
                ui.rrect(ox + ow - 58.0, yy + rh / 2.0 - 17.0, 46.0, 34.0, 10.0, lc);
                ui.text(&r.counts[i].to_string(), ox + ow - 35.0, yy + rh / 2.0, 20.0, WHITE, true, 0.5);
            }
        }
        ry += rh + 10.0;
    }
}

fn draw_podium(ui: &Ui, g: &Game, rows: &[Ranked], cx: f32, base: f32, since: f32) {
    let slots = [(1usize, -190.0f32, 110.0f32), (0, 0.0, 150.0), (2, 190.0, 80.0)];
    for (ri, dx, bh) in slots {
        let Some(r) = rows.get(ri) else { continue };
        let grow = ease((since - 0.25 * (2 - ri.min(2)) as f32) / 0.8);
        let h = bh * grow;
        let x = cx + dx - 80.0;
        ui.rrect(x, base - h, 160.0, h + 14.0, 14.0, ui.th.accent);
        if grow > 0.5 {
            ui.emo(["🥇", "🥈", "🥉"][ri], cx + dx, base - h + 34.0, 46.0);
        }
        let ty = base - h - 26.0;
        ui.text(&fmt_n(r.pts), cx + dx, ty, 22.0, WHITE, true, 0.5);
        ui.text(&format!("Group {}", r.id), cx + dx, ty - 28.0, 20.0, WHITE, true, 0.5);
        if let Some(t) = g.teams.get(&r.id) {
            let jump = if ri == 0 { (ui.t * 4.0).sin().abs() * -8.0 } else { 0.0 };
            ui.badge(&t.avatar, team_color(g, r.id), cx + dx, ty - 78.0 + jump, 70.0);
        }
    }
}

/// Debug soak run (QUIZ_RACE_SHOT=<dir>): plays a whole game with nine fake teams, one step
/// every 0.9 s, saving a screenshot before each step. Returns a window size to switch to.
#[cfg(debug_assertions)]
fn soak_step(g: &mut Game, now: u64, step: usize) -> Option<(f32, f32)> {
    if step == 1 {
        for (i, id) in g.quiz.teams.clone().into_iter().enumerate() {
            let tok = format!("demoToken{id}");
            g.join(&tok, id, emoji_list::AVATARS[i * 3 % emoji_list::AVATARS.len()]).unwrap();
            g.connect(&tok);
        }
        return None;
    }
    match g.phase {
        Phase::Waiting => {
            g.start(now);
            return match g.part {
                1 => Some((1500.0, 860.0)),
                2 => Some((1050.0, 620.0)),
                _ => None,
            };
        }
        Phase::Question => {
            let n = g.question().options.len();
            for (i, id) in g.quiz.teams.clone().into_iter().enumerate() {
                g.answer(&format!("demoToken{id}"), (i + g.qi) % n, now);
            }
            g.skip(now);
        }
        Phase::Reveal | Phase::Results => g.next(now),
        Phase::Countdown | Phase::Final => {}
    }
    None
}

// ---------- main ----------

/// Virtual vw×vh canvas, y down, content shifted down by `oy`.
fn cam(vw: f32, vh: f32, oy: f32) -> Camera2D {
    Camera2D::from_display_rect(Rect::new(0.0, vh - oy, vw, -vh))
}

fn window_conf() -> Conf {
    fn rgba<const N: usize>(png: &[u8]) -> [u8; N] {
        Image::from_file_with_format(png, Some(ImageFormat::Png)).unwrap().bytes.try_into().unwrap()
    }
    Conf {
        window_title: "Quiz Race".into(),
        window_width: 1280,
        window_height: 720,
        high_dpi: true,
        sample_count: 4,
        window_resizable: true,
        icon: Some(miniquad::conf::Icon {
            small: rgba(include_bytes!("../assets/icon-16.png")),
            medium: rgba(include_bytes!("../assets/icon-32.png")),
            big: rgba(include_bytes!("../assets/icon-64.png")),
        }),
        ..Default::default()
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    let (quiz, quiz_src, quiz_err) = load_quiz();
    let sh = Shared::new(Game::new(quiz));
    let (port, lan_url, server_err) = match server::start(sh.clone()) {
        Ok(port) => (port, server::lan_ip().map(|ip| format!("http://{ip}:{port}/")), None),
        Err(e) => (3000, None, Some(e)),
    };
    prevent_quit(); // so closing the window also stops the online link

    // render music and effects off the main thread so the window shows at once
    let (tx, rx) = mpsc::channel();
    let sfx_tx = tx.clone();
    std::thread::spawn(move || {
        let v: Vec<(u8, Vec<u8>)> = audio::ALL_SFX.iter().map(|&s| (sfx_id(s), audio::wav(&audio::sfx(s)))).collect();
        let _ = sfx_tx.send(Load::Sfx(v));
    });
    load_music(tx.clone());

    let emoji: HashMap<&'static str, Texture2D> =
        emoji_list::EMOJI.iter().map(|(e, png)| (*e, Texture2D::from_file_with_format(png, Some(ImageFormat::Png)))).collect();

    #[cfg(debug_assertions)]
    let shot_dir = std::env::var("QUIZ_RACE_SHOT").ok();
    #[cfg(debug_assertions)]
    let (mut soak_step_n, mut soak_t) = (0usize, 1.5f32);
    let start_phase = sh.game.lock().unwrap().phase;
    let mut app = App {
        sh,
        lan_url,
        port,
        tunnel: None,
        reload_fonts: false,
        server_err,
        qr: None,
        quiz_src,
        quiz_err,
        sfx: None,
        music: None,
        own_music: own_music().is_some(),
        loads: (tx, rx),
        muted: false,
        fullscreen: false,
        lanes: HashMap::new(),
        lane_pos: HashMap::new(),
        lane_part: 0,
        screen: (start_phase, 0, 0),
        screen_t: 0.0,
        prev: (start_phase, 0, 0, 0, 0, 0),
        confetti: vec![],
        floaters: vec![],
        reset_confirm: 0.0,
        kick_confirm: None,
        toast: None,
        th: theme(None),
    };
    let chars = quiz_chars(&app.sh.game.lock().unwrap().quiz);
    let mut px_pending: Option<(f32, f32)> = None;
    let mut ui = Ui { s: 1.0, px: 0.0, vw: BASE_W, vh: BASE_H, fonts: Default::default(), chars, emoji, alpha: 1.0, mouse: Vec2::ZERO, click: false, th: app.th, t: 0.0, oy: 0.0 };

    loop {
        // sounds ready?
        while let Ok(msg) = app.loads.1.try_recv() {
            match msg {
                Load::Sfx(v) => {
                    let mut sfx = HashMap::new();
                    for (id, bytes) in v {
                        if let Ok(s) = load_sound_from_bytes(&bytes).await {
                            sfx.insert(id, s);
                        }
                    }
                    app.sfx = Some(sfx);
                }
                Load::Music { lobby, quiz, err } => {
                    if let Some(old) = app.music.take() {
                        stop_sound(&old.lobby);
                        if let Some(q) = &old.quiz {
                            stop_sound(q);
                        }
                    }
                    let Ok(lobby) = load_sound_from_bytes(&lobby).await else { continue };
                    let quiz = match quiz {
                        Some(b) => load_sound_from_bytes(&b).await.ok(),
                        None => None,
                    };
                    play_sound(&lobby, PlaySoundParams { looped: true, volume: 0.0 });
                    if let Some(q) = &quiz {
                        play_sound(q, PlaySoundParams { looped: true, volume: 0.0 });
                    }
                    app.own_music = quiz.is_none();
                    app.music = Some(Music { lobby, quiz, vol: (0.0, 0.0) });
                    if let Some(e) = err {
                        app.toast(&e);
                    } else if app.own_music {
                        app.toast("Playing your music");
                    }
                }
            }
        }

        let now = app.now();
        let t = get_time() as f32;
        let s = app.scale();
        ui.s = s;
        let px = ((s * screen_dpi_scale()) * 8.0).round() / 8.0;
        // new pixel scale: rebuild fonts once the window has stopped changing size (meanwhile
        // the old ones are scaled); new questions: rebuild now, they may bring new characters
        if px != ui.px {
            match px_pending {
                Some((p, since)) if p == px && (t - since > 0.3 || ui.px == 0.0) => {
                    ui.px = px;
                    ui.fonts.borrow_mut().clear();
                    px_pending = None;
                }
                Some((p, _)) if p == px => {}
                _ => px_pending = Some((px, if ui.px == 0.0 { -1.0 } else { t })),
            }
            if ui.px == 0.0 {
                ui.px = px;
            }
        }
        if app.reload_fonts {
            app.reload_fonts = false;
            ui.chars = quiz_chars(&app.sh.game.lock().unwrap().quiz);
            ui.fonts.borrow_mut().clear();
        }
        ui.vw = screen_width() / s;
        ui.vh = screen_height() / s;
        ui.t = t;
        ui.click = is_mouse_button_pressed(MouseButton::Left);
        ui.mouse = vec2(mouse_position().0, mouse_position().1) / s;

        let sh = app.sh.clone();
        #[allow(clippy::await_holding_lock)] // released by drop(g) before next_frame()
        let g = sh.game.lock().unwrap();
        app.react(&g, now);
        // theme colours ease toward the current part's
        let lobby = g.phase == Phase::Final || (g.part == 0 && g.phase == Phase::Waiting);
        let target = theme(if lobby { None } else { Some(g.part) });
        let k = (get_frame_time() * 5.0).min(1.0);
        app.th = Theme {
            bg: mix(app.th.bg, target.bg, k),
            card: mix(app.th.card, target.card, k),
            cardb: mix(app.th.cardb, target.cardb, k),
            accent: mix(app.th.accent, target.accent, k),
        };
        ui.th = app.th;
        if app.lane_part != g.part {
            app.lane_part = g.part;
            app.lanes.clear();
        }
        let key = (if g.phase == Phase::Reveal { Phase::Question } else { g.phase }, g.part, g.qi);
        if key != app.screen {
            app.screen = key;
            app.screen_t = t;
        }
        let since = t - app.screen_t;

        clear_background(app.th.bg);
        set_camera(&cam(ui.vw, ui.vh, 0.0));
        let mut acts: Vec<Act> = vec![];
        let mut kick_req = None;
        let mut ui_cmd: Option<&str> = None;

        draw_header(&ui, &g, now);

        // main content slides/fades in on every screen change
        let appear = ease(since / 0.4);
        ui.alpha = appear;
        ui.oy = (1.0 - appear) * 24.0;
        set_camera(&cam(ui.vw, ui.vh, ui.oy));
        let top = 84.0;
        let bottom = ui.vh - 76.0;
        let w = ui.vw - 2.0 * PAD;
        let show_track = matches!(g.phase, Phase::Countdown | Phase::Question | Phase::Reveal | Phase::Results);
        let th_h = if show_track { track_height(&ui, g.teams.len()) } else { 0.0 };
        let main_bottom = if show_track { bottom - th_h - 12.0 } else { bottom };
        let mh = main_bottom - top;

        match g.phase {
            Phase::Waiting if g.part == 0 => {
                let jw = 340.0;
                ui.card(PAD, top, jw, mh);
                let cx = PAD + jw / 2.0;
                ui.text("Scan or open", cx, top + 26.0, 22.0, WHITE, true, 0.5);
                let url = app.join_url();
                if app.qr.as_ref().is_none_or(|(u, _)| *u != url) {
                    app.qr = QrCode::encode_text(&url, QrCodeEcc::Medium).ok().map(|q| (url.clone(), q));
                }
                let qs = (mh - 190.0).clamp(120.0, 270.0);
                if let Some((_, qr)) = &app.qr {
                    draw_qr(&ui, qr, cx, top + 48.0, qs);
                }
                let shown = url.trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/');
                let mut uy = top + 48.0 + qs + 26.0;
                let us = if ui.tw(shown, 22.0, true) > jw - 40.0 { 15.0 } else { 22.0 };
                let shown = ui.fit(shown, jw - 40.0, us, true);
                ui.rrect(PAD + 16.0, uy - 18.0, jw - 32.0, 36.0, 12.0, ui.th.bg);
                ui.text(&shown, cx, uy, us, WHITE, true, 0.5);
                uy += 30.0;
                if let Some(e) = &app.server_err {
                    ui.text(&ui.fit(e, jw - 30.0, 14.0, true), cx, uy, 14.0, hex(0xffb4a8), true, 0.5);
                } else {
                    let tstate = app.tunnel.as_ref().map(|t| t.state());
                    let (msg, col) = match &tstate {
                        Some(tunnel::State::Ready(_)) => ("Online link — works on any network".to_string(), WHITE),
                        Some(tunnel::State::Failed(e)) => (e.clone(), hex(0xffb4a8)),
                        None if app.lan_url.is_none() => ("No network found — use the online link".into(), hex(0xffb4a8)),
                        _ => ("Can't join? Phones on other Wi-Fi → Online link".into(), WHITE),
                    };
                    ui.text(&ui.fit(&msg, jw - 24.0, 14.0, true), cx, uy, 14.0, col, true, 0.5);
                    let by = uy + 18.0;
                    if by + 34.0 < main_bottom {
                        let (label, enabled, cmd) = match &tstate {
                            None | Some(tunnel::State::Failed(_)) => ("Online link (Internet)", true, "online"),
                            Some(tunnel::State::Downloading) => ("Downloading cloudflared…", false, ""),
                            Some(tunnel::State::Starting) => ("Starting online link…", false, ""),
                            Some(tunnel::State::Ready(_)) => ("Use Wi-Fi link instead", true, "lan"),
                        };
                        if ui.button(label, PAD + 30.0, by, jw - 60.0, 34.0, tstate.is_none(), enabled) {
                            ui_cmd = Some(cmd);
                        }
                    }
                }
                let by = uy + 18.0;
                let info = match &app.quiz_err {
                    Some(e) => e.clone(),
                    None => format!("{} questions · {}", g.quiz.total_questions(), app.quiz_src),
                };
                if by + 70.0 < main_bottom {
                    let col = if app.quiz_err.is_some() { hex(0xffb4a8) } else { Color::new(1.0, 1.0, 1.0, 0.7) };
                    ui.text(&ui.fit(&info, jw - 30.0, 13.0, false), cx, main_bottom - 18.0, 13.0, col, false, 0.5);
                }
                let sx = PAD + jw + PAD;
                lobby_slots(&ui, &g, sx, top, ui.vw - PAD - sx, mh, &app.kick_confirm, &mut acts, &mut kick_req);
            }
            Phase::Waiting => {
                let rw = 480.0f32.min(w * 0.45);
                let hx = PAD + (w - rw - PAD) / 2.0;
                let title = format!("Up next: {}", cap(g.part));
                let ptitle = ui.wrap(&g.quiz.parts[g.part].title, w - rw - 60.0, 24.0, true);
                let mut lines: Vec<&str> = ptitle.iter().map(|s| s.as_str()).collect();
                lines.push("Present now — press Start when the class is ready for the quiz.");
                let ts = if ui.tw(&title, 46.0, true) > w - rw - 40.0 { 34.0 } else { 46.0 };
                ui.emo(pt(g.part).emoji, hx, top + mh / 2.0 - 120.0, 120.0);
                ui.text(&title, hx, top + mh / 2.0, ts, WHITE, true, 0.5);
                for (i, l) in lines.iter().enumerate() {
                    let last = i + 1 == lines.len();
                    ui.text(l, hx, top + mh / 2.0 + 50.0 + i as f32 * 32.0, if last { 17.0 } else { 23.0 }, WHITE, !last, 0.5);
                }
                let rx = ui.vw - PAD - rw;
                ui.card(rx, top, rw, mh);
                ui.text("Overall standings", rx + 18.0, top + 24.0, 20.0, WHITE, true, 0.0);
                let rows = g.ranking(None);
                let rh = ((mh - 70.0) / rows.len().max(1) as f32 - 5.0).clamp(24.0, 42.0);
                table(&ui, &g, &rows, rx + 12.0, top + 40.0, rw - 24.0, rh, &[("Pts", 100.0, &|r: &Ranked| fmt_n(r.pts))]);
            }
            Phase::Countdown => {
                let left = g.cd_end.saturating_sub(now);
                let n = left.div_ceil(1000).clamp(1, 3);
                let frac = (left % 1000) as f32 / 1000.0;
                ui.emo(pt(g.part).emoji, ui.vw / 2.0 - 260.0, top + mh / 2.0, 120.0);
                ui.text(&cap(g.part), ui.vw / 2.0 + 40.0, top + mh * 0.28, 44.0, WHITE, true, 0.5);
                let zoom = (mh * 0.5).min(180.0) / COUNTDOWN_SIZE * (1.0 + 0.25 * frac);
                ui.text_zoom(&n.to_string(), ui.vw / 2.0 + 40.0, top + mh * 0.65, COUNTDOWN_SIZE, zoom, ui.th.accent, true, 0.5);
            }
            Phase::Question | Phase::Reveal => draw_question(&ui, &g, now, PAD, top, w, mh, since),
            Phase::Results => {
                let rows = g.ranking(Some(g.part));
                ui.card(PAD, top, w, mh);
                ui.emo(pt(g.part).emoji, PAD + 34.0, top + 24.0, 30.0);
                ui.text(&format!("{} — results", pt(g.part).name), PAD + 56.0, top + 24.0, 22.0, WHITE, true, 0.0);
                let rh = ((mh - 64.0) / rows.len().max(1) as f32 - 5.0).clamp(22.0, 42.0);
                let qc = g.q_count();
                let total = |r: &Ranked| fmt_n(g.teams.get(&r.id).map_or(0, |t| t.total()));
                table(&ui, &g, &rows, PAD + 14.0, top + 38.0, w - 28.0, rh, &[
                    ("Correct", 140.0, &|r: &Ranked| format!("{}/{}", r.cor, qc)),
                    ("Time", 140.0, &|r: &Ranked| format!("{:.1}s", r.ms as f32 / 1000.0)),
                    ("Part pts", 160.0, &|r: &Ranked| fmt_n(r.pts)),
                    ("Total", 160.0, &total),
                ]);
            }
            Phase::Final => {
                let rows = g.ranking(None);
                let pod_h = (mh * 0.48).min(330.0);
                draw_podium(&ui, &g, &rows, ui.vw / 2.0, top + pod_h, since);
                let ty = top + pod_h + 22.0;
                ui.card(PAD, ty, w, main_bottom - ty);
                let rh = ((main_bottom - ty - 40.0) / rows.len().max(1) as f32 - 5.0).clamp(20.0, 40.0);
                let parts = g.quiz.parts.len();
                let tq = g.quiz.total_questions();
                let labels: Vec<String> = (0..parts).map(|p| format!("Part {}", p + 1)).collect();
                let part_fns: Vec<Box<dyn Fn(&Ranked) -> String>> = (0..parts)
                    .map(|p| {
                        let gref = &g;
                        Box::new(move |r: &Ranked| fmt_n(gref.teams.get(&r.id).map_or(0, |t| t.scores[p]))) as Box<dyn Fn(&Ranked) -> String>
                    })
                    .collect();
                let cw = ((w - 380.0) / (parts as f32 + 2.0)).min(150.0);
                let mut cols: Vec<(&str, f32, &dyn Fn(&Ranked) -> String)> = vec![];
                for p in 0..parts {
                    cols.push((labels[p].as_str(), cw, part_fns[p].as_ref()));
                }
                let cor = |r: &Ranked| format!("{}/{}", r.cor, tq);
                let tot = |r: &Ranked| fmt_n(r.pts);
                cols.push(("Correct", cw, &cor));
                cols.push(("Total", cw, &tot));
                table(&ui, &g, &rows, PAD + 14.0, ty + 8.0, w - 28.0, rh, &cols);
            }
        }

        if show_track {
            let ty = bottom - th_h;
            draw_track(&ui, &g, &mut app.lanes, &mut app.lane_pos, PAD, ty, w, th_h);
        }

        // score pop-ups above the lanes
        ui.alpha = 1.0;
        set_camera(&cam(ui.vw, ui.vh, 0.0));
        app.floaters.retain(|f| t - f.born < 1.6);
        if show_track {
            for f in &app.floaters {
                if let Some(p) = app.lane_pos.get(&f.team) {
                    let k = (t - f.born) / 1.6;
                    ui.alpha = 1.0 - k * k;
                    let y = p.y + 6.0 - 26.0 * ease(k);
                    ui.rrect(p.x - 34.0, y - 13.0, 68.0, 26.0, 13.0, ui.th.accent);
                    ui.text(&f.text, p.x, y, 16.0, DARK, true, 0.5);
                }
            }
            ui.alpha = 1.0;
        }

        // footer
        let fy = ui.vh - 60.0;
        let reset_armed = t < app.reset_confirm;
        if ui.button(if reset_armed { "Click again to reset" } else { "Reset game" }, PAD, fy, if reset_armed { 200.0 } else { 130.0 }, 44.0, false, true) {
            if reset_armed {
                acts.push(Act::Reset);
                app.reset_confirm = 0.0;
            } else {
                app.reset_confirm = t + 3.0;
            }
        }
        let mut fx = PAD + if reset_armed { 212.0 } else { 142.0 };
        let auto_lbl = if g.auto { "Auto-advance: on" } else { "Auto-advance: off" };
        if ui.button(auto_lbl, fx, fy, 180.0, 44.0, false, true) {
            acts.push(Act::Auto(!g.auto));
        }
        fx += 192.0;
        if ui.button(if app.muted { "Sound: off (M)" } else { "Sound: on (M)" }, fx, fy, 150.0, 44.0, false, true) {
            ui_cmd = Some("mute");
        }
        fx += 162.0;
        if g.phase == Phase::Waiting && g.part == 0 {
            if ui.button("Questions file…", fx, fy, 160.0, 44.0, false, true) {
                ui_cmd = Some("questions");
            }
            fx += 172.0;
            let (label, cmd) = if app.own_music { ("Default music", "music-off") } else { ("Music…", "music") };
            if ui.button(label, fx, fy, 130.0, 44.0, false, true) {
                ui_cmd = Some(cmd);
            }
            fx += 142.0;
        }
        let last_q = g.qi + 1 >= g.q_count();
        let more_parts = g.part + 1 < g.quiz.parts.len();
        let (label, act, hint): (String, Option<Act>, String) = match g.phase {
            Phase::Waiting => (
                format!("Start Part {} quiz", g.part + 1),
                Some(Act::Start),
                if g.part == 0 { format!("{}/{} teams joined", g.teams.len(), g.quiz.teams.len()) } else { "Presentation first, then press start".into() },
            ),
            Phase::Countdown => ("Starting…".into(), None, String::new()),
            Phase::Question => ("End question now".into(), Some(Act::Skip), String::new()),
            Phase::Reveal => (
                if last_q { "Show part results".into() } else { "Next question".into() },
                Some(Act::Next),
                if g.auto { "auto-advancing…".into() } else { String::new() },
            ),
            Phase::Results => (
                if more_parts { format!("Continue: present Part {}", g.part + 2) } else { "Show final results".into() },
                Some(Act::Next),
                String::new(),
            ),
            Phase::Final => ("Game over".into(), None, String::new()),
        };
        let pw = 300.0;
        let px = ui.vw - PAD - pw;
        let enabled = act.is_some();
        let pressed = ui.button(&label, px, fy - 2.0, pw, 48.0, true, enabled)
            || (enabled && (is_key_pressed(KeyCode::Space) || is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::KpEnter)));
        if !hint.is_empty() && px - fx > 200.0 {
            ui.pill(px - 14.0, fy + 22.0, "", &hint, 15.0, 1.0);
        }
        if pressed {
            if let Some(a) = act {
                acts.push(a);
            }
        }

        // confetti
        let dt = get_frame_time();
        for c in &mut app.confetti {
            c.v.y += 260.0 * dt;
            c.v.x += (t * 3.0 + c.rot).sin() * 30.0 * dt;
            c.p += c.v * dt;
            c.rot += c.spin * dt;
            draw_rectangle_ex(c.p.x, c.p.y, 10.0, 6.0, DrawRectangleParams { offset: vec2(0.5, 0.5), rotation: c.rot, color: c.color });
        }
        let vh = ui.vh;
        app.confetti.retain(|c| c.p.y < vh + 20.0);

        // toast
        if let Some((msg, at)) = &app.toast {
            let k = t - at;
            if k < 3.5 {
                ui.alpha = (3.5 - k).min(1.0);
                let tw = ui.tw(msg, 17.0, true) + 40.0;
                ui.rrect(ui.vw / 2.0 - tw / 2.0, 84.0, tw, 40.0, 20.0, DARK);
                ui.text(msg, ui.vw / 2.0, 104.0, 17.0, WHITE, true, 0.5);
                ui.alpha = 1.0;
            }
        }
        drop(g);

        if ui.click && !acts.is_empty() || ui_cmd.is_some() || kick_req.is_some() {
            app.play(Sfx::Click);
        }
        if let Some(id) = kick_req {
            app.kick_confirm = Some((id, t + 3.0));
        }
        if is_key_pressed(KeyCode::M) {
            ui_cmd = Some("mute");
        }
        if is_key_pressed(KeyCode::F11) {
            app.fullscreen = !app.fullscreen;
            set_fullscreen(app.fullscreen);
        }
        match ui_cmd {
            Some("mute") => app.muted = !app.muted,
            Some("questions") => app.open_questions_file(),
            Some("music") => app.pick_music(),
            Some("music-off") => app.remove_music(),
            Some("online") => app.tunnel = Some(tunnel::Tunnel::start(app.port)),
            Some("lan") => app.tunnel = None,
            _ => {}
        }
        app.apply(acts);
        // debug builds only: QUIZ_RACE_SHOT=<dir> saves the results and final screens, then quits
        #[cfg(debug_assertions)]
        if let Some(dir) = &shot_dir {
            if t >= soak_t {
                soak_t = t + 0.9;
                get_screen_data().export_png(&format!("{dir}/soak{soak_step_n:03}.png"));
                if app.screen.0 == Phase::Final && soak_step_n > 5 {
                    std::process::exit(0);
                }
                soak_step_n += 1;
                if let Some((w, h)) = app.sh.with(|g, now| soak_step(g, now, soak_step_n)) {
                    request_new_screen_size(w, h);
                }
            }
        }
        if is_quit_requested() {
            app.tunnel = None; // stops cloudflared
            break;
        }
        next_frame().await;
    }
}

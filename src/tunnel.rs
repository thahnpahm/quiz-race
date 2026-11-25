//! "Online link": a Cloudflare quick tunnel, so phones that can't reach this computer directly
//! (another network, or school Wi-Fi that keeps devices apart) can still join. Runs cloudflared
//! hidden; downloads it once (official GitHub release) if it isn't installed.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

const DOWNLOAD: &str = "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe";

#[derive(Clone, PartialEq, Debug)]
pub enum State {
    Downloading,
    Starting,
    Ready(String),
    Failed(String),
}

struct Proc {
    stopped: bool,
    child: Option<Child>,
}

pub struct Tunnel {
    state: Arc<Mutex<State>>,
    proc: Arc<Mutex<Proc>>,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Quiz Race"))
}

fn find_cloudflared() -> Option<PathBuf> {
    let beside_exe = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("cloudflared.exe")));
    let on_path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).map(|d| d.join("cloudflared.exe")).collect::<Vec<_>>()).unwrap_or_default();
    [beside_exe, home().map(|h| h.join("cloudflared.exe"))].into_iter().flatten().chain(on_path).find(|p| p.is_file())
}

fn hidden(c: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

fn download() -> Result<PathBuf, String> {
    let dir = home().ok_or("LOCALAPPDATA is not set")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (tmp, dst) = (dir.join("cloudflared.download"), dir.join("cloudflared.exe"));
    let curl = std::env::var_os("SystemRoot").map(|r| PathBuf::from(r).join("System32").join("curl.exe")).unwrap_or_else(|| "curl.exe".into());
    let ok = hidden(Command::new(curl).args(["-L", "--fail", "--silent", "-o"]).arg(&tmp).arg(DOWNLOAD))
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        let _ = std::fs::remove_file(&tmp);
        return Err("Couldn't download cloudflared — is the Internet on?".into());
    }
    std::fs::rename(&tmp, &dst).map_err(|e| e.to_string())?;
    Ok(dst)
}

/// The public address in a cloudflared log line.
pub fn find_url(line: &str) -> Option<String> {
    let i = line.find("https://")?;
    let url: String = line[i..].chars().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '/' | '.' | '-')).collect();
    (url.ends_with(".trycloudflare.com") && url != "https://api.trycloudflare.com").then_some(url)
}

impl Tunnel {
    pub fn start(port: u16) -> Tunnel {
        let state = Arc::new(Mutex::new(State::Starting));
        let proc = Arc::new(Mutex::new(Proc { stopped: false, child: None }));
        let (st, pr) = (state.clone(), proc.clone());
        thread::spawn(move || {
            let set = |v: State| *st.lock().unwrap() = v;
            let exe = match find_cloudflared() {
                Some(p) => p,
                None => {
                    set(State::Downloading);
                    match download() {
                        Ok(p) => p,
                        Err(e) => return set(State::Failed(e)),
                    }
                }
            };
            set(State::Starting);
            let mut cmd = Command::new(exe);
            cmd.args(["tunnel", "--no-autoupdate", "--url", &format!("http://127.0.0.1:{port}")])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped());
            let mut child = match hidden(&mut cmd).spawn() {
                Ok(c) => c,
                Err(e) => return set(State::Failed(format!("Couldn't start cloudflared: {e}"))),
            };
            let log = child.stderr.take();
            {
                let mut p = pr.lock().unwrap();
                if p.stopped {
                    let _ = child.kill();
                    return;
                }
                p.child = Some(child);
            }
            // keep reading so cloudflared never blocks on a full pipe
            for line in BufReader::new(log.unwrap()).lines().map_while(Result::ok) {
                if let Some(u) = find_url(&line) {
                    set(State::Ready(u));
                }
            }
            if !pr.lock().unwrap().stopped {
                set(State::Failed("The online link stopped — try again.".into()));
            }
        });
        Tunnel { state, proc }
    }

    pub fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }
}

impl Drop for Tunnel {
    // ponytail: only a normal quit stops cloudflared; after a crash it lingers until reboot (harmless, points at a dead port)
    fn drop(&mut self) {
        let mut p = self.proc.lock().unwrap();
        p.stopped = true;
        if let Some(mut c) = p.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::find_url;

    /// Real run (needs Internet; downloads cloudflared once): `cargo test -- --ignored online`
    #[test]
    #[ignore]
    fn online_link_starts_and_stops() {
        use super::{State, Tunnel};
        let t = Tunnel::start(3000);
        let t0 = std::time::Instant::now();
        let url = loop {
            match t.state() {
                State::Ready(u) => break u,
                State::Failed(e) => panic!("{e}"),
                _ if t0.elapsed().as_secs() > 120 => panic!("no link after 120 s"),
                _ => std::thread::sleep(std::time::Duration::from_millis(200)),
            }
        };
        println!("online link: {url} after {:?}", t0.elapsed());
        drop(t); // must stop cloudflared
    }

    #[test]
    fn reads_the_link_from_cloudflared_log() {
        let box_line = "2026-11-25T06:14:47Z INF |  https://greene-pins-ranch-scanning.trycloudflare.com                                      |";
        assert_eq!(find_url(box_line).as_deref(), Some("https://greene-pins-ranch-scanning.trycloudflare.com"));
        assert_eq!(find_url("INF Requesting new quick Tunnel on trycloudflare.com..."), None);
        assert_eq!(find_url("ERR failed to request quick Tunnel: Post \"https://api.trycloudflare.com/tunnel\""), None);
        assert_eq!(find_url("see https://developers.cloudflare.com/cloudflare-one"), None);
    }
}

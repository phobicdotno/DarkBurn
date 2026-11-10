//! The update check (SHELL-13). Off unless the user turns on `Settings > Check for updates`;
//! the command `help.check_updates` always runs one check when asked.
//!
//! One HTTPS GET of a small JSON document (by default the GitHub releases API of the DarkBurn
//! repository), the version in it compared with this one, and the answer shown in a dialog with
//! a link. Nothing is downloaded or installed, and nothing identifying is sent: the only header
//! of its own is the generic user agent `DarkBurn`. The repository may be private, in which case
//! GitHub answers 404 or 401 and the dialog says that the check could not see any release.
//!
//! Layout of the JSON it understands: GitHub's `tag_name` / `html_url` / `body`, or plain
//! `version` / `url` / `notes`.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use darkburn_ui::ui_kit;
use egui::{Context, RichText};
use solidcraft_theme::Tokens;

use crate::i18n::tr;
use crate::shell::DarkBurn;

/// The releases API of the DarkBurn repository (latest published, non-draft release).
pub const DEFAULT_URL: &str = "https://api.github.com/repos/phobicdotno/DarkBurn/releases/latest";
/// Where "Open" goes when the answer has no usable link.
pub const RELEASES_PAGE: &str = "https://github.com/phobicdotno/DarkBurn/releases";
/// The only identification sent.
pub const USER_AGENT: &str = "DarkBurn";
/// Environment variable that replaces the address (for testing and workshops).
pub const URL_ENV: &str = "DARKBURN_UPDATE_URL";

/// What a check found.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    UpToDate {
        current: String,
    },
    Available {
        current: String,
        latest: String,
        url: String,
        notes: String,
    },
    /// The server answered but has nothing for us: private repository (404, 401, 403), no releases.
    NotVisible {
        status: u16,
    },
    /// No answer: no network, bad address, timeout, TLS trouble.
    Failed(String),
    /// An answer that is not the expected JSON.
    Unreadable(String),
}

impl Outcome {
    /// One plain sentence for the status line and the dialog.
    pub fn message(&self) -> String {
        match self {
            Outcome::UpToDate { current } => format!("{} {current}.", tr("updates.up_to_date", "DarkBurn is up to date, version")),
            Outcome::Available { latest, current, .. } => {
                format!("{} {latest} ({} {current}).", tr("updates.available", "A newer version is available:"), tr("updates.you_have", "you have"))
            }
            Outcome::NotVisible { status } => format!(
                "{} (HTTP {status}).",
                tr("updates.not_visible", "The release page is private or has no releases yet, so there is nothing to compare with")
            ),
            Outcome::Failed(e) => format!("{} {e}", tr("updates.failed", "Could not reach the update server:")),
            Outcome::Unreadable(e) => format!("{} {e}", tr("updates.unreadable", "The update server's answer could not be read:")),
        }
    }
}

/// A version as numbers: `v1.2.3`, `1.2` and `1.2.3-beta` give (1,2,3), (1,2,0), (1,2,3).
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let t = text.trim().trim_start_matches(['v', 'V']);
    let core = t.split(['-', '+', ' ']).next()?;
    if core.is_empty() {
        return None;
    }
    let mut parts = core.split('.');
    let mut next = || -> Option<u64> { parts.next().map_or(Some(0), |p| p.parse().ok()) };
    Some((next()?, next()?, next()?))
}

/// Whether `latest` is a newer version than `current`; false when either does not parse.
pub fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse_version(latest), parse_version(current)), (Some(l), Some(c)) if l > c)
}

/// A link that is safe to hand to the system's opener: https, and only characters that cannot
/// be read as shell syntax.
pub fn safe_link(url: &str) -> Option<&str> {
    let ok = url.starts_with("https://") && url.len() < 300 && url.chars().all(|c| c.is_ascii_alphanumeric() || "-._~:/?=#@+".contains(c));
    ok.then_some(url)
}

fn allowed_address(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    // Plain http only to this machine (the tests' loopback server).
    ["http://127.0.0.1", "http://localhost", "http://[::1]"].iter().any(|p| url.strip_prefix(p).is_some_and(|rest| rest.starts_with([':', '/'])))
}

/// Where a redirect from `from` to `location` leads, when the check may go there: the address
/// rule holds for every hop, and a secure address never steps down to plain http (not to this
/// computer either: a server on the internet has no business pointing the check at it).
fn next_hop(from: &str, location: &str) -> Result<String, String> {
    let location = location.trim();
    let to = if location.starts_with('/') && !location.starts_with("//") {
        let after_scheme = from.find("://").map_or(0, |i| i + 3);
        let origin_end = from[after_scheme..].find('/').map_or(from.len(), |i| after_scheme + i);
        format!("{}{location}", &from[..origin_end])
    } else {
        location.to_owned()
    };
    if !allowed_address(&to) || (from.starts_with("https://") && !to.starts_with("https://")) {
        return Err("the update address sent the check on to an address that does not start with https://".into());
    }
    Ok(to)
}

/// Read a releases document.
pub fn parse_release(body: &str, current: &str) -> Outcome {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return Outcome::Unreadable("not JSON".into());
    };
    let text = |keys: &[&str]| keys.iter().find_map(|k| v.get(*k).and_then(|x| x.as_str())).map(str::trim).filter(|s| !s.is_empty());
    let Some(latest) = text(&["tag_name", "version"]) else {
        return Outcome::Unreadable("no version in it".into());
    };
    if parse_version(latest).is_none() {
        return Outcome::Unreadable(format!("version {latest:?} is not a version number"));
    }
    if v.get("draft").and_then(|d| d.as_bool()) == Some(true) || !is_newer(latest, current) {
        return Outcome::UpToDate { current: current.to_string() };
    }
    let url = text(&["html_url", "url"]).and_then(safe_link).unwrap_or(RELEASES_PAGE).to_string();
    let notes: String = text(&["body", "notes"]).unwrap_or("").chars().take(1200).collect();
    Outcome::Available { current: current.to_string(), latest: latest.trim_start_matches(['v', 'V']).to_string(), url, notes }
}

/// Do one check, blocking (call it from a thread). `url` is the document's address.
pub fn check(url: &str, current: &str) -> Outcome {
    if !allowed_address(url) {
        return Outcome::Failed("the update address must start with https://".into());
    }
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .unversioned_rustls_crypto_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(10)))
        .timeout_connect(Some(Duration::from_secs(5)))
        .user_agent(USER_AGENT)
        // Redirects are followed here, one at a time, so every hop is checked (`next_hop`).
        .max_redirects(0)
        .max_redirects_will_error(false)
        .build()
        .into();
    let mut url = url.to_owned();
    let mut hops = 0;
    let mut response = loop {
        let response = match agent.get(&url).header("Accept", "application/json").call() {
            Ok(r) => r,
            Err(e) => return Outcome::Failed(e.to_string()),
        };
        if !matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            break response;
        }
        let Some(location) = response.headers().get("location").and_then(|l| l.to_str().ok()) else {
            return Outcome::Failed(format!("HTTP {} without an address to go to", response.status().as_u16()));
        };
        hops += 1;
        if hops > 3 {
            return Outcome::Failed("the update address sent the check on too many times".into());
        }
        url = match next_hop(&url, location) {
            Ok(to) => to,
            Err(e) => return Outcome::Failed(e),
        };
    };
    let status = response.status().as_u16();
    match status {
        200 => {}
        401 | 403 | 404 => return Outcome::NotVisible { status },
        _ => return Outcome::Failed(format!("HTTP {status}")),
    }
    match response.body_mut().with_config().limit(1 << 20).read_to_string() {
        Ok(body) => parse_release(&body, current),
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

/// The address to ask: the environment, the setting, or the default.
pub fn address(setting: &str) -> String {
    let env = std::env::var(URL_ENV).ok();
    [env.as_deref().unwrap_or(""), setting].iter().map(|s| s.trim()).find(|s| !s.is_empty()).unwrap_or(DEFAULT_URL).to_string()
}

/// The check in flight and its answer.
#[derive(Default)]
pub struct State {
    rx: Option<Receiver<Outcome>>,
    /// The latest answer.
    pub result: Option<Outcome>,
    /// The dialog is showing.
    pub open: bool,
    /// The running check was started by the command (show any answer), not by the setting
    /// (show only a newer version).
    asked: bool,
    started_at_launch: bool,
}

impl State {
    pub fn running(&self) -> bool {
        self.rx.is_some()
    }

    /// Start a check on a thread. Does nothing when one is running.
    pub fn start(&mut self, url: String, asked: bool) {
        if self.rx.is_some() {
            return;
        }
        if cfg!(test) && !url.starts_with("http://127.0.0.1") {
            // Tests never touch the network (their own loopback server is fine).
            self.asked = asked;
            self.open = asked;
            self.result = Some(Outcome::Failed("no network in tests".into()));
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        self.asked = asked;
        self.open = asked;
        let spawned = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            let _ = tx.send(check(&url, env!("CARGO_PKG_VERSION")));
        });
        if spawned.is_err() {
            self.rx = None;
            self.result = Some(Outcome::Failed("could not start the check".into()));
        }
    }

    /// Collect the answer if it has arrived. True when one did.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        match rx.try_recv() {
            Ok(outcome) => {
                self.rx = None;
                self.open = self.asked || matches!(outcome, Outcome::Available { .. });
                self.result = Some(outcome);
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.rx = None;
                self.result = Some(Outcome::Failed("the check stopped".into()));
                self.open = self.asked;
                true
            }
        }
    }
}

/// The command `help.check_updates` (and its older name `help.updates`).
pub fn run_command(app: &mut DarkBurn) {
    let url = address(&app.prefs.opts.update_url);
    app.extras.updates.start(url, true);
    app.note = Some(tr("updates.checking", "Checking for updates..."));
}

/// Once a frame: start the launch check if the setting is on, collect answers.
pub fn frame(ctx: &Context, app: &mut DarkBurn) {
    if !app.extras.updates.started_at_launch {
        app.extras.updates.started_at_launch = true;
        if app.prefs.opts.check_updates && !app.safe_mode {
            let url = address(&app.prefs.opts.update_url);
            app.extras.updates.start(url, false);
        }
    }
    if app.extras.updates.running() {
        ctx.request_repaint_after(Duration::from_millis(200));
        if app.extras.updates.poll()
            && app.extras.updates.asked
            && let Some(r) = &app.extras.updates.result
        {
            app.note = Some(r.message());
        }
    }
}

/// The result window.
pub fn dialog(ctx: &Context, app: &mut DarkBurn) {
    if !app.extras.updates.open {
        return;
    }
    let running = app.extras.updates.running();
    let result = app.extras.updates.result.clone();
    let link = match &result {
        Some(Outcome::Available { url, .. }) => Some(url.clone()),
        _ => None,
    };
    let primary = if link.is_some() { tr("updates.open", "Open the download page") } else { String::new() };
    let (closed, action) = ui_kit::Window::new(egui::Id::unique("db_updates"), "help", &tr("updates.title", "Check for Updates"))
        .actions(&primary, &tr("ui.close", "Close"), None)
        .show(ctx, true, |ui| {
            let t = Tokens::get();
            ui.set_min_width(380.0);
            match &result {
                _ if running => {
                    ui.label(tr("updates.checking", "Checking for updates..."));
                }
                None => {
                    ui.label(tr("updates.none_yet", "No check has run yet."));
                }
                Some(r) => {
                    ui.add(egui::Label::new(RichText::new(r.message())).wrap());
                    if let Outcome::Available { notes, .. } = r
                        && !notes.is_empty()
                    {
                        ui.add_space(t.space_6);
                        ui.add(egui::Label::new(RichText::new(notes.as_str()).color(t.text_dim)).wrap());
                    }
                }
            }
            ui.add_space(t.space_6);
            ui.add(
                egui::Label::new(
                    RichText::new(tr("updates.privacy", "Nothing is downloaded or installed, and nothing about you is sent.")).color(t.text_dim),
                )
                .wrap(),
            );
        });
    if let (Some(url), ui_kit::Action::Ok) = (&link, action) {
        crate::help::open_url(safe_link(url).unwrap_or(RELEASES_PAGE));
    }
    if closed || action != ui_kit::Action::None {
        app.extras.updates.open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// A tiny HTTP server on an ephemeral loopback port that answers `requests` requests.
    /// Returns the address and the raw requests it saw.
    fn serve(status: &'static str, body: &'static str, requests: usize) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for _ in 0..requests {
                let Ok((mut s, _)) = listener.accept() else { return };
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                log.lock().unwrap().push(String::from_utf8_lossy(&buf[..n]).to_string());
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(reply.as_bytes());
            }
        });
        (format!("http://127.0.0.1:{}/releases/latest", addr.port()), seen)
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert_eq!(parse_version("v0.0.25"), Some((0, 0, 25)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("1.2.3-beta.1"), Some((1, 2, 3)));
        assert_eq!(parse_version("nightly"), None);
        assert!(is_newer("v0.0.25", "0.0.24"));
        assert!(is_newer("0.1.0", "0.0.99"));
        assert!(is_newer("0.0.100", "0.0.99"));
        assert!(!is_newer("0.0.24", "0.0.24"));
        assert!(!is_newer("0.0.23", "0.0.24"));
        assert!(!is_newer("garbage", "0.0.24"));
    }

    #[test]
    fn links_that_could_run_commands_are_refused() {
        assert!(safe_link("https://github.com/phobicdotno/DarkBurn/releases/tag/v0.0.25").is_some());
        assert!(safe_link("http://example.com").is_none());
        assert!(safe_link("https://example.com/a&calc.exe").is_none());
        assert!(safe_link("https://example.com/a b").is_none());
        assert!(safe_link("https://example.com/\"").is_none());
    }

    #[test]
    fn a_newer_release_is_reported_with_its_link() {
        let o = parse_release(r#"{"tag_name":"v9.0.0","html_url":"https://github.com/x/y/releases/tag/v9.0.0","body":"New: things"}"#, "0.0.24");
        assert_eq!(
            o,
            Outcome::Available {
                current: "0.0.24".into(),
                latest: "9.0.0".into(),
                url: "https://github.com/x/y/releases/tag/v9.0.0".into(),
                notes: "New: things".into()
            }
        );
        let o = parse_release(r#"{"version":"9.0.0","url":"javascript:alert(1)"}"#, "0.0.24");
        assert!(matches!(o, Outcome::Available { url, .. } if url == RELEASES_PAGE), "an unsafe link is replaced");
        assert!(matches!(parse_release(r#"{"tag_name":"v0.0.24"}"#, "0.0.24"), Outcome::UpToDate { .. }));
        assert!(matches!(parse_release("<html>", "0.0.24"), Outcome::Unreadable(_)));
        assert!(matches!(parse_release(r#"{"tag_name":"soon"}"#, "0.0.24"), Outcome::Unreadable(_)));
    }

    #[test]
    fn loopback_server_gives_a_newer_version_and_only_the_generic_agent_is_sent() {
        let (url, seen) = serve("200 OK", r#"{"tag_name":"v99.0.0","html_url":"https://github.com/phobicdotno/DarkBurn/releases/tag/v99.0.0"}"#, 1);
        let o = check(&url, "0.0.24");
        assert!(matches!(&o, Outcome::Available { latest, .. } if latest == "99.0.0"), "{o:?}");
        let req = seen.lock().unwrap()[0].to_ascii_lowercase();
        assert!(req.contains("user-agent: darkburn\r\n"), "{req}");
        for forbidden in ["cookie", "authorization", "x-", "referer"] {
            assert!(!req.contains(&format!("\r\n{forbidden}")), "{forbidden} must not be sent: {req}");
        }
    }

    #[test]
    fn loopback_server_same_version_is_up_to_date() {
        let (url, _) = serve("200 OK", r#"{"tag_name":"v0.0.24"}"#, 1);
        assert!(matches!(check(&url, "0.0.24"), Outcome::UpToDate { .. }));
    }

    #[test]
    fn private_repository_answers_are_explained_not_errors() {
        for (status, code) in [("404 Not Found", 404), ("401 Unauthorized", 401), ("403 Forbidden", 403)] {
            let (url, _) = serve(status, r#"{"message":"Not Found"}"#, 1);
            let o = check(&url, "0.0.24");
            assert_eq!(o, Outcome::NotVisible { status: code });
            assert!(o.message().contains("private"), "{}", o.message());
        }
        let (url, _) = serve("500 Internal Server Error", "boom", 1);
        assert!(matches!(check(&url, "0.0.24"), Outcome::Failed(m) if m.contains("500")));
    }

    #[test]
    fn bad_answers_and_dead_servers_do_not_panic() {
        let (url, _) = serve("200 OK", "this is not json", 1);
        assert!(matches!(check(&url, "0.0.24"), Outcome::Unreadable(_)));
        // A port nothing listens on: bind, note the port, drop the listener.
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert!(matches!(check(&format!("http://127.0.0.1:{port}/x"), "0.0.24"), Outcome::Failed(_)));
        assert!(matches!(check("http://example.com/x", "0.0.24"), Outcome::Failed(m) if m.contains("https")), "plain http is only for loopback");
        assert!(matches!(check("ftp://127.0.0.1/x", "0.0.24"), Outcome::Failed(_)));
    }

    /// A loopback server that answers every request with a redirect to `to`.
    fn redirect_to(to: String) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for _ in 0..4 {
                let Ok((mut s, _)) = listener.accept() else { return };
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let _ = s.write_all(format!("HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes());
            }
        });
        format!("http://127.0.0.1:{}/releases/latest", addr.port())
    }

    /// The address rule holds for every hop: a server cannot send the check on to an address
    /// that would have been refused as the setting. The "other" address here is this computer
    /// under a name the rule does not allow (127.0.0.2), so nothing leaves it.
    #[test]
    fn a_redirect_to_an_address_that_is_not_allowed_is_not_followed() {
        let Ok(listener) = TcpListener::bind("127.0.0.2:0") else { return };
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let target = format!("http://127.0.0.2:{port}/releases/latest");
        assert!(!allowed_address(&target));
        let o = check(&redirect_to(target), "0.0.24");
        assert!(matches!(&o, Outcome::Failed(m) if m.contains("https")), "{o:?}");
        assert!(listener.accept().is_err(), "the check went on to the address it was redirected to");
    }

    #[test]
    fn a_redirect_to_an_allowed_address_is_followed() {
        let (url, seen) = serve("200 OK", r#"{"tag_name":"v99.0.0"}"#, 1);
        let o = check(&redirect_to(url), "0.0.24");
        assert!(matches!(&o, Outcome::Available { latest, .. } if latest == "99.0.0"), "{o:?}");
        assert_eq!(seen.lock().unwrap().len(), 1);
        // A chain that never ends is given up.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let me = format!("http://127.0.0.1:{}/x", listener.local_addr().unwrap().port());
        let to = me.clone();
        std::thread::spawn(move || {
            for _ in 0..10 {
                let Ok((mut s, _)) = listener.accept() else { return };
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let _ = s.write_all(format!("HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes());
            }
        });
        assert!(matches!(check(&me, "0.0.24"), Outcome::Failed(_)));
    }

    #[test]
    fn a_secure_address_never_steps_down_to_plain_http() {
        assert!(next_hop("https://example.com/a", "https://example.org/b").is_ok());
        assert_eq!(next_hop("https://example.com/a/b", "/c").unwrap(), "https://example.com/c");
        assert!(next_hop("https://example.com/a", "http://example.com/a").is_err());
        assert!(next_hop("https://example.com/a", "http://127.0.0.1:8080/a").is_err(), "a server cannot point the check at this computer");
        assert!(next_hop("http://127.0.0.1:5/a", "http://127.0.0.1:6/a").is_ok());
        assert!(next_hop("http://127.0.0.1:5/a", "ftp://x/a").is_err());
    }

    #[test]
    fn the_address_comes_from_the_setting_or_the_default() {
        // The environment variable is not touched here (tests run in parallel); only the setting.
        if std::env::var_os(URL_ENV).is_none() {
            assert_eq!(address(""), DEFAULT_URL);
            assert_eq!(address("  http://127.0.0.1:1/x "), "http://127.0.0.1:1/x");
        }
    }

    #[test]
    fn the_command_runs_a_check_in_the_background_and_reports() {
        let (url, _) = serve("200 OK", r#"{"tag_name":"v99.0.0"}"#, 1);
        let mut app = DarkBurn::new(crate::prefs::Prefs::default());
        app.prefs.opts.update_url = url;
        let r = crate::api::execute_headless(&mut app, "help.check_updates", &serde_json::Value::Null);
        assert!(r.is_ok(), "{r:?}");
        let end = std::time::Instant::now() + Duration::from_secs(10);
        while !app.extras.updates.poll() {
            assert!(std::time::Instant::now() < end, "no answer");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(matches!(app.extras.updates.result, Some(Outcome::Available { .. })));
        assert!(app.extras.updates.open);
    }

    #[test]
    fn the_setting_is_off_by_default() {
        assert!(!crate::prefs::Prefs::default().opts.check_updates);
    }

    /// Run frames until the check that `frame` started has answered.
    fn frames_until_answer(app: &mut DarkBurn) {
        let ctx = Context::default();
        let end = std::time::Instant::now() + Duration::from_secs(10);
        frame(&ctx, app);
        while app.extras.updates.running() {
            assert!(std::time::Instant::now() < end, "no answer");
            std::thread::sleep(Duration::from_millis(20));
            frame(&ctx, app);
        }
    }

    #[test]
    fn with_the_setting_on_a_start_checks_once_and_only_a_newer_version_opens_the_dialog() {
        // A newer release: the dialog opens by itself.
        let (url, seen) = serve("200 OK", r#"{"tag_name":"v99.0.0","html_url":"https://github.com/phobicdotno/DarkBurn/releases/tag/v99.0.0"}"#, 1);
        let mut app = DarkBurn::new(crate::prefs::Prefs::default());
        app.prefs.opts.check_updates = true;
        app.prefs.opts.update_url = url;
        frames_until_answer(&mut app);
        assert!(matches!(app.extras.updates.result, Some(Outcome::Available { .. })));
        assert!(app.extras.updates.open, "a newer version is shown");
        assert!(app.note.is_none(), "no status note for a check nobody asked for");
        // Later frames do not check again.
        frames_until_answer(&mut app);
        assert_eq!(seen.lock().unwrap().len(), 1);

        // The same version: the answer is kept, nothing pops up.
        let body: &'static str = Box::leak(format!(r#"{{"tag_name":"v{}"}}"#, env!("CARGO_PKG_VERSION")).into_boxed_str());
        let (url, seen) = serve("200 OK", body, 1);
        let mut app = DarkBurn::new(crate::prefs::Prefs::default());
        app.prefs.opts.check_updates = true;
        app.prefs.opts.update_url = url;
        frames_until_answer(&mut app);
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert!(matches!(app.extras.updates.result, Some(Outcome::UpToDate { .. })));
        assert!(!app.extras.updates.open, "up to date: no dialog at start");

        // A server that cannot be reached, or a private repository: silence at start.
        let (url, _) = serve("404 Not Found", r#"{"message":"Not Found"}"#, 1);
        let mut app = DarkBurn::new(crate::prefs::Prefs::default());
        app.prefs.opts.check_updates = true;
        app.prefs.opts.update_url = url;
        frames_until_answer(&mut app);
        assert!(matches!(app.extras.updates.result, Some(Outcome::NotVisible { status: 404 })));
        assert!(!app.extras.updates.open);
    }

    #[test]
    fn no_check_at_start_with_the_setting_off_or_in_safe_mode() {
        for (setting, safe) in [(false, false), (true, true)] {
            let (url, seen) = serve("200 OK", r#"{"tag_name":"v99.0.0"}"#, 1);
            let mut app = DarkBurn::new(crate::prefs::Prefs::default());
            app.prefs.opts.check_updates = setting;
            app.safe_mode = safe;
            app.prefs.opts.update_url = url;
            let ctx = Context::default();
            for _ in 0..5 {
                frame(&ctx, &mut app);
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(!app.extras.updates.running() && app.extras.updates.result.is_none());
            assert!(!app.extras.updates.open);
            assert!(seen.lock().unwrap().is_empty(), "nothing was asked of the server");
        }
    }

    #[test]
    fn the_dialog_shows_the_new_version_and_opens_only_a_safe_download_page() {
        use crate::ui_harness::Gui;
        let mut g = Gui::standard();
        {
            let mut a = g.app();
            a.extras.updates.result = Some(Outcome::Available {
                current: "0.0.24".into(),
                latest: "99.0.0".into(),
                url: "https://github.com/phobicdotno/DarkBurn/releases/tag/v99.0.0".into(),
                notes: "Faster previews.".into(),
            });
            a.extras.updates.open = true;
        }
        g.frames(2);
        assert!(g.window_titles().contains(&"Check for Updates".to_string()), "{:?}", g.window_titles());
        let texts = g.visible_texts().join(" | ");
        assert!(texts.contains("99.0.0") && texts.contains("Faster previews."), "{texts}");
        assert!(texts.contains("Nothing is downloaded"), "{texts}");
        g.screenshot("update-available");
        crate::help::take_opened();
        g.click_label("Open the download page");
        assert_eq!(crate::help::take_opened(), ["https://github.com/phobicdotno/DarkBurn/releases/tag/v99.0.0"]);
        assert!(!g.app().extras.updates.open, "the dialog closed");

        // A link that is not a plain web address is never handed to the system.
        {
            let mut a = g.app();
            a.extras.updates.result = Some(Outcome::Available {
                current: "0.0.24".into(),
                latest: "99.0.0".into(),
                url: "file:///C:/Windows/System32/calc.exe".into(),
                notes: String::new(),
            });
            a.extras.updates.open = true;
        }
        g.frames(2);
        g.click_label("Open the download page");
        assert_eq!(crate::help::take_opened(), [RELEASES_PAGE]);

        // Up to date: no download button, Close closes.
        {
            let mut a = g.app();
            a.extras.updates.result = Some(Outcome::UpToDate { current: "0.0.24".into() });
            a.extras.updates.open = true;
        }
        g.frames(2);
        assert!(!g.has_text("Open the download page"));
        assert!(g.visible_texts().join(" ").contains("up to date"));
        g.close_dialogs();
        assert!(!g.app().extras.updates.open);
    }

    #[test]
    fn help_menu_check_for_updates_opens_the_dialog() {
        use crate::ui_harness::Gui;
        let mut g = Gui::standard();
        g.open_menu("Help", "Check for Updates");
        g.frames(2);
        assert!(g.app().extras.updates.open);
        assert!(g.window_titles().contains(&"Check for Updates".to_string()));
        // Under test the network is never touched: the dialog says the check failed.
        assert!(matches!(g.app().extras.updates.result, Some(Outcome::Failed(_))));
        g.close_dialogs();
    }
}

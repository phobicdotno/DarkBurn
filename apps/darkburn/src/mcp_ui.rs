//! MCP in the window: the preferences, the control server's life (started only when the user
//! turned "MCP control" on), the "AI agents (MCP)" settings page, the status-bar chip, the
//! confirmation dialog, crash reports, and the windowless `--mcp-headless` mode.
//!
//! The safety rules live in `darkburn_mcp::Gate`; this file only gives the user the switches
//! and shows what the agent is doing. Nothing here opens a port unless `McpPrefs::enabled`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use darkburn_mcp::control::{ControlConfig, ControlServer, control_file_path};
use darkburn_mcp::{Backend, Event, Gate, McpPolicy, PolicyHandle};
use darkburn_ui::ui_kit;
use egui::{FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solidcraft_theme::Tokens;

use crate::mcp_backend::{McpBackend, SharedApp};
use crate::shell::DarkBurn;

pub const DEFAULT_PORT: u16 = darkburn_mcp::control::DEFAULT_PORT;
const AUDIT_KEEP: usize = 300;

/// What the user chose in Settings > AI agents (MCP). Everything starts off or restrictive.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpPrefs {
    /// Run the control server so an AI agent can use this window. Off: no port is opened.
    pub enabled: bool,
    pub port: u16,
    /// "Allow AI agents to control machines".
    pub allow_hardware: bool,
    /// "Allow Gweike commands" (on top of the machine switch).
    pub allow_gweike: bool,
    /// "Allow file writes".
    pub allow_file_write: bool,
    /// "Require my confirmation to start a job or fire".
    pub require_confirmation: bool,
}

impl Default for McpPrefs {
    fn default() -> Self {
        Self { enabled: false, port: DEFAULT_PORT, allow_hardware: false, allow_gweike: false, allow_file_write: true, require_confirmation: true }
    }
}

impl McpPrefs {
    pub fn policy(&self) -> McpPolicy {
        McpPolicy {
            allow_hardware: self.allow_hardware,
            allow_file_write: self.allow_file_write,
            require_confirmation_for_job_start: self.require_confirmation,
            allow_gweike: self.allow_gweike,
        }
    }
}

/// What the settings page and the status chip read: a snapshot the host refreshes every frame.
#[derive(Clone, Default)]
pub struct Live {
    pub running: bool,
    pub port: u16,
    pub clients: usize,
    pub discovery: Option<PathBuf>,
    pub error: Option<String>,
    pub audit: Vec<Event>,
    pub open_confirmations: usize,
    pub policy: McpPolicy,
}

static LIVE: OnceLock<Mutex<Live>> = OnceLock::new();
static GOTO_PAGE: AtomicBool = AtomicBool::new(false);

fn live() -> MutexGuard<'static, Live> {
    LIVE.get_or_init(|| Mutex::new(Live::default())).lock().unwrap_or_else(|e| e.into_inner())
}

pub fn snapshot() -> Live {
    live().clone()
}

/// The settings window should open on the AI agents page next time it draws.
pub fn take_goto_page() -> bool {
    GOTO_PAGE.swap(false, Ordering::SeqCst)
}

/// Owns the control server and the gate around the app's backend.
pub struct Host {
    app: SharedApp,
    policy: PolicyHandle,
    gate: Option<Gate<McpBackend>>,
    server: Option<ControlServer>,
    bound_port: Option<u16>,
    failed_port: Option<u16>,
    cursor: u64,
}

impl Host {
    pub fn new(app: SharedApp) -> Self {
        Self { app, policy: PolicyHandle::default(), gate: None, server: None, bound_port: None, failed_port: None, cursor: 0 }
    }

    fn stop(&mut self) {
        self.server = None; // dropping removes control.json and closes every client
        self.gate = None;
        self.bound_port = None;
        let mut l = live();
        l.running = false;
        l.clients = 0;
        l.open_confirmations = 0;
    }

    fn start(&mut self, ctx: &egui::Context, cfg: &McpPrefs) {
        let waker_ctx = ctx.clone();
        let discovery = control_file_path();
        let config = ControlConfig {
            port: cfg.port,
            discovery_file: Some(discovery.clone()),
            waker: Some(Arc::new(move || waker_ctx.request_repaint())),
            ..Default::default()
        };
        match ControlServer::bind(config) {
            Ok(server) => {
                self.policy = PolicyHandle::new(cfg.policy());
                let backend = McpBackend::new(self.app.clone(), ctx.clone(), false);
                self.gate = Some(Gate::new(backend, self.policy.clone()));
                self.bound_port = Some(server.port());
                self.server = Some(server);
                self.failed_port = None;
                let mut l = live();
                l.running = true;
                l.port = cfg.port;
                l.discovery = Some(discovery);
                l.error = None;
            }
            Err(e) => {
                self.failed_port = Some(cfg.port);
                let mut l = live();
                l.running = false;
                l.error = Some(format!("could not listen on 127.0.0.1:{}: {e}", cfg.port));
            }
        }
    }

    /// Once per frame, from `logic()` (no borrow of the app is outstanding there).
    pub fn frame(&mut self, ctx: &egui::Context) {
        let cfg = match self.app.try_borrow() {
            Ok(a) => a.prefs.mcp.clone(),
            Err(_) => return,
        };
        self.policy.set(cfg.policy());
        live().policy = cfg.policy();
        if self.server.is_some() && (!cfg.enabled || self.bound_port != Some(cfg.port)) {
            self.stop();
        }
        if !cfg.enabled {
            self.failed_port = None;
            live().error = None;
            return;
        }
        if self.server.is_none() && self.failed_port != Some(cfg.port) {
            self.start(ctx, &cfg);
        }
        let (Some(server), Some(gate)) = (&self.server, &mut self.gate) else { return };
        gate.inner().poll();
        server.pump(gate, 32);
        if gate.inner().busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        let fresh = gate.events(self.cursor).map(|p| (p.events, p.next)).unwrap_or_default();
        self.cursor = self.cursor.max(fresh.1);
        let open = gate.open_confirmations().len();
        let clients = server.connections();
        let mut l = live();
        l.clients = clients;
        l.open_confirmations = open;
        l.audit.extend(fresh.0);
        if l.audit.len() > AUDIT_KEEP {
            let cut = l.audit.len() - AUDIT_KEEP;
            l.audit.drain(..cut);
        }
    }

    /// The confirmation dialog: drawn while an agent waits for the user's yes or no. Confirm and
    /// Cancel are plain buttons; neither is the keyboard default.
    #[allow(deprecated, reason = "egui::Id::new is the only stable-id constructor for a one-off window")]
    pub fn confirmation_ui(&mut self, ctx: &egui::Context) {
        let Some(gate) = &mut self.gate else { return };
        let open = gate.open_confirmations();
        let Some((id, command, summary)) = open.into_iter().next() else { return };
        let t = Tokens::get();
        let mut answer = None;
        let window = egui::Id::new(("db_mcp", "confirm"));
        egui::Window::new("An AI agent wants to start the machine")
            .id(window)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_max_width(460.0);
                ui.label(egui::RichText::new("The agent asks to:").color(t.text_dim));
                ui.label(egui::RichText::new(&summary).strong());
                ui.add_space(t.space_6);
                ui.label(format!("Command: {command}"));
                ui.label(
                    egui::RichText::new("Check the machine, the material and the design before you confirm. Cancel refuses it.").color(t.text_dim),
                );
                ui.add_space(t.space_8);
                ui.horizontal(|ui| {
                    if ui.button("Confirm").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(false);
                    }
                });
            });
        // Escape refuses, like Cancel. Nothing on the keyboard confirms.
        if answer.is_none() && crate::escape::closes(ctx, window) {
            answer = Some(false);
        }
        if let Some(yes) = answer {
            let _ = gate.resolve_confirmation(&id, yes);
        }
    }
}

// ----- status chip ------------------------------------------------------------------------------

/// The "MCP" chip in the status bar, shown while the control server runs. Returns the new right
/// edge for the text to its left.
pub fn status_chip(ui: &mut Ui, bar: Rect, right: f32, app: &mut DarkBurn) -> f32 {
    let l = snapshot();
    if !l.running {
        return right;
    }
    let t = Tokens::get();
    let font = FontId::proportional(t.font_small);
    let hardware = l.policy.allow_hardware;
    let risky = hardware && (!l.policy.require_confirmation_for_job_start || l.policy.allow_gweike);
    let (fill, line, ink) = if risky {
        (egui::Color32::from_rgb(120, 30, 30), egui::Color32::from_rgb(235, 80, 80), egui::Color32::WHITE)
    } else if hardware {
        (egui::Color32::from_rgb(110, 80, 10), egui::Color32::from_rgb(235, 170, 40), egui::Color32::WHITE)
    } else {
        (t.accent_soft, t.accent, t.selected_ink)
    };
    let label = if l.clients > 0 { format!("MCP {}", l.clients) } else { "MCP".to_string() };
    let g = ui.painter().layout_no_wrap(label, font, ink);
    let w = g.size().x + 12.0;
    let br = Rect::from_min_size(pos2(right - w, bar.top() + 3.0), vec2(w, bar.height() - 6.0));
    let resp = ui.interact(br, ui.scope_id().with("mcp_chip"), Sense::click());
    ui.painter().rect(br, t.radius_sm, fill, Stroke::new(1.0, line), egui::StrokeKind::Inside);
    ui.painter().galley(pos2(br.left() + 6.0, br.center().y - g.size().y / 2.0), g, ink);
    let tip = format!(
        "MCP control is on (127.0.0.1:{}), {} agent{} connected. Machines: {}. Click for the settings.",
        l.port,
        l.clients,
        if l.clients == 1 { "" } else { "s" },
        if hardware {
            if risky { "agents may drive them WITHOUT asking" } else { "agents may drive them after you confirm" }
        } else {
            "agents cannot touch them"
        },
    );
    let resp = resp.on_hover_text(tip.clone());
    if resp.hovered() {
        app.hint = Some(tip);
    }
    if resp.clicked() {
        GOTO_PAGE.store(true, Ordering::SeqCst);
        app.settings_open = true;
    }
    br.left() - 8.0
}

// ----- the settings page --------------------------------------------------------------------------

fn copy_button(ui: &mut Ui, label: &str, text: &str) {
    if ui_kit::button(ui, label, ui_kit::Button::Small).clicked() {
        ui.ctx().copy_text(text.to_string());
    }
}

fn mono_box(ui: &mut Ui, text: &str) {
    let t = Tokens::get();
    egui::Frame::NONE.fill(t.control_fill).stroke(Stroke::new(1.0, t.dialog_border)).corner_radius(t.radius_sm).inner_margin(6.0).show(ui, |ui| {
        ui.add(egui::Label::new(egui::RichText::new(text).monospace().size(t.font_small)).wrap());
    });
}

pub fn server_exe() -> PathBuf {
    std::env::current_exe().map(|e| darkburn_mcp::config::server_path_next_to(&e)).unwrap_or_else(|_| PathBuf::from(darkburn_mcp::config::SERVER_EXE))
}

fn time_text(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}:{:02}", (s / 3600) % 24, (s / 60) % 60, s % 60)
}

/// Settings > AI agents (MCP).
pub fn settings_page(ui: &mut Ui, prefs: &mut crate::prefs::Prefs) {
    let t = Tokens::get();
    let live = snapshot();
    let m = &mut prefs.mcp;
    ui_kit::section_heading(ui, "AI agents (MCP)");
    ui.add(egui::Label::new(
        egui::RichText::new(
            "Lets an AI agent (Claude Code, Claude Desktop) use DarkBurn through MCP: draw and edit designs, change cut settings, preview jobs, take screenshots. It is off until you turn it on, and it only listens on this computer.",
        )
        .color(t.text_dim),
    ));
    ui.add_space(t.space_8);
    ui_kit::checkbox(ui, "MCP control", &mut m.enabled);
    ui_kit::row(ui, "Port", |ui| {
        let mut port = u32::from(m.port);
        ui_kit::number_field(ui, &mut port, 1024..=65535, 1.0, "");
        m.port = port.clamp(1024, 65535) as u16;
    });
    if live.running {
        ui.label(egui::RichText::new(format!("Listening on 127.0.0.1:{}. Connected agents: {}.", live.port, live.clients)).color(t.text));
    } else if let Some(e) = &live.error {
        ui.label(egui::RichText::new(e).color(egui::Color32::from_rgb(235, 80, 80)));
    } else {
        ui.label(egui::RichText::new("Off: DarkBurn opens no port.").color(t.text_dim));
    }
    let discovery = live.discovery.clone().unwrap_or_else(control_file_path);
    ui_kit::row(ui, "Connection file", |ui| {
        ui.label(egui::RichText::new(discovery.display().to_string()).monospace().size(t.font_small));
    });
    ui.horizontal(|ui| {
        copy_button(ui, "Copy path", &discovery.display().to_string());
    });
    ui.add_space(t.space_8);
    let exe = server_exe();
    let add = darkburn_mcp::config::claude_add_command(&exe);
    ui.label("Register it with Claude Code (once):");
    mono_box(ui, &add);
    ui.horizontal(|ui| {
        copy_button(ui, "Copy command", &add);
        copy_button(ui, "Copy Claude Desktop JSON", &darkburn_mcp::config::desktop_config_json(&exe));
    });
    ui.add_space(t.space_12);
    ui_kit::section_heading(ui, "What agents may do");
    ui_kit::checkbox(ui, "Allow file writes (save, export, G-code files)", &mut m.allow_file_write);
    ui_kit::checkbox(ui, "Allow AI agents to control machines", &mut m.allow_hardware);
    ui.add_enabled_ui(m.allow_hardware, |ui| {
        ui_kit::checkbox(ui, "Allow Gweike commands", &mut m.allow_gweike);
        ui_kit::checkbox(ui, "Require my confirmation to start a job or fire", &mut m.require_confirmation);
    });
    if m.allow_hardware {
        let warn = if m.require_confirmation {
            "Agents can connect and move the machine. Starting a job or firing the laser still shows a Confirm / Cancel dialog. Stop and emergency stop always work."
        } else {
            "WARNING: agents can start jobs and fire the laser without asking you. Stay at the machine."
        };
        ui.label(egui::RichText::new(warn).color(egui::Color32::from_rgb(235, 170, 40)));
    } else {
        ui.label(
            egui::RichText::new("Agents cannot connect to, move or fire any machine. Only stop and emergency stop are open to them.")
                .color(t.text_dim),
        );
    }
    ui.add_space(t.space_12);
    ui_kit::section_heading(ui, "Audit log");
    if live.audit.is_empty() {
        ui.label(egui::RichText::new("Nothing yet. Every call an agent makes is listed here.").color(t.text_dim));
    }
    egui::ScrollArea::vertical().max_height(180.0).stick_to_bottom(true).show(ui, |ui| {
        for e in live.audit.iter().filter(|e| matches!(e.kind.as_str(), "audit" | "confirmation")) {
            let what = if e.command.is_empty() { e.tool.clone() } else { format!("{} {}", e.tool, e.command) };
            let line = format!("{}  {:<10} {}  {}", time_text(e.ts_ms), e.kind, what.trim(), e.outcome);
            let bad = !matches!(e.outcome.as_str(), "ok" | "approved" | "requested");
            ui.label(egui::RichText::new(line).monospace().size(t.font_small).color(if bad {
                egui::Color32::from_rgb(235, 170, 40)
            } else {
                t.text
            }));
        }
    });
}

// ----- crash reports ------------------------------------------------------------------------------

fn crash_dir(settings: &Path) -> PathBuf {
    settings.join("crash-reports")
}

/// Write a report when the app panics (kept next to the settings), then run the default hook.
pub fn install_panic_hook(settings: Option<PathBuf>) {
    let Some(dir) = settings else { return };
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let d = crash_dir(&dir);
        let _ = std::fs::create_dir_all(&d);
        let text = format!("DarkBurn {} panicked\n{info}\n\nbacktrace:\n{}\n", env!("CARGO_PKG_VERSION"), std::backtrace::Backtrace::force_capture());
        let _ = std::fs::write(d.join(format!("crash-{}.txt", darkburn_mcp::now_ms())), text);
        previous(info);
    }));
}

/// The newest crash reports (text cut to 4000 characters) and the tail of the log.
pub fn crash_reports(settings: Option<&Path>, limit: usize) -> Value {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    if let Some(dir) = settings.map(crash_dir)
        && let Ok(rd) = std::fs::read_dir(dir)
    {
        for e in rd.flatten() {
            if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
                files.push((m, e.path()));
            }
        }
    }
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    let reports: Vec<Value> = files
        .into_iter()
        .take(limit.max(1))
        .map(|(_, p)| {
            let text = std::fs::read_to_string(&p).unwrap_or_default();
            json!({"file": p.display().to_string(), "text": text.chars().take(4000).collect::<String>()})
        })
        .collect();
    let log = crate::help::recent_log();
    let skip = log.chars().count().saturating_sub(3000);
    json!({"reports": reports, "log_tail": log.chars().skip(skip).collect::<String>()})
}

// ----- headless -----------------------------------------------------------------------------------

/// `darkburn --mcp-headless`: the app's backend without a window, MCP over stdio. No GUI, no
/// settings file, default preferences; machines are limited to simulators.
pub fn run_headless(allow_simulator_control: bool) -> std::process::ExitCode {
    use std::io::{BufReader, stdin, stdout};
    let app: SharedApp = Rc::new(RefCell::new(DarkBurn::new(crate::prefs::Prefs::default())));
    let backend = McpBackend::new(app, egui::Context::default(), true);
    let policy = McpPolicy { allow_hardware: allow_simulator_control, ..McpPolicy::default() };
    let gate = Gate::new(backend, PolicyHandle::new(policy));
    let boxed: Box<dyn Backend> = Box::new(gate);
    let mut server = darkburn_mcp::mcp_server(boxed);
    match server.serve(BufReader::new(stdin().lock()), stdout().lock()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("darkburn --mcp-headless: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use darkburn_mcp::PendingState;

    fn frame(ctx: &egui::Context, host: &mut Host, events: Vec<egui::Event>) {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1000.0, 700.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| host.confirmation_ui(ui.ctx()));
        out.textures_delta.clear();
    }

    fn press(key: egui::Key) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }
    }

    #[test]
    fn escape_refuses_the_agents_request_and_no_key_confirms_it() {
        let app: SharedApp = Rc::new(RefCell::new(DarkBurn::new(crate::prefs::Prefs::default())));
        let ctx = egui::Context::default();
        let mut host = Host::new(app.clone());
        let policy = McpPolicy { allow_hardware: true, ..McpPolicy::default() };
        let mut gate = Gate::new(McpBackend::new(app, ctx.clone(), true), PolicyHandle::new(policy));
        let pid = gate.execute("job.start", json!({})).unwrap_err().pending_id.expect("the request waits for the user");
        host.gate = Some(gate);
        let open = |host: &mut Host| host.gate.as_mut().unwrap().open_confirmations().len();
        frame(&ctx, &mut host, vec![]);
        assert_eq!(open(&mut host), 1);
        // Enter and Space do nothing: only a click on Confirm says yes.
        frame(&ctx, &mut host, vec![press(egui::Key::Enter), press(egui::Key::Space)]);
        assert_eq!(open(&mut host), 1);
        // Escape refuses: the request ends with an error and no token.
        frame(&ctx, &mut host, vec![press(egui::Key::Escape)]);
        assert_eq!(open(&mut host), 0);
        let state = host.gate.as_mut().unwrap().pending(&pid).unwrap();
        assert!(matches!(state, PendingState::Done { result: Err(_), .. }), "refused");
    }
}

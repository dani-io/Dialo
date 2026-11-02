//! The window: the keypad and recent calls when idle, the call screen while a call is on,
//! and three overlays (settings, the command palette, the list of shortcuts). Everything
//! is reachable from the keyboard; see `COMMANDS`.

use crate::contacts::{self, Book, Contact};
use crate::config::{self, Account, Direction, HistoryEntry, Settings, ThemeName, Transport};
use crate::engine::{CallState, Cmd, Engine, Event, Reg};
use crate::ipc::Remote;
use crate::icons::{self, Icon};
use crate::theme::{self, Palette};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Id, Key, Modifiers, Order, Pos2, Rect, Sense, Stroke, StrokeKind, TextEdit, Ui, UiBuilder, Vec2, pos2, vec2,
};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const NUMBER_ID: &str = "number";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Action {
    CallOrAnswer,
    HangUp,
    Mute,
    Hold,
    Keypad,
    Redial,
    Settings,
    Contacts,
    Import,
    NextTheme,
    Shortcuts,
    ClearHistory,
    Quit,
}

/// Every command, with its keys, for the palette and the shortcuts list.
pub const COMMANDS: &[(Action, &str, &str)] = &[
    (Action::CallOrAnswer, "Call / answer", "Enter"),
    (Action::HangUp, "Hang up / decline", "Esc (twice in a call)"),
    (Action::Mute, "Mute microphone", "Ctrl+M"),
    (Action::Hold, "Hold / resume", "Ctrl+H"),
    (Action::Keypad, "Show or hide the keypad", "Ctrl+D"),
    (Action::Redial, "Call the last number again", "Ctrl+R"),
    (Action::Contacts, "Contacts", "Ctrl+B"),
    (Action::Import, "Import contacts (vCard)", "Ctrl+I"),
    (Action::Settings, "Account and settings", "Ctrl+,"),
    (Action::NextTheme, "Next theme", "Ctrl+T"),
    (Action::Shortcuts, "Keyboard shortcuts", "F1"),
    (Action::ClearHistory, "Clear recent calls", ""),
    (Action::Quit, "Quit Dialo", "Ctrl+Q"),
];

struct ActiveCall {
    state: CallState,
    peer: String,
    name: String,
    outgoing: bool,
    talking_since: Option<Instant>,
    muted: bool,
    held: bool,
    remote_held: bool,
    pad: bool,
    dtmf: String,
    /// Set when Esc was pressed once while talking: a second Esc within 1.5 s hangs up.
    esc_at: Option<Instant>,
    /// The desktop notification of a ringing call, to close once it is answered.
    notification: Arc<Mutex<Option<u32>>>,
    generation: u64,
}

impl ActiveCall {
    fn title(&self) -> String {
        if !self.name.trim().is_empty() {
            return self.name.trim().to_string();
        }
        user_part(&self.peer)
    }
}

enum Overlay {
    None,
    Settings { draft: Account, theme: ThemeName, notify: bool, show_pass: bool, first: bool },
    Palette { query: String, sel: usize },
    Shortcuts,
    Contacts { query: String, sel: usize },
}

/// A line under the number: a contact's phone, or a recent call.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Row {
    Contact(usize, usize),
    Recent(usize),
}

/// What an import, on its worker, came back with.
type Imported = Result<(String, Vec<Contact>), String>;

pub struct App {
    engine: Engine,
    events: Receiver<Event>,
    remote: Receiver<Remote>,
    notif_tx: Sender<(u64, bool)>,
    notif_rx: Receiver<(u64, bool)>,
    settings: Settings,
    pal: Palette,
    history: Vec<HistoryEntry>,
    input: String,
    sel: Option<usize>,
    reg: Reg,
    reg_text: String,
    fatal: Option<String>,
    call: Option<ActiveCall>,
    generation: u64,
    overlay: Overlay,
    toasts: Vec<(String, Instant)>,
    pad: bool,
    themed: Option<ThemeName>,
    book: Book,
    import_tx: Sender<Imported>,
    import_rx: Receiver<Imported>,
    /// A file chooser is open (on its worker).
    picking: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext, engine: Engine, events: Receiver<Event>, remote: Receiver<Remote>) -> App {
        theme::fonts(&cc.egui_ctx);
        let settings = Settings::load();
        let first = !settings.account.is_complete();
        if !first {
            engine.send(Cmd::SetAccount(Some(settings.account.clone())));
        }
        let (notif_tx, notif_rx) = channel();
        let (import_tx, import_rx) = channel();
        let overlay = if first {
            Overlay::Settings { draft: settings.account.clone(), theme: settings.theme, notify: settings.notify, show_pass: false, first: true }
        } else {
            Overlay::None
        };
        App {
            engine,
            events,
            remote,
            notif_tx,
            notif_rx,
            pal: settings.theme.palette(),
            settings,
            history: config::load_history(),
            input: String::new(),
            sel: None,
            reg: Reg::Off,
            reg_text: String::new(),
            fatal: None,
            call: None,
            generation: 0,
            overlay,
            toasts: Vec::new(),
            pad: true,
            themed: None,
            book: Book::load(),
            import_tx,
            import_rx,
            picking: false,
        }
    }

    fn toast(&mut self, s: impl Into<String>) {
        self.toasts.push((s.into(), Instant::now()));
    }

    /// What shows under the number: with nothing typed, recent calls; else contacts
    /// that match, then recents that match. Recents are newest first, one per address.
    fn matches(&self) -> Vec<Row> {
        let q = self.input.trim().to_lowercase();
        let mut out: Vec<Row> = self.book.search(&q, 30).into_iter().map(|(c, p)| Row::Contact(c, p)).collect();
        let digits: String = q.chars().filter(|c| c.is_ascii_digit() || *c == '+').collect();
        let mut seen = std::collections::HashSet::new();
        for (i, h) in self.history.iter().enumerate().rev() {
            if !seen.insert(h.uri.clone()) {
                continue;
            }
            let name = self.name_for(h);
            let hay = format!("{} {}", name.to_lowercase(), h.uri.to_lowercase());
            let ok = q.is_empty() || hay.contains(&q) || (!digits.is_empty() && digits.len() == q.len() && user_part(&h.uri).contains(&digits));
            if ok {
                out.push(Row::Recent(i));
            }
            if out.len() >= 60 {
                break;
            }
        }
        out
    }

    /// The number or address a row calls.
    fn row_target(&self, r: Row) -> String {
        match r {
            Row::Contact(c, p) => self.book.list[c].phones[p].number.clone(),
            Row::Recent(i) => self.history[i].uri.clone(),
        }
    }

    /// A recent call's name: what the call said, else the address book's.
    fn name_for(&self, h: &HistoryEntry) -> String {
        if !h.name.trim().is_empty() {
            return h.name.trim().to_string();
        }
        self.book.lookup(&h.uri).map(|(c, _)| c.name.clone()).unwrap_or_default()
    }

    /// Opens a file chooser for a .vcf, on a worker: zenity or kdialog, whichever is
    /// installed. Without either, says how else to import.
    fn pick_vcf(&mut self, ctx: &egui::Context) {
        if self.picking {
            return;
        }
        self.picking = true;
        let tx = self.import_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let tries: [(&str, &[&str]); 2] = [
                ("zenity", &["--file-selection", "--title=Import contacts", "--file-filter=vCard (*.vcf) | *.vcf *.VCF", "--file-filter=All files | *"]),
                ("kdialog", &["--title", "Import contacts", "--getopenfilename", ".", "*.vcf *.VCF|vCard"]),
            ];
            let mut ran = false;
            for (cmd, args) in tries {
                let Ok(out) = std::process::Command::new(cmd).args(args).stderr(std::process::Stdio::null()).output() else { continue };
                ran = true;
                let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if out.status.success() && !path.is_empty() {
                    let _ = tx.send(import_file(&path));
                } else {
                    let _ = tx.send(Ok((String::new(), Vec::new())));
                }
                break;
            }
            if !ran {
                let _ = tx.send(Err("No file chooser: drop the .vcf on Dialo, or run  dialo import FILE.vcf".into()));
            }
            ctx.request_repaint();
        });
    }

    fn take_imports(&mut self, ctx: &egui::Context) {
        // Files dropped on the window.
        let dropped: Vec<_> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        for p in dropped {
            let r = import_file(&p.to_string_lossy());
            let _ = self.import_tx.send(r);
        }
        while let Ok(r) = self.import_rx.try_recv() {
            self.picking = false;
            match r {
                Ok((_, list)) if list.is_empty() => {}
                Ok((from, list)) => {
                    let n = list.len();
                    let added = self.book.add(list);
                    match self.book.save() {
                        Ok(()) if added == n => self.toast(format!("Added {added} contacts from {from}")),
                        Ok(()) => self.toast(format!("Added {added} new of {n} contacts")),
                        Err(e) => self.toast(format!("Could not save the contacts: {e}")),
                    }
                }
                Err(e) => self.toast(e),
            }
        }
    }

    fn dial(&mut self, typed: String) {
        if self.call.is_some() || typed.trim().is_empty() {
            return;
        }
        if !self.settings.account.is_complete() {
            self.toast("Set up your SIP account first");
            self.open_settings();
            return;
        }
        self.engine.send(Cmd::Dial(typed));
    }

    fn open_settings(&mut self) {
        self.overlay = Overlay::Settings {
            draft: self.settings.account.clone(),
            theme: self.settings.theme,
            notify: self.settings.notify,
            show_pass: false,
            first: false,
        };
    }

    fn run(&mut self, ctx: &egui::Context, a: Action) {
        match a {
            Action::CallOrAnswer => match &self.call {
                Some(c) if c.state == CallState::Incoming => self.engine.send(Cmd::Answer),
                Some(_) => {}
                None => {
                    let m = self.matches();
                    let target = match self.sel.and_then(|s| m.get(s)) {
                        Some(&r) => self.row_target(r),
                        None => self.input.clone(),
                    };
                    self.dial(target);
                }
            },
            Action::HangUp => {
                if self.call.is_some() {
                    self.engine.send(Cmd::Hangup);
                }
            }
            Action::Mute => {
                if let Some(c) = &mut self.call {
                    c.muted = !c.muted;
                    self.engine.send(Cmd::Mute(c.muted));
                }
            }
            Action::Hold => {
                if let Some(c) = &mut self.call {
                    if c.state == CallState::Talking {
                        c.held = !c.held;
                        self.engine.send(Cmd::Hold(c.held));
                    }
                }
            }
            Action::Keypad => match &mut self.call {
                Some(c) => c.pad = !c.pad,
                None => self.pad = !self.pad,
            },
            Action::Redial => {
                if let Some(h) = self.history.iter().rev().find(|h| h.direction == Direction::Outgoing) {
                    let uri = h.uri.clone();
                    self.dial(uri);
                } else {
                    self.toast("No call to repeat yet");
                }
            }
            Action::Settings => self.open_settings(),
            Action::Contacts => self.overlay = Overlay::Contacts { query: String::new(), sel: 0 },
            Action::Import => self.pick_vcf(ctx),
            Action::NextTheme => {
                let all = ThemeName::ALL;
                let i = all.iter().position(|t| *t == self.settings.theme).unwrap_or(0);
                self.settings.theme = all[(i + 1) % all.len()];
                let _ = self.settings.save();
                self.toast(format!("Theme: {}", self.settings.theme.label()));
            }
            Action::Shortcuts => self.overlay = Overlay::Shortcuts,
            Action::ClearHistory => {
                self.history.clear();
                config::save_history(&self.history);
                self.sel = None;
                self.toast("Recent calls cleared");
            }
            Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }

    fn handle_events(&mut self, ctx: &egui::Context) {
        while let Ok(e) = self.events.try_recv() {
            match e {
                Event::Failed(why) => self.fatal = Some(why),
                Event::Registration(r, text) => {
                    if r == Reg::Failed && self.reg != Reg::Failed {
                        let t = if text.is_empty() { "Registration failed".to_string() } else { format!("Registration failed: {text}") };
                        self.toast(t);
                    }
                    self.reg = r;
                    self.reg_text = text;
                }
                Event::Call { state, peer, name } => {
                    let is_new = self.call.is_none();
                    if is_new {
                        self.generation += 1;
                        // The address book's name, when the call brings none.
                        let name = if name.trim().is_empty() { self.book.lookup(&peer).map(|(c, _)| c.name.clone()).unwrap_or_default() } else { name.clone() };
                        let outgoing = state != CallState::Incoming;
                        self.call = Some(ActiveCall {
                            state,
                            peer: peer.clone(),
                            name: name.clone(),
                            outgoing,
                            talking_since: None,
                            muted: false,
                            held: false,
                            remote_held: false,
                            pad: false,
                            dtmf: String::new(),
                            esc_at: None,
                            notification: Arc::new(Mutex::new(None)),
                            generation: self.generation,
                        });
                        if state == CallState::Incoming {
                            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Critical));
                            if self.settings.notify {
                                let c = self.call.as_ref().unwrap();
                                notify_incoming(&c.title(), &peer, c.generation, c.notification.clone(), self.notif_tx.clone(), ctx.clone());
                            }
                        }
                    }
                    let c = self.call.as_mut().unwrap();
                    if state == CallState::Talking && c.talking_since.is_none() {
                        c.talking_since = Some(Instant::now());
                        close_notification(&c.notification);
                    }
                    c.state = state;
                    if !peer.is_empty() {
                        c.peer = peer;
                    }
                    if !name.is_empty() && c.name.is_empty() {
                        c.name = name;
                    }
                }
                Event::Ended { reason, answered } => {
                    if let Some(c) = self.call.take() {
                        close_notification(&c.notification);
                        let secs = c.talking_since.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                        let direction = if c.outgoing {
                            Direction::Outgoing
                        } else if answered {
                            Direction::Incoming
                        } else if reason == "Hung up" {
                            Direction::Declined
                        } else {
                            Direction::Missed
                        };
                        self.history.push(HistoryEntry { uri: c.peer.clone(), name: c.name.clone(), direction, at: config::now(), secs });
                        if self.history.len() > config::HISTORY_MAX {
                            let extra = self.history.len() - config::HISTORY_MAX;
                            self.history.drain(..extra);
                        }
                        config::save_history(&self.history);
                        if !answered && c.outgoing && reason != "Hung up" {
                            self.toast(reason);
                        } else if direction == Direction::Missed {
                            self.toast(format!("Missed call from {}", c.title()));
                        }
                        self.input.clear();
                        self.sel = None;
                    }
                }
                Event::RemoteHold(on) => {
                    if let Some(c) = &mut self.call {
                        c.remote_held = on;
                    }
                }
                Event::Notice(s) => self.toast(s),
            }
        }
        while let Ok((generation, answer)) = self.notif_rx.try_recv() {
            if self.call.as_ref().is_some_and(|c| c.generation == generation && c.state == CallState::Incoming) {
                self.engine.send(if answer { Cmd::Answer } else { Cmd::Hangup });
            }
        }
        while let Ok(r) = self.remote.try_recv() {
            match r {
                Remote::Call(n) => {
                    if self.call.is_none() {
                        self.input = n.clone();
                        self.dial(n);
                    }
                }
                Remote::Answer => self.run(ctx, Action::CallOrAnswer),
                Remote::Hangup => self.run(ctx, Action::HangUp),
                Remote::ToggleMute => self.run(ctx, Action::Mute),
                Remote::ToggleHold => self.run(ctx, Action::Hold),
                Remote::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                Remote::Quit => self.run(ctx, Action::Quit),
                Remote::Import(p) => {
                    let r = import_file(&p);
                    let _ = self.import_tx.send(r);
                }
            }
        }
    }

    /// Keys that mean the same everywhere, then the ones for the screen showing.
    fn handle_keys(&mut self, ctx: &egui::Context) {
        let pressed = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        let ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;

        if pressed(ctrl_shift, Key::P) || pressed(Modifiers::CTRL, Key::K) {
            self.overlay = match self.overlay {
                Overlay::Palette { .. } => Overlay::None,
                _ => Overlay::Palette { query: String::new(), sel: 0 },
            };
            return;
        }
        if pressed(Modifiers::NONE, Key::F1) {
            self.overlay = Overlay::Shortcuts;
        }
        if pressed(Modifiers::CTRL, Key::Q) {
            return self.run(ctx, Action::Quit);
        }
        if !matches!(self.overlay, Overlay::None) {
            return;
        }
        if pressed(Modifiers::CTRL, Key::Comma) {
            return self.run(ctx, Action::Settings);
        }
        if pressed(Modifiers::CTRL, Key::T) {
            self.run(ctx, Action::NextTheme);
        }
        if pressed(Modifiers::CTRL, Key::B) {
            return self.run(ctx, Action::Contacts);
        }
        if pressed(Modifiers::CTRL, Key::I) {
            self.run(ctx, Action::Import);
        }
        if pressed(Modifiers::CTRL, Key::M) {
            self.run(ctx, Action::Mute);
        }
        if pressed(Modifiers::CTRL, Key::H) {
            self.run(ctx, Action::Hold);
        }
        if pressed(Modifiers::CTRL, Key::D) {
            self.run(ctx, Action::Keypad);
        }
        if pressed(Modifiers::CTRL, Key::R) {
            self.run(ctx, Action::Redial);
        }
        if pressed(Modifiers::NONE, Key::Enter) {
            self.run(ctx, Action::CallOrAnswer);
        }

        if let Some(c) = &mut self.call {
            if pressed(Modifiers::NONE, Key::Escape) {
                let now = Instant::now();
                let confirm = c.state != CallState::Talking || c.esc_at.is_some_and(|t| t.elapsed().as_secs_f32() < 1.5);
                if confirm {
                    self.engine.send(Cmd::Hangup);
                } else {
                    c.esc_at = Some(now);
                    self.toasts.push(("Press Esc again to hang up".into(), now));
                }
            }
            // Digits typed during a call go out as tones.
            if c.state == CallState::Talking {
                let typed: Vec<char> = ctx.input(|i| {
                    i.events
                        .iter()
                        .filter_map(|e| if let egui::Event::Text(t) = e { Some(t.clone()) } else { None })
                        .flat_map(|t| t.chars().collect::<Vec<_>>())
                        .filter(|ch| ch.is_ascii_digit() || matches!(ch, '*' | '#' | 'A'..='D'))
                        .collect()
                });
                for ch in typed {
                    self.send_dtmf(ch);
                }
            }
        } else {
            if pressed(Modifiers::NONE, Key::Escape) {
                self.input.clear();
                self.sel = None;
            }
            let n = self.matches().len();
            if pressed(Modifiers::NONE, Key::ArrowDown) && n > 0 {
                self.sel = Some(self.sel.map_or(0, |s| (s + 1).min(n - 1)));
            }
            if pressed(Modifiers::NONE, Key::ArrowUp) {
                self.sel = match self.sel {
                    Some(0) | None => None,
                    Some(s) => Some(s - 1),
                };
            }
        }
    }

    fn send_dtmf(&mut self, ch: char) {
        if let Some(c) = &mut self.call {
            c.dtmf.push(ch);
            if c.dtmf.chars().count() > 24 {
                c.dtmf.remove(0);
            }
            self.engine.send(Cmd::Dtmf(ch));
        }
    }

    // ---- drawing -------------------------------------------------------------------

    fn header(&mut self, ui: &mut Ui, rect: Rect) {
        let p = self.pal;
        let painter = ui.painter_at(rect);
        let (color, text) = match self.reg {
            Reg::Ok => (p.good, "Online".to_string()),
            Reg::Registering => (p.warn, "Connecting…".to_string()),
            Reg::Failed => (p.bad, if self.reg_text.is_empty() { "Not registered".into() } else { self.reg_text.clone() }),
            Reg::Off => (p.dim, "No account".to_string()),
        };
        let account = if self.settings.account.is_complete() {
            let a = &self.settings.account;
            if a.display_name.trim().is_empty() { format!("{}@{}", a.username.trim(), a.domain.trim()) } else { a.display_name.trim().to_string() }
        } else {
            "Dialo".to_string()
        };
        let y = rect.center().y;
        let t = ui.input(|i| i.time) as f32;
        let pulse = if self.reg == Reg::Registering { 0.6 + 0.4 * (t * 4.0).sin().abs() } else { 1.0 };
        painter.circle_filled(pos2(rect.left() + 6.0, y), 4.5, color.gamma_multiply(pulse));
        painter.circle_filled(pos2(rect.left() + 6.0, y), 8.0, color.gamma_multiply(0.18 * pulse));
        let g = painter.layout_no_wrap(account, FontId::proportional(14.0), p.text);
        let w = g.size().x;
        painter.galley(pos2(rect.left() + 20.0, y - 17.0), g, p.text);
        painter.text(pos2(rect.left() + 20.0, y + 1.0), Align2::LEFT_TOP, elide(&text, 40), FontId::proportional(11.5), p.dim);
        let _ = w;

        // Palette and settings, on the right.
        for (i, (icon, action, tip)) in [(Icon::Gear, Action::Settings, "Settings  Ctrl+,"), (Icon::Search, Action::Shortcuts, "Commands  Ctrl+K")].into_iter().enumerate() {
            let c = pos2(rect.right() - 14.0 - i as f32 * 34.0, y);
            let r = Rect::from_center_size(c, vec2(30.0, 30.0));
            let resp = ui.interact(r, Id::unique(("hdr", i)), Sense::click()).on_hover_text(tip);
            if resp.hovered() {
                painter.rect_filled(r, CornerRadius::same(8), p.hover);
            }
            icons::draw(&painter, icon, c, 18.0, if resp.hovered() { p.text } else { p.dim });
            if resp.clicked() {
                if action == Action::Shortcuts {
                    self.overlay = Overlay::Palette { query: String::new(), sel: 0 };
                } else {
                    self.run(ui.ctx(), action);
                }
            }
        }
    }

    fn idle(&mut self, ui: &mut Ui, rect: Rect) {
        let p = self.pal;
        let ctx = ui.ctx().clone();
        // The number, big and centered.
        let field = Rect::from_min_size(rect.min, vec2(rect.width(), 52.0));
        let before = self.input.clone();
        let edit = TextEdit::singleline(&mut self.input)
            .id(Id::unique(NUMBER_ID))
            .font(FontId::proportional(28.0))
            .align(Align2::CENTER_CENTER)
            .frame(egui::Frame::NONE)
            .hint_text(egui::RichText::new("Number or name").color(p.dim.gamma_multiply(0.7)))
            .desired_width(rect.width());
        ui.put(field, edit);
        if self.input != before {
            self.sel = None;
        }
        if matches!(self.overlay, Overlay::None) && ctx.memory(|m| m.focused().is_none()) {
            ctx.memory_mut(|m| m.request_focus(Id::unique(NUMBER_ID)));
        }
        let underline = field.left_bottom().y;
        ui.painter().hline(field.x_range().shrink(field.width() * 0.18), underline, Stroke::new(1.0, p.hover));

        // Bottom: the call button, with backspace beside it.
        let bottom_h = 84.0;
        let bottom = Rect::from_min_max(pos2(rect.left(), rect.bottom() - bottom_h), rect.max);
        let call_c = bottom.center();
        let can_call = !self.input.trim().is_empty() || self.sel.is_some();
        let fill = if can_call { p.good } else { p.good.gamma_multiply(0.45) };
        if round_button(ui, call_c, 32.0, fill, Icon::Phone, ink_on(p.good), Id::unique("call"), "Call  Enter").clicked() {
            self.run(&ctx, Action::CallOrAnswer);
        }
        if !self.input.is_empty() {
            let bc = pos2(call_c.x + 78.0, call_c.y);
            let r = round_button(ui, bc, 22.0, Color32::TRANSPARENT, Icon::Backspace, p.dim, Id::unique("bksp"), "Delete  Backspace");
            if r.clicked() {
                self.input.pop();
            }
            if r.secondary_clicked() || r.long_touched() {
                self.input.clear();
            }
        }
        let kp = pos2(call_c.x - 78.0, call_c.y);
        if round_button(ui, kp, 22.0, Color32::TRANSPARENT, Icon::Keypad, if self.pad { p.accent } else { p.dim }, Id::unique("kp"), "Keypad  Ctrl+D").clicked() {
            self.pad = !self.pad;
        }

        // The keypad above it, when shown; the recents fill what is left.
        let mut list_bottom = bottom.top() - 6.0;
        // Typing a name: the keypad steps aside for the matches.
        let by_name = self.input.chars().any(|c| c.is_alphabetic());
        if self.pad && !by_name {
            let pad_h = (rect.height() * 0.46).clamp(220.0, 300.0);
            let pad = Rect::from_min_max(pos2(rect.left(), bottom.top() - pad_h), pos2(rect.right(), bottom.top() - 4.0));
            if let Some(k) = keypad(ui, pad, &p, "pad") {
                self.input.push(k);
                self.sel = None;
            }
            list_bottom = pad.top() - 6.0;
        }
        let list = Rect::from_min_max(pos2(rect.left(), underline + 12.0), pos2(rect.right(), list_bottom));
        if list.height() > 40.0 {
            self.recents(ui, list);
        }
    }

    fn recents(&mut self, ui: &mut Ui, rect: Rect) {
        let p = self.pal;
        let m = self.matches();
        let mut child = ui.new_child(UiBuilder::new().max_rect(rect));
        if m.is_empty() {
            let msg = if !self.input.trim().is_empty() {
                "Enter calls this number"
            } else if self.history.is_empty() && self.book.list.is_empty() {
                "Calls you make and get show up here.\nCtrl+I imports contacts."
            } else if self.history.is_empty() {
                "Calls you make and get show up here"
            } else {
                ""
            };
            child.painter().text(rect.center_top() + vec2(0.0, 30.0), Align2::CENTER_CENTER, msg, FontId::proportional(13.0), p.dim);
            return;
        }
        let label = if self.input.trim().is_empty() { "RECENT" } else { "MATCHES" };
        child.label(egui::RichText::new(label).size(10.5).color(p.dim).strong());
        let mut clicked = None;
        egui::ScrollArea::vertical().id_salt("recents").auto_shrink([false, false]).show(&mut child, |ui| {
            for (row, &r) in m.iter().enumerate() {
                let selected = self.sel == Some(row);
                let (title, sub, right, seed, missed) = match r {
                    Row::Contact(ci, pi) => {
                        let c = &self.book.list[ci];
                        let ph = &c.phones[pi];
                        let sub = if ph.label.is_empty() { ph.number.clone() } else { format!("{} · {}", ph.label, ph.number) };
                        (c.name.clone(), sub, String::new(), c.name.clone(), false)
                    }
                    Row::Recent(i) => {
                        let h = &self.history[i];
                        let name = self.name_for(h);
                        let title = if name.is_empty() { user_part(&h.uri) } else { name };
                        let (arrow, what) = match h.direction {
                            Direction::Outgoing => ("↗", "Outgoing"),
                            Direction::Incoming => ("↙", "Incoming"),
                            Direction::Missed => ("↙", "Missed"),
                            Direction::Declined => ("↙", "Declined"),
                        };
                        let sub = if h.secs > 0 { format!("{arrow} {what} · {}", duration(h.secs)) } else { format!("{arrow} {what}") };
                        (title, sub, ago(h.at), h.uri.clone(), h.direction == Direction::Missed)
                    }
                };
                if contact_row(ui, &p, &title, &sub, &right, &seed, missed, selected, matches!(r, Row::Contact(..))).clicked() {
                    clicked = Some(r);
                }
            }
        });
        if let Some(r) = clicked {
            let t = self.row_target(r);
            self.dial(t);
        }
    }

    fn in_call(&mut self, ui: &mut Ui, rect: Rect) {
        let p = self.pal;
        let ctx = ui.ctx().clone();
        let t = ui.input(|i| i.time) as f32;
        let Some(c) = &self.call else { return };
        let title = c.title();
        let peer = c.peer.clone();
        let state = c.state;
        let pad = c.pad;
        let (muted, held, remote_held) = (c.muted, c.held, c.remote_held);
        let status = match state {
            CallState::Dialing => "Calling…".to_string(),
            CallState::Ringing => "Ringing…".to_string(),
            CallState::Incoming => "Incoming call".to_string(),
            CallState::Talking if held => "On hold".to_string(),
            CallState::Talking if remote_held => "Held by the other side".to_string(),
            CallState::Talking => clock(c.talking_since.map(|s| s.elapsed().as_secs()).unwrap_or(0)),
        };
        let dtmf = c.dtmf.clone();
        let painter = ui.painter_at(rect);

        // The avatar, with rings going out from it while the call rings.
        let compact = pad;
        let av_r = if compact { 34.0 } else { 56.0 };
        let av_c = pos2(rect.center().x, rect.top() + if compact { 48.0 } else { 96.0 });
        if matches!(state, CallState::Dialing | CallState::Ringing | CallState::Incoming) {
            let accent = if state == CallState::Incoming { p.good } else { p.accent };
            for k in 0..3 {
                let phase = ((t * 0.7) + k as f32 / 3.0).fract();
                let r = av_r + 4.0 + phase * 30.0;
                painter.circle_stroke(av_c, r, Stroke::new(2.0, accent.gamma_multiply((1.0 - phase) * 0.55)));
            }
            ctx.request_repaint();
        } else if state == CallState::Talking && !muted && !held {
            // A slow glow, so a live call looks alive.
            let g = 0.10 + 0.06 * (t * 1.6).sin();
            painter.circle_filled(av_c, av_r + 10.0, p.good.gamma_multiply(g));
        }
        avatar(&painter, av_c, av_r, &title, &peer, av_r * 0.75);
        let mut y = av_c.y + av_r + if compact { 16.0 } else { 40.0 };
        painter.text(pos2(rect.center().x, y), Align2::CENTER_TOP, elide(&title, 28), FontId::proportional(22.0), p.text);
        y += 30.0;
        if user_part(&peer) != title {
            painter.text(pos2(rect.center().x, y), Align2::CENTER_TOP, elide(&strip_sip(&peer), 40), FontId::proportional(12.5), p.dim);
            y += 20.0;
        }
        let status_col = if held || remote_held { p.warn } else if state == CallState::Talking { p.good } else { p.dim };
        let status_font = if state == CallState::Talking && !held && !remote_held { FontId::monospace(15.0) } else { FontId::proportional(14.0) };
        painter.text(pos2(rect.center().x, y), Align2::CENTER_TOP, status, status_font, status_col);
        y += 26.0;
        if muted {
            painter.text(pos2(rect.center().x, y), Align2::CENTER_TOP, "Microphone off", FontId::proportional(12.0), p.bad);
            y += 20.0;
        }
        if state == CallState::Talking {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }

        // Bottom: hang up, or decline and answer.
        let bottom = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 90.0), rect.max);
        let bc = bottom.center();
        if state == CallState::Incoming {
            if round_button(&mut *ui, pos2(bc.x - 70.0, bc.y), 32.0, p.bad, Icon::HangUp, ink_on(p.bad), Id::unique("decline"), "Decline  Esc").clicked() {
                self.engine.send(Cmd::Hangup);
            }
            // The answer button breathes, to say it's the one to press.
            let s = 1.0 + 0.05 * (t * 5.0).sin();
            if round_button(&mut *ui, pos2(bc.x + 70.0, bc.y), 32.0 * s, p.good, Icon::Phone, ink_on(p.good), Id::unique("answer"), "Answer  Enter").clicked() {
                self.engine.send(Cmd::Answer);
            }
            return;
        }
        if round_button(&mut *ui, bc, 32.0, p.bad, Icon::HangUp, ink_on(p.bad), Id::unique("hangup"), "Hang up  Esc Esc").clicked() {
            self.engine.send(Cmd::Hangup);
        }

        // Mute, keypad and hold above it.
        let talking = state == CallState::Talking;
        let row_y = bottom.top() - 44.0;
        let items = [(if muted { Icon::MicOff } else { Icon::Mic }, "Mute", muted, Action::Mute, "Ctrl+M"), (Icon::Keypad, "Keypad", pad, Action::Keypad, "Ctrl+D"), (Icon::Pause, "Hold", held, Action::Hold, "Ctrl+H")];
        for (i, (icon, label, on, action, keys)) in items.into_iter().enumerate() {
            let x = rect.center().x + (i as f32 - 1.0) * 84.0;
            let enabled = talking || action != Action::Hold;
            let fill = if on { p.text } else { p.raised };
            let ink = if on { p.panel } else if enabled { p.text } else { p.dim.gamma_multiply(0.5) };
            let resp = round_button(&mut *ui, pos2(x, row_y), 26.0, fill, icon, ink, Id::unique(("ctl", i)), &format!("{label}  {keys}"));
            ui.painter().text(pos2(x, row_y + 32.0), Align2::CENTER_TOP, label, FontId::proportional(11.0), p.dim);
            if resp.clicked() && enabled {
                self.run(&ctx, action);
            }
        }

        if pad {
            let pad_rect = Rect::from_min_max(pos2(rect.left(), y + 24.0), pos2(rect.right(), row_y - 40.0));
            if !dtmf.is_empty() {
                ui.painter().text(pos2(rect.center().x, y), Align2::CENTER_TOP, &dtmf, FontId::monospace(16.0), p.accent);
            }
            if pad_rect.height() > 120.0 {
                if let Some(k) = keypad(&mut *ui, pad_rect, &p, "dtmf") {
                    if talking {
                        self.send_dtmf(k);
                    }
                }
            }
        }
    }

    fn overlays(&mut self, ctx: &egui::Context, screen: Rect) {
        if matches!(self.overlay, Overlay::None) {
            return;
        }
        let p = self.pal;
        // Dim what's behind; a click on it closes the overlay (but not the first setup).
        let first = matches!(self.overlay, Overlay::Settings { first: true, .. });
        egui::Area::new(Id::unique("scrim")).order(Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
            let r = ui.allocate_rect(screen, Sense::click());
            ui.painter().rect_filled(screen, CornerRadius::ZERO, Color32::from_black_alpha(if p.dark { 150 } else { 90 }));
            if r.clicked() && !first {
                self.overlay = Overlay::None;
            }
        });
        let width = (screen.width() - 24.0).min(420.0);
        let mut close = false;
        let mut action = None;
        let mut save = None;
        egui::Area::new(Id::unique("overlay")).order(Order::Foreground).anchor(Align2::CENTER_TOP, vec2(0.0, 28.0)).show(ctx, |ui| {
            egui::Frame::NONE.fill(p.panel).corner_radius(CornerRadius::same(16)).stroke(Stroke::new(1.0, p.hover)).inner_margin(18).shadow(egui::Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(90) }).show(ui, |ui| {
                ui.set_width(width - 36.0);
                match &mut self.overlay {
                    Overlay::None => {}
                    Overlay::Palette { query, sel } => {
                        let resp = ui.add(TextEdit::singleline(query).hint_text("Type a command, or a number to call").font(FontId::proportional(16.0)).desired_width(f32::INFINITY).frame(egui::Frame::NONE));
                        resp.request_focus();
                        ui.separator();
                        let q = query.to_lowercase();
                        let list: Vec<_> = COMMANDS.iter().filter(|(_, l, _)| q.is_empty() || fuzzy(&l.to_lowercase(), &q)).collect();
                        let looks_like_number = !q.is_empty() && q.chars().all(|c| c.is_ascii_digit() || "+*# -".contains(c));
                        let n = list.len() + looks_like_number as usize;
                        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) && n > 0 {
                            *sel = (*sel + 1).min(n - 1);
                        }
                        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
                            *sel = sel.saturating_sub(1);
                        }
                        *sel = (*sel).min(n.saturating_sub(1));
                        let enter = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
                        if looks_like_number {
                            let r = palette_row(ui, &p, &format!("Call {query}"), "Enter", *sel == 0);
                            if r.clicked() || (enter && *sel == 0) {
                                save = Some(Err(query.clone()));
                            }
                        }
                        for (k, (a, label, keys)) in list.iter().enumerate() {
                            let idx = k + looks_like_number as usize;
                            let r = palette_row(ui, &p, label, keys, *sel == idx);
                            if r.clicked() || (enter && *sel == idx) {
                                action = Some(*a);
                            }
                        }
                        if n == 0 {
                            ui.label(egui::RichText::new("Nothing matches").color(p.dim));
                        }
                        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                            close = true;
                        }
                    }
                    Overlay::Contacts { query, sel } => {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Contacts").size(18.0).strong());
                            ui.label(egui::RichText::new(format!("{}", self.book.list.len())).color(p.dim));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("Import vCard…").on_hover_text("Ctrl+I, or drop a .vcf on the window").clicked() {
                                    action = Some(Action::Import);
                                }
                            });
                        });
                        let resp = ui.add(TextEdit::singleline(query).hint_text("Search names, companies, numbers").font(FontId::proportional(15.0)).desired_width(f32::INFINITY).margin(vec2(8.0, 6.0)));
                        resp.request_focus();
                        ui.add_space(4.0);
                        let rows: Vec<(usize, usize)> = if query.trim().is_empty() {
                            self.book.list.iter().enumerate().flat_map(|(ci, c)| (0..c.phones.len()).map(move |pi| (ci, pi))).collect()
                        } else {
                            self.book.search(query, 500)
                        };
                        let n = rows.len();
                        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) && n > 0 {
                            *sel = (*sel + 1).min(n - 1);
                        }
                        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
                            *sel = sel.saturating_sub(1);
                        }
                        *sel = (*sel).min(n.saturating_sub(1));
                        let enter = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
                        if self.book.list.is_empty() {
                            ui.add_space(10.0);
                            ui.label(egui::RichText::new("No contacts yet. In Google Contacts choose Export > vCard, then import the file here or drop it on the window.").color(p.dim));
                        } else if n == 0 {
                            ui.label(egui::RichText::new("Nothing matches").color(p.dim));
                        }
                        egui::ScrollArea::vertical().max_height(screen.height() - 170.0).auto_shrink([false, true]).show(ui, |ui| {
                            for (k, &(ci, pi)) in rows.iter().enumerate() {
                                let c = &self.book.list[ci];
                                let ph = &c.phones[pi];
                                let sub = if ph.label.is_empty() { ph.number.clone() } else { format!("{} · {}", ph.label, ph.number) };
                                let r = contact_row(ui, &p, &c.name, &sub, &c.org, &c.name, false, *sel == k, true);
                                if r.clicked() || (enter && *sel == k) {
                                    save = Some(Err(ph.number.clone()));
                                }
                            }
                        });
                        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                            close = true;
                        }
                    }
                    Overlay::Shortcuts => {
                        ui.label(egui::RichText::new("Keyboard shortcuts").size(18.0).strong());
                        ui.add_space(4.0);
                        egui::Grid::new("keys").num_columns(2).spacing([24.0, 8.0]).show(ui, |ui| {
                            for (_, label, keys) in COMMANDS.iter().filter(|c| !c.2.is_empty()) {
                                ui.label(*label);
                                ui.label(egui::RichText::new(*keys).monospace().color(p.accent));
                                ui.end_row();
                            }
                            for (label, keys) in [("Command palette", "Ctrl+K, Ctrl+Shift+P"), ("Contacts and recents", "↑ ↓, then Enter"), ("Tones during a call", "0-9 * #"), ("Clear the number", "Esc")] {
                                ui.label(label);
                                ui.label(egui::RichText::new(keys).monospace().color(p.accent));
                                ui.end_row();
                            }
                        });
                        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape) || i.consume_key(Modifiers::NONE, Key::F1)) {
                            close = true;
                        }
                    }
                    Overlay::Settings { draft, theme, notify, show_pass, first } => {
                        ui.label(egui::RichText::new(if *first { "Welcome to Dialo" } else { "Settings" }).size(19.0).strong());
                        if *first {
                            ui.label(egui::RichText::new("Add your SIP account (from your PBX or provider) to start calling.").color(p.dim));
                        }
                        ui.add_space(6.0);
                        let field = |ui: &mut Ui, label: &str, value: &mut String, hint: &str, pass: bool| {
                            ui.label(egui::RichText::new(label).size(11.5).color(p.dim));
                            ui.add(TextEdit::singleline(value).hint_text(hint).password(pass).desired_width(f32::INFINITY).margin(vec2(8.0, 6.0)));
                        };
                        egui::ScrollArea::vertical().max_height(screen.height() - 190.0).show(ui, |ui| {
                            field(ui, "Server (PBX or provider)", &mut draft.domain, "pbx.example.com or 192.168.1.10:5060", false);
                            ui.columns(2, |cols| {
                                field(&mut cols[0], "Extension / user", &mut draft.username, "201", false);
                                cols[1].label(egui::RichText::new("Password").size(11.5).color(p.dim));
                                cols[1].horizontal(|ui| {
                                    ui.add(TextEdit::singleline(&mut draft.password).password(!*show_pass).desired_width(ui.available_width() - 34.0).margin(vec2(8.0, 6.0)));
                                    if icon_toggle(ui, Icon::Eye, *show_pass, &p).on_hover_text("Show the password").clicked() {
                                        *show_pass = !*show_pass;
                                    }
                                });
                            });
                            field(ui, "Your name (shown to people you call)", &mut draft.display_name, "Optional", false);
                            ui.label(egui::RichText::new("Transport").size(11.5).color(p.dim));
                            ui.horizontal(|ui| {
                                for t in Transport::ALL {
                                    ui.selectable_value(&mut draft.transport, t, t.name().to_uppercase());
                                }
                                ui.add_space(12.0);
                                ui.checkbox(&mut draft.srtp, "Encrypt audio (SRTP)");
                            });
                            egui::CollapsingHeader::new(egui::RichText::new("Advanced").color(p.dim)).show(ui, |ui| {
                                field(ui, "Authentication user", &mut draft.auth_user, "When it differs from the extension", false);
                                field(ui, "Outbound proxy", &mut draft.proxy, "Optional", false);
                                field(ui, "STUN server (behind NAT)", &mut draft.stun, "stun.l.google.com:19302", false);
                            });
                            ui.add_space(6.0);
                            ui.label(egui::RichText::new("Look").size(11.5).color(p.dim));
                            ui.horizontal_wrapped(|ui| {
                                for t in ThemeName::ALL {
                                    ui.selectable_value(theme, t, t.label());
                                }
                            });
                            ui.checkbox(notify, "Notify me of incoming calls");
                        });
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let ok = draft.is_complete();
                            let save_btn = ui.add_enabled(ok, egui::Button::new(egui::RichText::new("  Save  ").color(Color32::WHITE).strong()).fill(p.accent));
                            let save_key = ui.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::S) || (ok && i.consume_key(Modifiers::NONE, Key::Enter)));
                            if save_btn.clicked() || (ok && save_key) {
                                save = Some(Ok((draft.clone(), *theme, *notify)));
                            }
                            if !*first && ui.button("Cancel").clicked() {
                                close = true;
                            }
                            ui.label(egui::RichText::new("Ctrl+S saves").size(11.0).color(p.dim));
                        });
                        if !*first && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                            close = true;
                        }
                    }
                }
            });
        });
        if close {
            self.overlay = Overlay::None;
        }
        if let Some(a) = action {
            self.overlay = Overlay::None;
            self.run(ctx, a);
        }
        match save {
            Some(Ok((acc, theme, notify))) => {
                let changed = acc != self.settings.account;
                self.settings.account = acc;
                self.settings.theme = theme;
                self.settings.notify = notify;
                if let Err(e) = self.settings.save() {
                    self.toast(format!("Could not save the settings: {e}"));
                }
                if changed || self.reg != Reg::Ok {
                    self.engine.send(Cmd::SetAccount(Some(self.settings.account.clone())));
                }
                self.overlay = Overlay::None;
            }
            Some(Err(number)) => {
                self.overlay = Overlay::None;
                self.input = number.clone();
                self.dial(number);
            }
            None => {}
        }
    }

    fn toasts(&mut self, ctx: &egui::Context, screen: Rect) {
        if !matches!(self.overlay, Overlay::None) {
            return;
        }
        self.toasts.retain(|(_, t)| t.elapsed().as_secs_f32() < 3.5);
        if let Some((text, at)) = self.toasts.last() {
            let p = self.pal;
            let age = at.elapsed().as_secs_f32();
            let alpha = (age / 0.15).min(1.0).min((3.5 - age) / 0.4).clamp(0.0, 1.0);
            let text = text.clone();
            egui::Area::new(Id::unique("toast")).order(Order::Tooltip).interactable(false).fixed_pos(pos2(screen.center().x, screen.top() + 64.0)).pivot(Align2::CENTER_TOP).show(ctx, |ui| {
                egui::Frame::NONE.fill(p.text.gamma_multiply(alpha)).corner_radius(CornerRadius::same(18)).inner_margin(egui::Margin::symmetric(16, 8)).show(ui, |ui| {
                    ui.label(egui::RichText::new(text).color(p.panel.gamma_multiply(alpha)).size(13.0));
                });
            });
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.themed != Some(self.settings.theme) {
            self.pal = self.settings.theme.palette();
            theme::apply(&ctx, &self.pal);
            self.themed = Some(self.settings.theme);
        }
        self.handle_events(&ctx);
        self.take_imports(&ctx);
        self.handle_keys(&ctx);

        let screen = ui.max_rect();
        ui.painter().rect_filled(screen, CornerRadius::ZERO, self.pal.panel);
        let inner = screen.shrink2(vec2(18.0, 14.0));
        let header = Rect::from_min_size(inner.min, vec2(inner.width(), 40.0));
        self.header(ui, header);
        let body = Rect::from_min_max(pos2(inner.left(), header.bottom() + 14.0), inner.max);

        if let Some(why) = &self.fatal {
            ui.painter().text(body.center(), Align2::CENTER_CENTER, format!("Dialo could not start SIP:\n{why}"), FontId::proportional(14.0), self.pal.bad);
        } else if self.call.is_some() {
            self.in_call(ui, body);
        } else {
            self.idle(ui, body);
        }
        self.overlays(&ctx, screen);
        self.toasts(&ctx, screen);
        // Registration pulses while connecting.
        if self.reg == Reg::Registering {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        self.pal.panel.to_normalized_gamma_f32()
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.call.is_some() {
            self.engine.send(Cmd::Hangup);
        }
    }
}

// ---- pieces ------------------------------------------------------------------------

fn round_button(ui: &mut Ui, center: Pos2, radius: f32, fill: Color32, icon: Icon, ink: Color32, id: Id, tip: &str) -> egui::Response {
    let rect = Rect::from_center_size(center, Vec2::splat(radius * 2.0));
    let resp = ui.interact(rect, id, Sense::click()).on_hover_text(tip);
    let t = ui.ctx().animate_bool(id.with("hover"), resp.hovered());
    let press = if resp.is_pointer_button_down_on() { 0.94 } else { 1.0 };
    let painter = ui.painter();
    let r = radius * (1.0 + 0.04 * t) * press;
    if fill == Color32::TRANSPARENT {
        if t > 0.0 {
            painter.circle_filled(center, r, ui.visuals().widgets.hovered.bg_fill.gamma_multiply(t));
        }
    } else {
        painter.circle_filled(center + vec2(0.0, 3.0), r, Color32::from_black_alpha(40));
        painter.circle_filled(center, r, fill);
        // Lit from above: lighter at the top, the colour itself at the bottom.
        gradient_circle(painter, center, r, fill.lerp_to_gamma(Color32::WHITE, 0.10 + 0.08 * t), fill);
    }
    icons::draw(painter, icon, center, radius * 0.95, ink);
    resp
}

fn icon_toggle(ui: &mut Ui, icon: Icon, on: bool, p: &Palette) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
    if on || resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(8), if on { p.accent.gamma_multiply(0.3) } else { p.hover });
    }
    icons::draw(ui.painter(), icon, r.center(), 18.0, if on { p.text } else { p.dim });
    resp
}

/// The 12 keys; returns the one clicked. Right-click on 0 types +.
fn keypad(ui: &mut Ui, rect: Rect, p: &Palette, salt: &str) -> Option<char> {
    const KEYS: [(char, &str); 12] = [
        ('1', ""),
        ('2', "ABC"),
        ('3', "DEF"),
        ('4', "GHI"),
        ('5', "JKL"),
        ('6', "MNO"),
        ('7', "PQRS"),
        ('8', "TUV"),
        ('9', "WXYZ"),
        ('*', ""),
        ('0', "+"),
        ('#', ""),
    ];
    let cw = (rect.width() / 3.0).min(110.0);
    let ch = rect.height() / 4.0;
    let r = (ch * 0.42).min(cw * 0.36).min(34.0);
    let left = rect.center().x - cw * 1.5;
    let mut out = None;
    for (i, (key, letters)) in KEYS.iter().enumerate() {
        let c = pos2(left + cw * (i % 3) as f32 + cw / 2.0, rect.top() + ch * (i / 3) as f32 + ch / 2.0);
        let kr = Rect::from_center_size(c, Vec2::splat(r * 2.0));
        let id = Id::unique((salt, i));
        let resp = ui.interact(kr, id, Sense::click());
        let t = ui.ctx().animate_bool(id.with("h"), resp.hovered());
        let down = resp.is_pointer_button_down_on();
        let fill = if down { p.hover } else { p.raised.lerp_to_gamma(p.hover, t * 0.6) };
        ui.painter().circle_filled(c, r, fill);
        let dy = if letters.is_empty() { 0.0 } else { -5.0 };
        ui.painter().text(c + vec2(0.0, dy), Align2::CENTER_CENTER, key.to_string(), FontId::proportional(r * 0.72), p.text);
        if !letters.is_empty() {
            ui.painter().text(c + vec2(0.0, r * 0.42), Align2::CENTER_CENTER, *letters, FontId::proportional(8.5), p.dim);
        }
        if resp.clicked() {
            out = Some(*key);
        }
        if resp.secondary_clicked() && *key == '0' {
            out = Some('+');
        }
    }
    out
}

/// White on dark fills, near-black on the pastel ones (Mocha, Tokyo Night).
fn ink_on(fill: Color32) -> Color32 {
    let l = 0.2126 * fill.r() as f32 + 0.7152 * fill.g() as f32 + 0.0722 * fill.b() as f32;
    if l > 170.0 { Color32::from_rgb(0x1e, 0x1e, 0x2e) } else { Color32::WHITE }
}

/// A disc shaded from `top` to `bottom`.
fn gradient_circle(painter: &egui::Painter, c: Pos2, r: f32, top: Color32, bottom: Color32) {
    let mut mesh = egui::Mesh::default();
    let n = (r * 1.2).clamp(24.0, 96.0) as u32;
    let shade = |y: f32| top.lerp_to_gamma(bottom, ((y - (c.y - r)) / (2.0 * r)).clamp(0.0, 1.0));
    mesh.colored_vertex(c, shade(c.y));
    for i in 0..n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        let pt = c + Vec2::angled(a) * r;
        mesh.colored_vertex(pt, shade(pt.y));
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    painter.add(mesh);
}

fn avatar(painter: &egui::Painter, c: Pos2, r: f32, name: &str, seed: &str, font: f32) {
    let (a, b) = theme::avatar_colors(seed);
    gradient_circle(painter, c, r, a, b);
    painter.circle_stroke(c, r, Stroke::new(1.0, Color32::from_white_alpha(24)));
    painter.text(c, Align2::CENTER_CENTER, theme::initials(name), FontId::proportional(font), Color32::WHITE);
}

/// A person or a call: avatar, name, a line under it, and something on the right.
#[allow(clippy::too_many_arguments)]
fn contact_row(ui: &mut Ui, p: &Palette, title: &str, sub: &str, right: &str, seed: &str, missed: bool, selected: bool, contact: bool) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
    if selected || resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(10), if selected { p.accent.gamma_multiply(0.22) } else { p.raised });
    }
    if selected {
        ui.scroll_to_rect(r, None);
    }
    avatar(ui.painter(), pos2(r.left() + 24.0, r.center().y), 16.0, title, seed, 12.0);
    let col = if missed { p.bad } else { p.text };
    ui.painter().text(pos2(r.left() + 50.0, r.top() + 8.0), Align2::LEFT_TOP, elide(title, 30), FontId::proportional(14.5), col);
    ui.painter().text(pos2(r.left() + 50.0, r.top() + 27.0), Align2::LEFT_TOP, elide(sub, 40), FontId::proportional(11.5), p.dim);
    if !right.is_empty() {
        ui.painter().text(pos2(r.right() - 10.0, r.center().y), Align2::RIGHT_CENTER, elide(right, 16), FontId::proportional(11.5), p.dim);
    } else if contact {
        icons::draw(ui.painter(), Icon::Phone, pos2(r.right() - 18.0, r.center().y), 14.0, if selected || resp.hovered() { p.good } else { p.dim.gamma_multiply(0.6) });
    }
    resp
}

/// Reads a .vcf for the address book: the file's name and its contacts.
fn import_file(path: &str) -> Imported {
    let p = std::path::Path::new(path);
    let list = contacts::read_vcf(p)?;
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok((name, list))
}

fn palette_row(ui: &mut Ui, p: &Palette, label: &str, keys: &str, selected: bool) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    if selected || resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(8), if selected { p.accent.gamma_multiply(0.25) } else { p.raised });
    }
    if selected {
        ui.scroll_to_rect(r, None);
    }
    ui.painter().text(pos2(r.left() + 10.0, r.center().y), Align2::LEFT_CENTER, label, FontId::proportional(14.0), p.text);
    if !keys.is_empty() {
        let g = ui.painter().layout_no_wrap(keys.to_string(), FontId::monospace(11.5), p.dim);
        let kr = Rect::from_min_size(pos2(r.right() - g.size().x - 18.0, r.center().y - g.size().y / 2.0 - 3.0), g.size() + vec2(10.0, 6.0));
        ui.painter().rect(kr, CornerRadius::same(5), p.raised, Stroke::new(1.0, p.hover), StrokeKind::Inside);
        ui.painter().galley(kr.min + vec2(5.0, 3.0), g, p.dim);
    }
    resp
}

/// Every letter of `q`, in order, somewhere in `s`.
fn fuzzy(s: &str, q: &str) -> bool {
    let mut it = s.chars();
    q.chars().filter(|c| !c.is_whitespace()).all(|qc| it.any(|c| c == qc))
}

/// `"Bob" <sip:201@pbx;transport=udp>` → `201`.
pub fn user_part(uri: &str) -> String {
    let s = strip_sip(uri);
    s.split('@').next().unwrap_or(&s).to_string()
}

fn strip_sip(uri: &str) -> String {
    let s = uri.trim();
    let s = s.rsplit_once('<').map(|(_, r)| r.trim_end_matches('>')).unwrap_or(s);
    let s = s.strip_prefix("sips:").or_else(|| s.strip_prefix("sip:")).unwrap_or(s);
    s.split(';').next().unwrap_or(s).to_string()
}

fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { s.chars().take(max - 1).collect::<String>() + "…" }
}

fn clock(secs: u64) -> String {
    if secs >= 3600 { format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60) } else { format!("{:02}:{:02}", secs / 60, secs % 60) }
}

fn duration(secs: u64) -> String {
    if secs < 60 { format!("{secs}s") } else if secs < 3600 { format!("{}m {}s", secs / 60, secs % 60) } else { format!("{}h {}m", secs / 3600, secs / 60 % 60) }
}

fn ago(at: u64) -> String {
    let d = config::now().saturating_sub(at);
    match d {
        0..60 => "now".into(),
        60..3600 => format!("{}m", d / 60),
        3600..86400 => format!("{}h", d / 3600),
        86400..172800 => "yesterday".into(),
        _ => format!("{}d", d / 86400),
    }
}

/// A desktop notification with Answer and Decline, through `notify-send` (libnotify),
/// which mako, dunst, swaync and the others all understand.
fn notify_incoming(title: &str, peer: &str, generation: u64, id: Arc<Mutex<Option<u32>>>, tx: Sender<(u64, bool)>, ctx: egui::Context) {
    let (title, body) = (title.to_string(), strip_sip(peer));
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader};
        let child = std::process::Command::new("notify-send")
            .args(["-a", "Dialo", "-u", "critical", "-i", "call-start", "-p", "-A", "answer=Answer", "-A", "decline=Decline", "-h", "string:category:call.incoming"])
            .arg(format!("Incoming call · {title}"))
            .arg(body)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn();
        let Ok(mut child) = child else { return };
        let out = child.stdout.take().unwrap();
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let line = line.trim().to_string();
            if let Ok(n) = line.parse::<u32>() {
                *id.lock().unwrap() = Some(n);
            } else if line == "answer" || line == "decline" {
                let _ = tx.send((generation, line == "answer"));
                ctx.request_repaint();
            }
        }
        let _ = child.wait();
    });
}

fn close_notification(id: &Arc<Mutex<Option<u32>>>) {
    if let Some(n) = id.lock().unwrap().take() {
        let _ = std::process::Command::new("gdbus")
            .args(["call", "--session", "--dest", "org.freedesktop.Notifications", "--object-path", "/org/freedesktop/Notifications", "--method", "org.freedesktop.Notifications.CloseNotification"])
            .arg(n.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_parts() {
        assert_eq!(user_part("\"Bob\" <sip:201@pbx.lan;transport=udp>"), "201");
        assert_eq!(user_part("sip:0912@pbx"), "0912");
        assert_eq!(strip_sip("<sips:a@b:5061;transport=tls>"), "a@b:5061");
    }

    #[test]
    fn times() {
        assert_eq!(clock(65), "01:05");
        assert_eq!(clock(3725), "1:02:05");
        assert_eq!(duration(125), "2m 5s");
    }

    #[test]
    fn fuzzy_matches_in_order() {
        assert!(fuzzy("hang up / decline", "hup"));
        assert!(!fuzzy("mute microphone", "zz"));
    }
}

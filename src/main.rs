//! Dialo: a small SIP softphone. `dialo` opens the window (or shows the one already
//! running); `dialo call 201`, `dialo answer`, `dialo hangup`, `dialo mute`, `dialo hold`
//! drive the running one, for Hyprland keybinds; `dialo tel:+98...` or `dialo sip:...`
//! calls, for links.

mod config;
mod contacts;
mod engine;
mod ffi;
mod icons;
mod ipc;
mod theme;
mod tones;
mod ui;

use eframe::egui;
use ipc::Remote;
use std::sync::{Arc, Mutex};

const USAGE: &str = "\
Usage: dialo [COMMAND]

  (none)            open Dialo, or bring the running one forward
  call NUMBER       call a number, an extension or a SIP address
  tel:… | sip:…     the same, for links
  answer            answer the ringing call
  hangup            hang up, cancel, or decline
  mute | hold       toggle the microphone, or hold
  import FILE.vcf   add contacts from a vCard file (Google Contacts > Export)
  quit              close the running Dialo
  --version";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let first = args.first().map(String::as_str).unwrap_or("");
    let remote = match first {
        "" => Remote::Show,
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            return;
        }
        "-V" | "--version" => {
            println!("dialo {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        "--engine-test" => std::process::exit(engine_test(&args[1..])),
        s if s.starts_with("tel:") || s.starts_with("sip:") || s.starts_with("sips:") || s.starts_with("callto:") => {
            Remote::Call(s.trim_start_matches("callto:").trim_start_matches("//").to_string())
        }
        "import" if args.len() == 2 => {
            // The running Dialo reads it from its own folder: make the path absolute.
            let p = std::path::absolute(&args[1]).unwrap_or_else(|_| args[1].clone().into());
            Remote::Import(p.to_string_lossy().into_owned())
        }
        _ => match Remote::parse(&args.join(" ")) {
            Some(r) => r,
            None => {
                eprintln!("dialo: unknown command '{}'\n\n{USAGE}", args.join(" "));
                std::process::exit(2);
            }
        },
    };
    // One Dialo per session: a running one takes the command.
    if ipc::send(&remote) {
        return;
    }
    if let Remote::Import(path) = &remote {
        // No window open: add them to the address book from here.
        match contacts::read_vcf(std::path::Path::new(path)) {
            Ok(list) => {
                let mut book = contacts::Book::load();
                let n = list.len();
                let added = book.add(list);
                if let Err(e) = book.save() {
                    eprintln!("dialo: could not save the contacts: {e}");
                    std::process::exit(1);
                }
                println!("{added} new of {n} contacts; {} in all", book.list.len());
                return;
            }
            Err(e) => {
                eprintln!("dialo: {e}");
                std::process::exit(1);
            }
        }
    }
    if matches!(remote, Remote::Answer | Remote::Hangup | Remote::ToggleMute | Remote::ToggleHold | Remote::Quit) {
        eprintln!("dialo: Dialo is not running");
        std::process::exit(1);
    }
    if let Err(e) = run_window(remote) {
        eprintln!("dialo: {e}");
        std::process::exit(1);
    }
}

fn run_window(first: Remote) -> Result<(), String> {
    // The window's egui context exists only once eframe starts; until then wake-ups
    // from the engine and the socket go nowhere.
    let ctx_slot: Arc<Mutex<Option<egui::Context>>> = Arc::default();
    let waker = |slot: &Arc<Mutex<Option<egui::Context>>>| -> Box<dyn Fn() + Send> {
        let slot = slot.clone();
        Box::new(move || {
            if let Some(c) = slot.lock().unwrap().as_ref() {
                c.request_repaint();
            }
        })
    };
    let (engine, events) = engine::Engine::start(engine::Options::default(), waker(&ctx_slot));
    let (rtx, rrx) = std::sync::mpsc::channel();
    if let Err(e) = ipc::listen(rtx.clone(), waker(&ctx_slot)) {
        eprintln!("dialo: no control socket ({e}); keybinds won't reach this window");
    }
    if let Remote::Call(_) = first {
        let _ = rtx.send(first);
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("dialo")
            .with_title("Dialo")
            .with_inner_size([380.0, 640.0])
            .with_min_inner_size([320.0, 480.0]),
        ..Default::default()
    };
    let result = eframe::run_native(
        "Dialo",
        options,
        Box::new(move |cc| {
            *ctx_slot.lock().unwrap() = Some(cc.egui_ctx.clone());
            Ok(Box::new(ui::App::new(cc, engine, events, rrx)))
        }),
    );
    ipc::unlisten();
    result.map_err(|e| e.to_string())
}

/// `--engine-test PBX USER TARGET|- SOURCE.wav HEARD.wav SECONDS`: the engine alone,
/// with WAV files for sound. Calls TARGET (or, with `-`, waits for a call on
/// 127.0.0.1:5072 and answers it), talks for SECONDS, hangs up. Used by tools/loopback.sh.
fn engine_test(a: &[String]) -> i32 {
    use engine::{CallState, Cmd, Event};
    use std::time::{Duration, Instant};
    let [pbx, user, target, source, heard, secs] = a else {
        eprintln!("--engine-test PBX USER TARGET|- SOURCE.wav HEARD.wav SECONDS");
        return 2;
    };
    let secs: u64 = secs.parse().unwrap_or(1);
    let opts = engine::Options {
        source: format!("aufile,{source}"),
        player: format!("aufile,{heard}"),
        alert: format!("aufile,{heard}.ring"),
        listen: std::env::var("DIALO_LISTEN").unwrap_or_else(|_| "127.0.0.1:5072".into()),
        // Loopback, which baresip leaves out unless pinned (see engine.rs).
        iface: Some(std::env::var("DIALO_NET").unwrap_or_else(|_| "127.0.0.1".into())),
    };
    let (eng, rx) = engine::Engine::start(opts, Box::new(|| {}));
    let acc = config::Account { username: user.clone(), domain: pbx.clone(), ..Default::default() };
    eng.send(Cmd::SetAccount(Some(acc)));
    if target != "-" {
        std::thread::sleep(Duration::from_millis(300));
        eng.send(Cmd::Dial(target.clone()));
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut talking: Option<Instant> = None;
    loop {
        if let Some(t) = talking {
            if t.elapsed() >= Duration::from_secs(secs) {
                eng.send(Cmd::Hangup);
                std::thread::sleep(Duration::from_millis(300));
                println!("Hung up");
                return 0;
            }
        }
        if Instant::now() > deadline {
            println!("Timed out");
            return 1;
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Event::Failed(why)) => {
                println!("Failed: {why}");
                return 1;
            }
            Ok(Event::Call { state: CallState::Incoming, peer, .. }) => {
                println!("Incoming from {peer}");
                std::thread::sleep(Duration::from_millis(500));
                eng.send(Cmd::Answer);
            }
            Ok(Event::Call { state: CallState::Talking, peer, .. }) => {
                println!("Talking with {peer}");
                talking.get_or_insert_with(Instant::now);
            }
            Ok(Event::Ended { reason, answered }) => {
                println!("Ended: {reason}");
                return if answered { 0 } else { 1 };
            }
            Ok(e) => println!("{e:?}"),
            Err(_) => {}
        }
    }
}

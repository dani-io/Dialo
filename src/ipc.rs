//! One Dialo per session. The running one listens on `$XDG_RUNTIME_DIR/dialo.sock`; a
//! second `dialo` (from a Hyprland keybind, or a `tel:` link) hands it a line and exits.
//!
//! Lines: `call <number or address>`, `answer`, `hangup`, `mute`, `hold`, `show`, `quit`.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

#[derive(Debug, Clone, PartialEq)]
pub enum Remote {
    Call(String),
    Answer,
    Hangup,
    ToggleMute,
    ToggleHold,
    Show,
    Quit,
}

impl Remote {
    pub fn parse(line: &str) -> Option<Remote> {
        let line = line.trim();
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        Some(match word {
            "call" if !rest.trim().is_empty() => Remote::Call(rest.trim().to_string()),
            "answer" => Remote::Answer,
            "hangup" => Remote::Hangup,
            "mute" => Remote::ToggleMute,
            "hold" => Remote::ToggleHold,
            "show" => Remote::Show,
            "quit" => Remote::Quit,
            _ => return None,
        })
    }

    pub fn line(&self) -> String {
        match self {
            Remote::Call(s) => format!("call {s}"),
            Remote::Answer => "answer".into(),
            Remote::Hangup => "hangup".into(),
            Remote::ToggleMute => "mute".into(),
            Remote::ToggleHold => "hold".into(),
            Remote::Show => "show".into(),
            Remote::Quit => "quit".into(),
        }
    }
}

pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join("dialo.sock")
}

/// Sends to a running Dialo; `false` when none is listening.
pub fn send(msg: &Remote) -> bool {
    match UnixStream::connect(socket_path()) {
        Ok(mut s) => writeln!(s, "{}", msg.line()).is_ok(),
        Err(_) => false,
    }
}

/// Starts listening; every line received goes to `tx`, then `wake` runs.
pub fn listen(tx: Sender<Remote>, wake: Box<dyn Fn() + Send>) -> std::io::Result<()> {
    let path = socket_path();
    // A socket left by a Dialo that crashed: nobody answers on it.
    if path.exists() && UnixStream::connect(&path).is_err() {
        let _ = std::fs::remove_file(&path);
    }
    let listener = UnixListener::bind(&path)?;
    std::thread::Builder::new().name("ipc".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            for line in BufReader::new(stream).lines().map_while(Result::ok).take(16) {
                if let Some(msg) = Remote::parse(&line) {
                    if tx.send(msg).is_err() {
                        return;
                    }
                    wake();
                }
            }
        }
    })?;
    Ok(())
}

pub fn unlisten() {
    let _ = std::fs::remove_file(socket_path());
}

#[cfg(test)]
mod tests {
    use super::Remote;

    #[test]
    fn round_trip() {
        for r in [Remote::Call("sip:1@x".into()), Remote::Answer, Remote::Hangup, Remote::ToggleMute, Remote::ToggleHold, Remote::Show, Remote::Quit] {
            assert_eq!(Remote::parse(&r.line()), Some(r));
        }
        assert_eq!(Remote::parse("call  "), None);
        assert_eq!(Remote::parse("format c:"), None);
    }
}

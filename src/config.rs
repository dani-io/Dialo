//! Settings in `~/.config/dialo/config.json` (only the owner can read it: the SIP password
//! is in it), and the call history in `~/.local/state/dialo/history.json`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    #[default]
    Udp,
    Tcp,
    Tls,
}

impl Transport {
    pub const ALL: [Transport; 3] = [Transport::Udp, Transport::Tcp, Transport::Tls];
    pub fn name(self) -> &'static str {
        match self {
            Transport::Udp => "udp",
            Transport::Tcp => "tcp",
            Transport::Tls => "tls",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Account {
    pub display_name: String,
    /// The user part of the address, usually the extension.
    pub username: String,
    /// When the PBX wants a different name for authentication.
    pub auth_user: String,
    pub password: String,
    /// The PBX or provider: `pbx.example.com` or `10.0.0.5:5060`.
    pub domain: String,
    pub transport: Transport,
    /// An outbound proxy, when calls must go through one.
    pub proxy: String,
    /// Encrypt the audio (SRTP); the PBX must offer it too.
    pub srtp: bool,
    /// A STUN server for getting through NAT: `stun.l.google.com:19302`.
    pub stun: String,
}

impl Account {
    pub fn is_complete(&self) -> bool {
        !self.username.trim().is_empty() && !self.domain.trim().is_empty()
    }

    /// The account line baresip takes, without the password (set apart, so it needs no
    /// escaping).
    pub fn aor(&self) -> String {
        let user = self.username.trim();
        let domain = self.domain.trim();
        let mut s = String::new();
        let name = self.display_name.trim().replace('"', "");
        if !name.is_empty() {
            s += &format!("\"{name}\" ");
        }
        s += &format!("<sip:{user}@{domain};transport={}>", self.transport.name());
        let auth = self.auth_user.trim();
        if !auth.is_empty() {
            s += &format!(";auth_user={auth}");
        }
        let proxy = self.proxy.trim();
        if !proxy.is_empty() {
            let proxy = proxy.strip_prefix("sip:").unwrap_or(proxy);
            s += &format!(";outbound=\"sip:{proxy};transport={}\"", self.transport.name());
        }
        s += ";regint=600;answermode=manual";
        if self.srtp {
            s += ";mediaenc=srtp";
        }
        let stun = self.stun.trim();
        if !stun.is_empty() {
            let stun = stun.strip_prefix("stun:").unwrap_or(stun);
            s += &format!(";medianat=stun;stunserver=\"stun:{stun}\"");
        }
        s
    }

    /// A SIP address for what was typed: a number or name is called at the account's
    /// domain; `sip:` addresses and `user@host` are called as they are.
    pub fn target(&self, typed: &str) -> Option<String> {
        let typed = typed.trim();
        if typed.is_empty() {
            return None;
        }
        if typed.starts_with("sip:") || typed.starts_with("sips:") {
            return Some(typed.to_string());
        }
        if let Some(rest) = typed.strip_prefix("tel:") {
            return self.target(rest);
        }
        if typed.contains('@') {
            return Some(format!("sip:{typed}"));
        }
        // Spaces, dashes, dots and brackets people write in numbers.
        let number: String = typed.chars().filter(|c| !matches!(c, ' ' | '-' | '.' | '(' | ')')).collect();
        if number.is_empty() || self.domain.trim().is_empty() {
            return None;
        }
        Some(format!("sip:{number}@{}", self.domain.trim()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeName {
    #[default]
    Midnight,
    Mocha,
    TokyoNight,
    Paper,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    pub account: Account,
    pub theme: ThemeName,
    /// Show a desktop notification for an incoming call.
    pub notify: bool,
    /// Your own Google OAuth client ("Desktop app"), when the build carries none.
    pub google_client_id: String,
    pub google_client_secret: String,
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("dialo")
}

fn state_dir() -> PathBuf {
    dirs::state_dir().or_else(dirs::data_local_dir).unwrap_or_else(|| PathBuf::from(".")).join("dialo")
}

impl Settings {
    pub fn load() -> Settings {
        match std::fs::read(config_dir().join("config.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!("dialo: config.json is not readable ({e}); starting from defaults");
                Settings { notify: true, ..Default::default() }
            }),
            Err(_) => Settings { notify: true, ..Default::default() },
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(self).expect("settings serialize");
        write_private(&config_dir().join("config.json"), &json)
    }
}

/// Writes through a temporary file, readable by the owner only.
pub fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Outgoing,
    Incoming,
    Missed,
    /// Turned away without answering.
    Declined,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// What to call back: the SIP address.
    pub uri: String,
    pub name: String,
    pub direction: Direction,
    /// Unix seconds.
    pub at: u64,
    /// Seconds talked; 0 when never answered.
    pub secs: u64,
}

pub const HISTORY_MAX: usize = 300;

pub fn load_history() -> Vec<HistoryEntry> {
    std::fs::read(state_dir().join("history.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_history(h: &[HistoryEntry]) {
    let json = serde_json::to_vec(h).expect("history serialize");
    if let Err(e) = write_private(&state_dir().join("history.json"), &json) {
        eprintln!("dialo: could not save the call history: {e}");
    }
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acc() -> Account {
        Account { username: "201".into(), domain: "pbx.lan".into(), ..Default::default() }
    }

    #[test]
    fn aor_has_what_was_set() {
        let mut a = acc();
        assert_eq!(a.aor(), "<sip:201@pbx.lan;transport=udp>;regint=600;answermode=manual");
        a.display_name = "Dan \"D\"".into();
        a.transport = Transport::Tls;
        a.proxy = "sip:edge.pbx.lan".into();
        a.srtp = true;
        a.stun = "stun.example.com:3478".into();
        assert_eq!(
            a.aor(),
            "\"Dan D\" <sip:201@pbx.lan;transport=tls>;outbound=\"sip:edge.pbx.lan;transport=tls\";regint=600;answermode=manual;mediaenc=srtp;medianat=stun;stunserver=\"stun:stun.example.com:3478\""
        );
    }

    #[test]
    fn targets() {
        let a = acc();
        assert_eq!(a.target(" 0912 345-67 89 ").as_deref(), Some("sip:09123456789@pbx.lan"));
        assert_eq!(a.target("tel:+1 (555) 010").as_deref(), Some("sip:+1555010@pbx.lan"));
        assert_eq!(a.target("bob@example.org").as_deref(), Some("sip:bob@example.org"));
        assert_eq!(a.target("sip:300@pbx.lan").as_deref(), Some("sip:300@pbx.lan"));
        assert_eq!(a.target("   "), None);
    }
}

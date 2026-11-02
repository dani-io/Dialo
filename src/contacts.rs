//! The address book: contacts imported from vCard files (Google Contacts' "Export", any
//! phone, Nextcloud...) or synced from Google (see `google.rs`), kept in
//! `~/.local/share/dialo/contacts.json`. Typing in the dialer searches it, and the
//! name of a number that calls is looked up here.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Phone {
    /// "mobile", "work", "sip"... as the source wrote it, lowercased; may be empty.
    pub label: String,
    /// As written: `+98 912 555 1234`, `201`, or a `sip:` address.
    pub number: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub name: String,
    #[serde(default)]
    pub org: String,
    pub phones: Vec<Phone>,
    /// Where it came from: "google" (replaced by every sync) or "vcard".
    #[serde(default)]
    pub source: String,
}

#[derive(Default)]
pub struct Book {
    pub list: Vec<Contact>,
}

fn path() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("dialo").join("contacts.json")
}

impl Book {
    pub fn load() -> Book {
        let list = std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Book { list }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let json = serde_json::to_vec(&self.list).map_err(std::io::Error::other)?;
        crate::config::write_private(&path(), &json)
    }

    /// Adds contacts, skipping ones already here (same name and same numbers); returns
    /// how many were new.
    pub fn add(&mut self, new: Vec<Contact>) -> usize {
        let mut added = 0;
        for c in new {
            let key = |c: &Contact| (c.name.to_lowercase(), c.phones.iter().map(|p| digits(&p.number)).collect::<Vec<_>>());
            if self.list.iter().any(|o| key(o) == key(&c)) {
                continue;
            }
            self.list.push(c);
            added += 1;
        }
        self.sort();
        added
    }

    /// Replaces everything that came from `source` with `new`.
    pub fn replace_source(&mut self, source: &str, new: Vec<Contact>) {
        self.list.retain(|c| c.source != source);
        self.list.extend(new);
        self.sort();
    }

    fn sort(&mut self) {
        self.list.sort_by_key(|c| c.name.to_lowercase());
    }

    /// The contact and phone a number or SIP address belongs to. Numbers match when
    /// one's digits end the other's and at least 7 are the same (so `+98 912 555 1234`
    /// is `09125551234`), or exactly for short extensions.
    pub fn lookup(&self, uri_or_number: &str) -> Option<(&Contact, &Phone)> {
        let user = crate::ui::user_part(uri_or_number);
        let d = digits(&user);
        for c in &self.list {
            for p in &c.phones {
                if same_number(&d, &digits(&crate::ui::user_part(&p.number))) {
                    return Some((c, p));
                }
                if !p.number.chars().any(|ch| ch.is_ascii_digit()) || p.number.contains('@') {
                    // A SIP address or a name: compare as it is.
                    if crate::ui::user_part(&p.number).eq_ignore_ascii_case(&user) && !user.is_empty() {
                        return Some((c, p));
                    }
                }
            }
        }
        None
    }

    /// Contacts and their phones that match what is typed: words in the name or
    /// company, or digits in a number. At most `max`.
    pub fn search(&self, q: &str, max: usize) -> Vec<(usize, usize)> {
        let q = q.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let qd = digits(&q);
        let numeric = !qd.is_empty() && q.chars().all(|c| c.is_ascii_digit() || " +-()".contains(c));
        let mut out = Vec::new();
        for (ci, c) in self.list.iter().enumerate() {
            let hay = format!("{} {}", c.name.to_lowercase(), c.org.to_lowercase());
            let by_name = !numeric && q.split_whitespace().all(|w| hay.split_whitespace().any(|h| h.starts_with(w)) || hay.contains(w));
            for (pi, p) in c.phones.iter().enumerate() {
                let by_number = numeric && digits(&p.number).contains(&qd);
                if by_name || by_number {
                    out.push((ci, pi));
                    if out.len() >= max {
                        return out;
                    }
                }
            }
        }
        out
    }
}

pub fn digits(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

fn same_number(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a.len() < 7 || b.len() < 7 {
        return a == b;
    }
    let n = a.len().min(b.len()).min(10);
    a[a.len() - n..] == b[b.len() - n..]
}

// ---- vCard ------------------------------------------------------------------------

/// Reads a .vcf file (one or many cards, vCard 2.1, 3.0 or 4.0).
pub fn read_vcf(path: &Path) -> Result<Vec<Contact>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let cards = parse_vcards(&text);
    if cards.is_empty() {
        return Err(format!("{} has no contacts with a phone number in it", path.display()));
    }
    Ok(cards)
}

pub fn parse_vcards(text: &str) -> Vec<Contact> {
    // Unfold: a line starting with a space or tab continues the one before (and
    // quoted-printable soft breaks end in `=`).
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = raw.strip_prefix(' ').or_else(|| raw.strip_prefix('\t')) {
            if let Some(last) = lines.last_mut() {
                last.push_str(rest);
                continue;
            }
        }
        if let Some(last) = lines.last_mut() {
            if last.ends_with('=') && last.to_ascii_uppercase().contains("QUOTED-PRINTABLE") {
                last.pop();
                last.push_str(raw);
                continue;
            }
        }
        lines.push(raw.to_string());
    }

    let mut out = Vec::new();
    let mut cur: Option<(String, String, String, Vec<Phone>)> = None; // fn, n, org, phones
    // Google writes `item1.TEL:...` and `item1.X-ABLabel:Work`: labels by group.
    let mut group_labels: Vec<(String, String)> = Vec::new();
    for line in lines {
        let Some((head, value)) = line.split_once(':') else { continue };
        let mut parts = head.split(';');
        let full = parts.next().unwrap_or("");
        let (group, prop) = match full.rsplit_once('.') {
            Some((g, p)) => (g.to_string(), p.to_ascii_uppercase()),
            None => (String::new(), full.to_ascii_uppercase()),
        };
        let params: Vec<String> = parts.map(|s| s.to_string()).collect();
        let qp = params.iter().any(|p| p.eq_ignore_ascii_case("ENCODING=QUOTED-PRINTABLE"));
        let value = if qp { quoted_printable(value) } else { unescape(value) };
        match prop.as_str() {
            "BEGIN" if value.eq_ignore_ascii_case("VCARD") => {
                cur = Some(Default::default());
                group_labels.clear();
            }
            "END" if value.eq_ignore_ascii_case("VCARD") => {
                if let Some((fname, n, org, mut phones)) = cur.take() {
                    for p in &mut phones {
                        if let Some(stripped) = p.label.strip_prefix('\u{1}') {
                            let g = stripped.to_string();
                            p.label = group_labels.iter().find(|(gg, _)| *gg == g).map(|(_, l)| l.to_lowercase()).unwrap_or_default();
                        }
                    }
                    let name = if !fname.trim().is_empty() { fname.trim().to_string() } else if !n.trim().is_empty() { n } else { org.clone() };
                    if !phones.is_empty() {
                        let name = if name.is_empty() { phones[0].number.clone() } else { name };
                        out.push(Contact { name, org, phones, source: "vcard".into() });
                    }
                }
            }
            _ => {
                let Some((fname, n, org, phones)) = cur.as_mut() else { continue };
                match prop.as_str() {
                    "FN" => *fname = value,
                    // N: family;given;middle;prefix;suffix
                    "N" => {
                        let f: Vec<&str> = value.split(';').collect();
                        let given = f.get(1).copied().unwrap_or("");
                        let family = f.first().copied().unwrap_or("");
                        *n = format!("{given} {family}").trim().to_string();
                    }
                    "ORG" => *org = value.split(';').next().unwrap_or("").trim().to_string(),
                    "TEL" | "IMPP" | "X-SIP" | "X-SIPADDRESS" => {
                        let number = value.trim().trim_start_matches("tel:").to_string();
                        if number.is_empty() {
                            continue;
                        }
                        if prop == "IMPP" && !number.starts_with("sip:") {
                            continue;
                        }
                        let mut label = tel_label(&params);
                        if prop != "TEL" && label.is_empty() {
                            label = "sip".into();
                        }
                        if label.is_empty() && !group.is_empty() {
                            // Filled in from X-ABLabel at the end of the card.
                            label = format!("\u{1}{group}");
                        }
                        if !phones.iter().any(|p| p.number == number) {
                            phones.push(Phone { label, number });
                        }
                    }
                    "X-ABLABEL" if !group.is_empty() => {
                        let l = value.trim_start_matches("_$!<").trim_end_matches(">!$_").to_string();
                        group_labels.push((group, l));
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

/// The first useful TYPE: `TYPE=CELL,VOICE`, `TYPE=work`, or 2.1's bare `CELL`.
fn tel_label(params: &[String]) -> String {
    for p in params {
        let v = match p.split_once('=') {
            Some((k, v)) if k.eq_ignore_ascii_case("TYPE") => v,
            Some(_) => continue,
            None => p.as_str(),
        };
        for t in v.trim_matches('"').split(',') {
            let t = t.to_lowercase();
            let label = match t.as_str() {
                "cell" | "mobile" => "mobile",
                "home" => "home",
                "work" => "work",
                "main" => "main",
                "fax" => "fax",
                "pager" => "pager",
                "voice" | "pref" | "internet" | "x-internet" => continue,
                other if !other.is_empty() => return other.trim_start_matches("x-").to_string(),
                _ => continue,
            };
            return label.to_string();
        }
    }
    String::new()
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push(' '),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn quoted_printable(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'=' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOGLE: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Sadegh Ahmadi\r\nN:Ahmadi;Sadegh;;;\r\nTEL;TYPE=CELL:+98 912 555 1234\r\nitem1.TEL:021-8888 0000\r\nitem1.X-ABLabel:Office\r\nORG:Go4u\r\nEND:VCARD\r\nBEGIN:VCARD\r\nVERSION:3.0\r\nFN:No Phone\r\nEMAIL:a@b.c\r\nEND:VCARD\r\nBEGIN:VCARD\r\nVERSION:2.1\r\nN;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:=D8=B3=D8=A7=D8=B1=D8=A7;;;;\r\nTEL;CELL:09121112222\r\nEND:VCARD\r\nBEGIN:VCARD\r\nVERSION:4.0\r\nFN:Reception\r\nTEL;VALUE=uri;TYPE=work:tel:300\r\nIMPP:sip:300@pbx.lan\r\nEND:VCARD\r\n";

    #[test]
    fn reads_google_and_phone_exports() {
        let c = parse_vcards(GOOGLE);
        assert_eq!(c.len(), 3, "{c:#?}");
        assert_eq!(c[0].name, "Sadegh Ahmadi");
        assert_eq!(c[0].org, "Go4u");
        assert_eq!(c[0].phones[0], Phone { label: "mobile".into(), number: "+98 912 555 1234".into() });
        assert_eq!(c[0].phones[1], Phone { label: "office".into(), number: "021-8888 0000".into() });
        // Quoted-printable UTF-8 in a vCard 2.1 from an old phone; only N given.
        assert_eq!(c[1].name, "سارا");
        assert_eq!(c[1].phones[0].label, "mobile");
        assert_eq!(c[2].phones, vec![Phone { label: "work".into(), number: "300".into() }, Phone { label: "sip".into(), number: "sip:300@pbx.lan".into() }]);
    }

    #[test]
    fn folded_lines() {
        let c = parse_vcards("BEGIN:VCARD\nFN:Very Long\n  Name\nTEL:1234567\nEND:VCARD\n");
        assert_eq!(c[0].name, "Very Long Name");
    }

    #[test]
    fn lookup_and_search() {
        let mut b = Book::default();
        b.add(parse_vcards(GOOGLE));
        assert_eq!(b.lookup("sip:09125551234@pbx").unwrap().0.name, "Sadegh Ahmadi");
        assert_eq!(b.lookup("\"x\" <sip:+989125551234@pbx>").unwrap().0.name, "Sadegh Ahmadi");
        assert_eq!(b.lookup("sip:300@pbx.lan").unwrap().0.name, "Reception");
        assert!(b.lookup("sip:301@pbx.lan").is_none());
        assert!(b.lookup("sip:30@pbx.lan").is_none());
        let names = |r: Vec<(usize, usize)>| r.into_iter().map(|(c, _)| b.list[c].name.clone()).collect::<Vec<_>>();
        assert_eq!(names(b.search("sad", 10)), vec!["Sadegh Ahmadi", "Sadegh Ahmadi"]);
        assert_eq!(names(b.search("go4", 10)).len(), 2);
        assert_eq!(names(b.search("5551", 10)), vec!["Sadegh Ahmadi"]);
        // Importing the same file again adds nothing.
        assert_eq!(b.add(parse_vcards(GOOGLE)), 0);
    }
}

//! Google Contacts, read straight from Google: sign in once in the browser (OAuth 2.0 for
//! installed apps: a loopback redirect to 127.0.0.1 and PKCE), then every sync asks the
//! People API for the phone numbers of everyone in the account. The refresh token is
//! kept in `~/.config/dialo/google.json`, readable by the owner only; signing out deletes
//! it. Only `contacts.readonly` is asked for: Dialo never changes the contacts.
//!
//! Google wants an OAuth client of the "Desktop app" kind. A build can carry one
//! (`DIALO_GOOGLE_CLIENT_ID` / `DIALO_GOOGLE_CLIENT_SECRET` at compile time), or the user
//! makes their own in Google Cloud Console and enters it in Settings. A desktop client's
//! "secret" is not secret (Google says so); PKCE is what protects the sign-in.

use crate::contacts::{Contact, Phone};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";
const PEOPLE_URL: &str = "https://people.googleapis.com/v1/people/me/connections";
const SCOPE: &str = "https://www.googleapis.com/auth/contacts.readonly";
/// How long the browser has to come back with the sign-in.
const SIGN_IN_WAIT: Duration = Duration::from_secs(300);

/// The OAuth client: the build's, unless Settings has one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Client {
    pub id: String,
    pub secret: String,
}

impl Client {
    pub fn resolve(id: &str, secret: &str) -> Option<Client> {
        if !id.trim().is_empty() {
            return Some(Client { id: id.trim().into(), secret: secret.trim().into() });
        }
        let id = option_env!("DIALO_GOOGLE_CLIENT_ID")?;
        Some(Client { id: id.into(), secret: option_env!("DIALO_GOOGLE_CLIENT_SECRET").unwrap_or("").into() })
    }
}

#[derive(Serialize, Deserialize, Default)]
struct Saved {
    refresh_token: String,
    /// The client the token belongs to: another client's token is no use.
    client_id: String,
}

fn token_path() -> PathBuf {
    crate::config::config_dir().join("google.json")
}

/// Whether a sign-in is kept for this client.
pub fn signed_in(client: &Client) -> bool {
    load().is_some_and(|s| s.client_id == client.id)
}

fn load() -> Option<Saved> {
    serde_json::from_slice(&std::fs::read(token_path()).ok()?).ok()
}

/// Forgets the sign-in, and asks Google to drop the token too.
pub fn sign_out() {
    if let Some(s) = load() {
        let _ = agent().post(REVOKE_URL).send_form(&[("token", &s.refresh_token)]);
    }
    let _ = std::fs::remove_file(token_path());
}

fn agent() -> ureq::Agent {
    let tls = native_tls::TlsConnector::new().map(std::sync::Arc::new);
    let mut b = ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).user_agent(concat!("Dialo/", env!("CARGO_PKG_VERSION")));
    if let Ok(tls) = tls {
        b = b.tls_connector(tls);
    }
    b.build()
}

/// Signs in when needed (opening the browser, and waiting for it), then reads every
/// contact with a phone number. `progress` gets a line to show while it works.
pub fn sync(client: &Client, progress: &dyn Fn(&str)) -> Result<Vec<Contact>, String> {
    let agent = agent();
    let refresh = match load() {
        Some(s) if s.client_id == client.id && !s.refresh_token.is_empty() => s.refresh_token,
        _ => {
            progress("Sign in to Google in your browser…");
            let t = sign_in(&agent, client)?;
            let saved = Saved { refresh_token: t.clone(), client_id: client.id.clone() };
            crate::config::write_private(&token_path(), &serde_json::to_vec(&saved).unwrap()).map_err(|e| format!("Could not keep the sign-in: {e}"))?;
            t
        }
    };
    progress("Reading your Google contacts…");
    let access = match access_token(&agent, client, &refresh) {
        Ok(a) => a,
        Err(TokenError::Revoked) => {
            // Signed out elsewhere, or the token expired (7 days for apps in testing).
            let _ = std::fs::remove_file(token_path());
            return Err("Google asks to sign in again: sync once more".into());
        }
        Err(TokenError::Other(e)) => return Err(e),
    };
    let mut out = Vec::new();
    let mut page = String::new();
    loop {
        let mut req = agent
            .get(PEOPLE_URL)
            .set("Authorization", &format!("Bearer {access}"))
            .query("personFields", "names,phoneNumbers,organizations")
            .query("pageSize", "1000")
            .query("sortOrder", "FIRST_NAME_ASCENDING");
        if !page.is_empty() {
            req = req.query("pageToken", &page);
        }
        let body = req.call().map_err(http_error)?.into_string().map_err(|e| e.to_string())?;
        let (list, next) = parse_connections(&body)?;
        out.extend(list);
        match next {
            Some(n) => page = n,
            None => break,
        }
    }
    Ok(out)
}

enum TokenError {
    Revoked,
    Other(String),
}

fn access_token(agent: &ureq::Agent, client: &Client, refresh: &str) -> Result<String, TokenError> {
    let mut form = vec![("client_id", client.id.as_str()), ("refresh_token", refresh), ("grant_type", "refresh_token")];
    if !client.secret.is_empty() {
        form.push(("client_secret", &client.secret));
    }
    match agent.post(TOKEN_URL).send_form(&form) {
        Ok(r) => {
            let v = json(r).map_err(TokenError::Other)?;
            v["access_token"].as_str().map(str::to_string).ok_or_else(|| TokenError::Other("Google sent no access token".into()))
        }
        Err(ureq::Error::Status(code @ (400 | 401), r)) => {
            let body = r.into_string().unwrap_or_default();
            if body.contains("invalid_grant") {
                Err(TokenError::Revoked)
            } else {
                Err(TokenError::Other(format!("Google answered {code}: {}", body.chars().take(200).collect::<String>())))
            }
        }
        Err(e) => Err(TokenError::Other(http_error(e))),
    }
}

/// Opens Google's consent page and waits on 127.0.0.1 for the browser to come back;
/// returns the refresh token.
fn sign_in(agent: &ureq::Agent, client: &Client) -> Result<String, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("Could not listen for the sign-in: {e}"))?;
    let redirect = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let verifier = b64url(&random(48));
    let challenge = b64url(&openssl::sha::sha256(verifier.as_bytes()));
    let state = b64url(&random(16));
    let url = format!(
        "{AUTH_URL}?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={challenge}&code_challenge_method=S256&state={state}&access_type=offline&prompt=consent",
        enc(&client.id),
        enc(&redirect),
        enc(SCOPE)
    );
    if std::process::Command::new("xdg-open").arg(&url).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().is_err() {
        eprintln!("dialo: open this to sign in to Google:\n{url}");
    }
    let code = wait_for_code(&listener, &state, Instant::now() + SIGN_IN_WAIT)?;
    let mut form = vec![
        ("client_id", client.id.as_str()),
        ("code", code.as_str()),
        ("code_verifier", verifier.as_str()),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect.as_str()),
    ];
    if !client.secret.is_empty() {
        form.push(("client_secret", &client.secret));
    }
    let v = json(agent.post(TOKEN_URL).send_form(&form).map_err(http_error)?)?;
    v["refresh_token"].as_str().map(str::to_string).ok_or_else(|| "Google sent no refresh token".into())
}

/// Serves the redirect: the first request with our `state` brings the code (or the
/// reason there is none). Anything else gets a 404 and is ignored.
fn wait_for_code(listener: &TcpListener, state: &str, until: Instant) -> Result<String, String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    loop {
        if Instant::now() > until {
            return Err("The Google sign-in took too long".into());
        }
        let stream = match listener.accept() {
            Ok((s, _)) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(e) => return Err(e.to_string()),
        };
        if let Some(r) = answer_redirect(stream, state) {
            return r;
        }
    }
}

fn answer_redirect(mut stream: TcpStream, state: &str) -> Option<Result<String, String>> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).ok()?;
    let result = parse_redirect(&line, state);
    let (status, msg) = match &result {
        Some(Ok(_)) => ("200 OK", "Dialo is signed in to Google. You can close this tab."),
        Some(Err(_)) => ("200 OK", "Dialo was not signed in. You can close this tab."),
        None => ("404 Not Found", "Not found"),
    };
    let page = format!(
        "<!doctype html><meta charset=utf-8><title>Dialo</title><body style=\"font:16px system-ui;background:#16171d;color:#e6e6ef;display:grid;place-items:center;height:90vh\"><p>{msg}</p>"
    );
    let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}", page.len());
    result
}

/// `GET /?state=..&code=.. HTTP/1.1` → the code; `None` for requests not ours.
fn parse_redirect(request_line: &str, state: &str) -> Option<Result<String, String>> {
    let target = request_line.strip_prefix("GET ")?.split(' ').next()?;
    let query = target.split_once('?')?.1;
    let mut code = None;
    let mut error = None;
    let mut ours = false;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = dec(v);
        match k {
            "state" => ours = v == state,
            "code" => code = Some(v),
            "error" => error = Some(v),
            _ => {}
        }
    }
    if !ours {
        return None;
    }
    Some(match (code, error) {
        (Some(c), _) if !c.is_empty() => Ok(c),
        (_, Some(e)) if e == "access_denied" => Err("Google sign-in was cancelled".into()),
        (_, Some(e)) => Err(format!("Google sign-in failed: {e}")),
        _ => Err("Google sent no sign-in code".into()),
    })
}

/// One page of `people.connections.list`: the people with a phone, and the next page.
fn parse_connections(body: &str) -> Result<(Vec<Contact>, Option<String>), String> {
    #[derive(Deserialize, Default)]
    #[serde(default, rename_all = "camelCase")]
    struct Page {
        connections: Vec<Person>,
        next_page_token: Option<String>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default, rename_all = "camelCase")]
    struct Person {
        names: Vec<Name>,
        phone_numbers: Vec<Number>,
        organizations: Vec<Org>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default, rename_all = "camelCase")]
    struct Name {
        display_name: String,
    }
    #[derive(Deserialize, Default)]
    #[serde(default, rename_all = "camelCase")]
    struct Number {
        value: String,
        canonical_form: String,
        formatted_type: String,
        #[serde(rename = "type")]
        kind: String,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Org {
        name: String,
    }
    let page: Page = serde_json::from_str(body).map_err(|e| format!("Google's answer is not readable: {e}"))?;
    let mut out = Vec::new();
    for p in page.connections {
        let mut phones: Vec<Phone> = Vec::new();
        for n in p.phone_numbers {
            let number = if n.value.trim().is_empty() { n.canonical_form } else { n.value.trim().to_string() };
            if number.is_empty() || phones.iter().any(|x| x.number == number) {
                continue;
            }
            let label = if n.kind.is_empty() { n.formatted_type.to_lowercase() } else { n.kind.to_lowercase() };
            phones.push(Phone { label, number });
        }
        if phones.is_empty() {
            continue;
        }
        let org = p.organizations.into_iter().map(|o| o.name).find(|n| !n.is_empty()).unwrap_or_default();
        let name = p.names.into_iter().map(|n| n.display_name).find(|n| !n.trim().is_empty()).unwrap_or_else(|| if org.is_empty() { phones[0].number.clone() } else { org.clone() });
        out.push(Contact { name, org, phones, source: "google".into() });
    }
    Ok((out, page.next_page_token.filter(|t| !t.is_empty())))
}

fn json(r: ureq::Response) -> Result<serde_json::Value, String> {
    let body = r.into_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&body).map_err(|e| format!("Google's answer is not readable: {e}"))
}

fn http_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, r) => {
            let body = r.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().or_else(|| v["error_description"].as_str()).or_else(|| v["error"].as_str()).map(str::to_string))
                .unwrap_or_default();
            if msg.is_empty() { format!("Google answered {code}") } else { format!("Google: {msg}") }
        }
        ureq::Error::Transport(t) => format!("Could not reach Google: {t}"),
    }
}

fn random(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    openssl::rand::rand_bytes(&mut b).expect("random bytes");
    b
}

fn b64url(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut s = String::with_capacity(bytes.len() * 4 / 3 + 3);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..=c.len() {
            s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    s
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn dec(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()).ok_or(()) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_and_escapes() {
        assert_eq!(b64url(b"foobar"), "Zm9vYmFy");
        assert_eq!(b64url(b"fo"), "Zm8");
        // RFC 7636's example verifier and challenge.
        let challenge = b64url(&openssl::sha::sha256(b"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"));
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert_eq!(enc("http://127.0.0.1:5"), "http%3A%2F%2F127.0.0.1%3A5");
        assert_eq!(dec("4%2F0Ab%2Bc+d"), "4/0Ab+c d");
        assert_eq!(dec("50%"), "50%");
    }

    #[test]
    fn redirect() {
        assert_eq!(parse_redirect("GET /?state=s1&code=4%2Fxyz&scope=a HTTP/1.1", "s1"), Some(Ok("4/xyz".into())));
        assert_eq!(parse_redirect("GET /?state=s1&error=access_denied HTTP/1.1", "s1"), Some(Err("Google sign-in was cancelled".into())));
        assert_eq!(parse_redirect("GET /?state=other&code=x HTTP/1.1", "s1"), None);
        assert_eq!(parse_redirect("GET /favicon.ico HTTP/1.1", "s1"), None);
    }

    #[test]
    fn redirect_over_a_socket() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            // A stray request first, then the real one.
            for path in ["/favicon.ico", "/?code=abc&state=st"] {
                let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
                write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
                let mut r = String::new();
                let _ = std::io::Read::read_to_string(&mut s, &mut r);
                assert!(r.starts_with(if path.contains("code") { "HTTP/1.1 200" } else { "HTTP/1.1 404" }), "{r}");
            }
        });
        assert_eq!(wait_for_code(&l, "st", Instant::now() + Duration::from_secs(5)), Ok("abc".into()));
        t.join().unwrap();
    }

    #[test]
    fn people_page() {
        let body = r#"{"connections":[
            {"resourceName":"people/1","names":[{"displayName":"Sadegh Ahmadi"}],"phoneNumbers":[{"value":"0912 555 1234","canonicalForm":"+989125551234","type":"mobile","formattedType":"Mobile"},{"value":"0912 555 1234","type":"work"}],"organizations":[{"name":"Go4u"}]},
            {"resourceName":"people/2","names":[{"displayName":"Email Only"}]},
            {"resourceName":"people/3","phoneNumbers":[{"canonicalForm":"+15551230000","formattedType":"Work"}],"organizations":[{"name":"Acme"}]}
        ],"nextPageToken":"p2","totalPeople":3}"#;
        let (list, next) = parse_connections(body).unwrap();
        assert_eq!(next.as_deref(), Some("p2"));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "Sadegh Ahmadi");
        assert_eq!(list[0].phones, vec![Phone { label: "mobile".into(), number: "0912 555 1234".into() }]);
        assert_eq!(list[0].source, "google");
        assert_eq!((list[1].name.as_str(), list[1].phones[0].number.as_str(), list[1].phones[0].label.as_str()), ("Acme", "+15551230000", "work"));
        assert_eq!(parse_connections("{}").unwrap().0.len(), 0);
    }
}

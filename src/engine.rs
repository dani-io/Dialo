//! The SIP side: baresip running on a thread of its own. The window sends it `Cmd`s
//! through a libre mqueue (the one thread-safe way into libre's loop) and hears back
//! `Event`s over a channel, with a wake-up so the window repaints.
//!
//! Dialo handles one call at a time (baresip turns a second one away as busy); the call
//! is looked up by its id each time, never kept as a pointer, so a call baresip has
//! already freed is never touched.

use crate::config::Account;
use crate::ffi::{self, ev};
use std::ffi::{CString, c_int, c_void};
use std::ptr::null_mut;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

#[derive(Debug)]
pub enum Cmd {
    /// Replace the account (and register it); `None` drops it.
    SetAccount(Option<Account>),
    Dial(String),
    Answer,
    /// Hang up, cancel, or turn away a ringing call.
    Hangup,
    Mute(bool),
    Hold(bool),
    Dtmf(char),
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reg {
    Off,
    Registering,
    Ok,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallState {
    Dialing,
    Ringing,
    Incoming,
    Talking,
}

#[derive(Debug, Clone)]
pub enum Event {
    /// Why the engine could not start; nothing more will come.
    Failed(String),
    Registration(Reg, String),
    /// A call came, or one is being made: its id, the peer's address and name.
    Call { state: CallState, peer: String, name: String },
    /// The call ended, with the reason ("Busy Here", "Connection reset by peer"...).
    Ended { reason: String, answered: bool },
    /// The other side put us on hold, or took us off.
    RemoteHold(bool),
    /// Something went wrong that the user should see, without ending anything.
    Notice(String),
}

/// Where the sound goes and comes from, and where SIP listens. The defaults are for a
/// desktop; tests point them at files.
#[derive(Clone, Debug)]
pub struct Options {
    pub source: String,
    pub player: String,
    pub alert: String,
    pub listen: String,
    /// The network interface to use, when not the default route's.
    pub iface: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        let pw = std::env::var_os("XDG_RUNTIME_DIR")
            .map(|d| std::path::Path::new(&d).join("pipewire-0").exists())
            .unwrap_or(false);
        let dev = if pw { "pipewire," } else { "alsa,default" };
        // DIALO_AUDIO picks the device ("alsa,default", "pipewire,<node>"), and
        // DIALO_AUDIO_PLAY a different one for what you hear.
        let dev = std::env::var("DIALO_AUDIO").unwrap_or_else(|_| dev.to_string());
        let play = std::env::var("DIALO_AUDIO_PLAY").unwrap_or_else(|_| dev.clone());
        Options { source: dev, player: play.clone(), alert: play, listen: std::env::var("DIALO_LISTEN").unwrap_or_else(|_| "0.0.0.0:0".into()), iface: std::env::var("DIALO_NET").ok() }
    }
}

pub struct Engine {
    mq: SendPtr,
    thread: Option<JoinHandle<()>>,
}

#[derive(Clone, Copy)]
struct SendPtr(*mut ffi::mqueue);
unsafe impl Send for SendPtr {}

impl Engine {
    /// Starts baresip. `wake` is called after every event is sent.
    pub fn start(opts: Options, wake: Box<dyn Fn() + Send>) -> (Engine, Receiver<Event>) {
        let (tx, rx) = channel();
        let (mq_tx, mq_rx) = channel::<Result<SendPtr, String>>();
        let thread = std::thread::Builder::new()
            .name("baresip".into())
            .spawn(move || run(opts, tx, wake, mq_tx))
            .expect("spawn the baresip thread");
        match mq_rx.recv() {
            Ok(Ok(mq)) => (Engine { mq, thread: Some(thread) }, rx),
            Ok(Err(_)) | Err(_) => {
                let _ = thread.join();
                (Engine { mq: SendPtr(null_mut()), thread: None }, rx)
            }
        }
    }

    pub fn send(&self, cmd: Cmd) {
        if self.mq.0.is_null() {
            return;
        }
        let data = Box::into_raw(Box::new(cmd)) as *mut c_void;
        // A failed push (out of memory) leaks one small box; nothing else to do with it.
        unsafe { ffi::mqueue_push(self.mq.0, 0, data) };
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Some(t) = self.thread.take() {
            self.send(Cmd::Quit);
            let _ = t.join();
        }
    }
}

/// What the engine thread keeps; the event and queue handlers get it as their `arg`.
struct Ctx {
    tx: Sender<Event>,
    wake: Box<dyn Fn() + Send>,
    opts: Options,
    ua: *mut ffi::ua,
    account: Option<Account>,
    /// The id of the call in progress.
    call: Option<String>,
    answered: bool,
    tone: *mut ffi::play,
    quitting: bool,
    /// The call is ending because of us, whatever reason baresip gives.
    local_hangup: bool,
}

impl Ctx {
    fn emit(&self, e: Event) {
        let _ = self.tx.send(e);
        (self.wake)();
    }

    fn the_call(&self) -> *mut ffi::call {
        match &self.call {
            Some(id) => {
                let id = CString::new(id.as_str()).unwrap_or_default();
                unsafe { ffi::uag_call_find(id.as_ptr()) }
            }
            None => null_mut(),
        }
    }

    fn stop_tone(&mut self) {
        if !self.tone.is_null() {
            unsafe { ffi::mem_deref(self.tone as *mut c_void) };
            self.tone = null_mut();
        }
    }

    /// Plays PCM over and over until `stop_tone`, on the alert device or the call's.
    fn play(&mut self, pcm: &[i16], alert: bool) {
        self.stop_tone();
        let (m, d) = split_dev(if alert { &self.opts.alert } else { &self.opts.player });
        unsafe {
            let mb = ffi::mbuf_alloc(pcm.len() * 2);
            if mb.is_null() {
                return;
            }
            let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
            ffi::mbuf_write_mem(mb, bytes.as_ptr(), bytes.len());
            let (m, d) = (CString::new(m).unwrap_or_default(), CString::new(d).unwrap_or_default());
            let err = ffi::play_tone(&mut self.tone, ffi::baresip_player(), mb, crate::tones::RATE, 1, -1, m.as_ptr(), d.as_ptr());
            ffi::mem_deref(mb as *mut c_void);
            if err != 0 {
                self.tone = null_mut();
            }
        }
    }

    fn set_account(&mut self, acc: Option<Account>) {
        if !self.ua.is_null() {
            unsafe {
                ffi::ua_unregister(self.ua);
                ffi::ua_destroy(self.ua);
            }
            self.ua = null_mut();
        }
        self.account = None;
        let Some(acc) = acc.filter(|a| a.is_complete()) else {
            self.emit(Event::Registration(Reg::Off, String::new()));
            return;
        };
        let Ok(aor) = CString::new(acc.aor()) else {
            self.emit(Event::Registration(Reg::Failed, "The account has a character SIP can't carry".into()));
            return;
        };
        let mut ua = null_mut();
        let err = unsafe { ffi::ua_alloc(&mut ua, aor.as_ptr()) };
        if err != 0 || ua.is_null() {
            self.emit(Event::Registration(Reg::Failed, format!("The account was not accepted ({})", errno(err))));
            return;
        }
        if !acc.password.is_empty() {
            let pass = CString::new(acc.password.as_str()).unwrap_or_default();
            unsafe { ffi::account_set_auth_pass(ffi::ua_account(ua), pass.as_ptr()) };
        }
        self.ua = ua;
        self.account = Some(acc);
        self.emit(Event::Registration(Reg::Registering, String::new()));
        let err = unsafe { ffi::ua_register(ua) };
        if err != 0 {
            self.emit(Event::Registration(Reg::Failed, format!("Could not register ({})", errno(err))));
        }
    }

    fn command(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::SetAccount(acc) => self.set_account(acc),
            Cmd::Dial(typed) => {
                if self.call.is_some() {
                    return;
                }
                let Some(acc) = &self.account else {
                    self.emit(Event::Notice("Set up an account first (Ctrl+,)".into()));
                    return;
                };
                let Some(uri) = acc.target(&typed) else { return };
                let curi = CString::new(uri.as_str()).unwrap_or_default();
                let mut call = null_mut();
                let err = unsafe { ffi::ua_connect(self.ua, &mut call, std::ptr::null(), curi.as_ptr(), ffi::VIDMODE_OFF) };
                if err != 0 || call.is_null() {
                    self.emit(Event::Notice(format!("Could not call {uri} ({})", errno(err))));
                    return;
                }
                self.call = Some(ffi::string(unsafe { ffi::call_id(call) }));
                self.answered = false;
                self.emit(Event::Call { state: CallState::Dialing, peer: uri, name: String::new() });
            }
            Cmd::Answer => {
                let call = self.the_call();
                if !call.is_null() {
                    self.stop_tone();
                    unsafe { ffi::ua_answer(self.ua, call, ffi::VIDMODE_OFF) };
                }
            }
            Cmd::Hangup => {
                let call = self.the_call();
                self.stop_tone();
                if !call.is_null() {
                    // A ringing call that is turned away hears "busy"; others just end.
                    self.local_hangup = true;
                    let (code, reason) = if self.answered || self.is_outgoing() { (0, null()) } else { (486, c"Busy Here".as_ptr()) };
                    unsafe { ffi::ua_hangup(self.ua, call, code, reason) };
                    // baresip sends no CLOSED event for a call hung up from here.
                    self.ended("Hung up".into());
                }
            }
            Cmd::Mute(on) => {
                let call = self.the_call();
                if !call.is_null() {
                    unsafe { ffi::audio_mute(ffi::call_audio(call), on) };
                }
            }
            Cmd::Hold(on) => {
                let call = self.the_call();
                if !call.is_null() {
                    let err = unsafe { ffi::call_hold(call, on) };
                    if err != 0 {
                        self.emit(Event::Notice(format!("Hold failed ({})", errno(err))));
                    }
                }
            }
            Cmd::Dtmf(key) => {
                let call = self.the_call();
                if !call.is_null() && key.is_ascii() {
                    unsafe {
                        ffi::call_send_digit(call, key as std::ffi::c_char);
                        ffi::call_send_digit(call, ffi::KEYCODE_REL);
                    }
                }
            }
            Cmd::Quit => {
                self.quitting = true;
                self.stop_tone();
                self.ua = null_mut();
                unsafe { ffi::ua_stop_all(false) };
            }
        }
    }

    fn is_outgoing(&self) -> bool {
        let call = self.the_call();
        !call.is_null() && unsafe { ffi::call_is_outgoing(call) }
    }

    fn ended(&mut self, reason: String) {
        let reason = if std::mem::take(&mut self.local_hangup) { "Hung up".to_string() } else { reason };
        if self.call.take().is_some() {
            self.stop_tone();
            let answered = std::mem::take(&mut self.answered);
            self.emit(Event::Ended { reason, answered });
        }
    }

    fn event(&mut self, e: u32, event: *mut ffi::bevent) {
        let text = ffi::string(unsafe { ffi::bevent_get_text(event) });
        let call = unsafe { ffi::bevent_get_call(event) };
        let ua = unsafe { ffi::bevent_get_ua(event) };
        if !ua.is_null() && ua != self.ua && call.is_null() {
            return;
        }
        let ours = |ctx: &Ctx| !call.is_null() && ctx.call.as_deref() == Some(ffi::string(unsafe { ffi::call_id(call) }).as_str());
        match e {
            ev::REGISTERING => {}
            ev::REGISTER_OK => self.emit(Event::Registration(Reg::Ok, text)),
            ev::REGISTER_FAIL => self.emit(Event::Registration(Reg::Failed, text)),
            ev::UNREGISTERING => {}
            // A new INVITE: take it, unless a call is already on (then the caller hears busy).
            ev::SIPSESS_CONN => {
                let msg = unsafe { ffi::bevent_get_msg(event) };
                if msg.is_null() {
                    return;
                }
                unsafe { ffi::bevent_stop(event) };
                let ua = unsafe { ffi::uag_find_msg(msg) };
                if self.call.is_some() || ua.is_null() || ua != self.ua {
                    unsafe { ffi::sip_treply(null_mut(), ffi::uag_sip(), msg, 486, c"Busy Here".as_ptr()) };
                    return;
                }
                let err = unsafe { ffi::ua_accept(ua, msg) };
                if err != 0 {
                    self.emit(Event::Notice(format!("Could not take an incoming call ({})", errno(err))));
                }
            }
            ev::CALL_INCOMING => {
                if self.call.is_some() {
                    return;
                }
                self.call = Some(ffi::string(unsafe { ffi::call_id(call) }));
                self.answered = false;
                let peer = ffi::string(unsafe { ffi::call_peeruri(call) });
                let name = ffi::string(unsafe { ffi::call_peername(call) });
                self.play(&crate::tones::ringtone(), true);
                self.emit(Event::Call { state: CallState::Incoming, peer, name });
            }
            ev::CALL_RINGING if ours(self) => {
                self.play(&crate::tones::ringback(), false);
                self.emit_state(call, CallState::Ringing);
            }
            // Early media: the far end plays its own ringback or announcement.
            ev::CALL_PROGRESS if ours(self) => self.stop_tone(),
            ev::CALL_ESTABLISHED if ours(self) => {
                self.stop_tone();
                self.answered = true;
                self.emit_state(call, CallState::Talking);
            }
            ev::CALL_CLOSED if ours(self) => {
                let code = unsafe { ffi::call_scode(call) };
                let reason = if text.is_empty() { format!("{code}") } else { text };
                self.ended(reason);
            }
            ev::CALL_HOLD if ours(self) => self.emit(Event::RemoteHold(true)),
            ev::CALL_RESUME if ours(self) => self.emit(Event::RemoteHold(false)),
            ev::AUDIO_ERROR => self.emit(Event::Notice(format!("Sound device: {text}"))),
            _ => {}
        }
    }

    fn emit_state(&self, call: *mut ffi::call, state: CallState) {
        let peer = ffi::string(unsafe { ffi::call_peeruri(call) });
        let name = ffi::string(unsafe { ffi::call_peername(call) });
        self.emit(Event::Call { state, peer, name });
    }
}

fn null() -> *const std::ffi::c_char {
    std::ptr::null()
}

/// `pipewire,` → ("pipewire", ""); `alsa,default` → ("alsa", "default").
fn split_dev(s: &str) -> (&str, &str) {
    s.split_once(',').unwrap_or((s, ""))
}

fn errno(err: c_int) -> String {
    std::io::Error::from_raw_os_error(err).to_string()
}

extern "C" fn on_event(e: u32, event: *mut ffi::bevent, arg: *mut c_void) {
    let ctx = unsafe { &mut *(arg as *mut Ctx) };
    ctx.event(e, event);
}

extern "C" fn on_cmd(_id: c_int, data: *mut c_void, arg: *mut c_void) {
    let ctx = unsafe { &mut *(arg as *mut Ctx) };
    let cmd = unsafe { Box::from_raw(data as *mut Cmd) };
    ctx.command(*cmd);
}

extern "C" fn on_exit(_arg: *mut c_void) {
    unsafe { ffi::re_cancel() };
}

fn config_text(opts: &Options) -> String {
    let cafile = ["/etc/ssl/certs/ca-certificates.crt", "/etc/pki/tls/certs/ca-bundle.crt", "/etc/ssl/cert.pem"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists());
    let mut c = String::new();
    c += &format!("sip_listen\t{}\n", opts.listen);
    if let Some(ca) = cafile {
        c += &format!("sip_cafile\t{ca}\n");
    }
    if let Some(i) = &opts.iface {
        c += &format!("net_interface\t{i}\n");
    }
    c += "call_max_calls\t1\n";
    c += &format!("audio_player\t{}\naudio_source\t{}\naudio_alert\t{}\n", opts.player, opts.source, opts.alert);
    c += "rtp_ports\t10000-20000\n";
    c += "audio_jitter_buffer_type\tadaptive\naudio_jitter_buffer_ms\t60-300\n";
    // Only the sound modules the devices name, so a missing PipeWire isn't an error.
    let devs = [&opts.source, &opts.player, &opts.alert];
    for m in ["pipewire", "alsa", "aufile"] {
        if devs.iter().any(|d| split_dev(d).0 == m) {
            c += &format!("module\t{m}\n");
        }
    }
    for m in ["g711", "opus", "auresamp", "auconv", "stun", "turn", "ice", "srtp"] {
        c += &format!("module\t{m}\n");
    }
    c += "opus_bitrate\t32000\nopus_stereo\tno\nopus_sprop_stereo\tno\n";
    c
}

fn run(opts: Options, tx: Sender<Event>, wake: Box<dyn Fn() + Send>, mq_tx: Sender<Result<SendPtr, String>>) {
    let fail = |tx: &Sender<Event>, mq_tx: &Sender<Result<SendPtr, String>>, msg: String| {
        let _ = tx.send(Event::Failed(msg.clone()));
        let _ = mq_tx.send(Err(msg));
    };
    unsafe {
        if ffi::libre_init() != 0 {
            return fail(&tx, &mq_tx, "libre could not start".into());
        }
        let debug = std::env::var_os("DIALO_DEBUG").is_some();
        ffi::log_enable_info(debug);
        let conf = config_text(&opts);
        let mut err = ffi::conf_configure_buf(conf.as_ptr(), conf.len());
        if err == 0 {
            err = ffi::baresip_init(ffi::conf_config());
        }
        if err == 0 {
            err = ffi::ua_init(c"Dialo/0.1".as_ptr(), true, true, true);
        }
        if err != 0 {
            ffi::libre_close();
            return fail(&tx, &mq_tx, format!("SIP could not start ({})", errno(err)));
        }
        let ctx = Box::into_raw(Box::new(Ctx {
            tx: tx.clone(),
            wake,
            opts,
            ua: null_mut(),
            account: None,
            call: None,
            answered: false,
            tone: null_mut(),
            quitting: false,
            local_hangup: false,
        }));
        let arg = ctx as *mut c_void;
        uag_set_exit(arg);
        ffi::bevent_register(on_event, arg);
        err = ffi::conf_modules();
        if err != 0 {
            let _ = tx.send(Event::Notice(format!("Some audio modules did not load ({})", errno(err))));
        }
        let mut mq = null_mut();
        if ffi::mqueue_alloc(&mut mq, on_cmd, arg) != 0 {
            return fail(&tx, &mq_tx, "No command queue".into());
        }
        let _ = mq_tx.send(Ok(SendPtr(mq)));

        ffi::re_main(None);

        ffi::bevent_unregister(on_event);
        (*ctx).stop_tone();
        ffi::mem_deref(mq as *mut c_void);
        ffi::ua_close();
        ffi::module_app_unload();
        ffi::conf_close();
        ffi::baresip_close();
        ffi::mod_close();
        drop(Box::from_raw(ctx));
        ffi::libre_close();
    }
}

unsafe fn uag_set_exit(arg: *mut c_void) {
    unsafe { ffi::uag_set_exit_handler(on_exit, arg) };
}

#[cfg(test)]
mod tests {
    #[test]
    fn devices_split() {
        assert_eq!(super::split_dev("pipewire,"), ("pipewire", ""));
        assert_eq!(super::split_dev("alsa,default"), ("alsa", "default"));
        assert_eq!(super::split_dev("aufile,/tmp/a.wav"), ("aufile", "/tmp/a.wav"));
    }
}

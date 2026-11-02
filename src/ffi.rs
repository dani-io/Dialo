//! The part of libre and baresip (4.12) Dialo calls, declared by hand. Every function here
//! must be called on the engine thread, the one running `re_main`, except `mqueue_push`.

#![allow(non_camel_case_types, dead_code)]

use std::ffi::{c_char, c_int, c_void};

#[repr(C)]
pub struct ua {
    _p: [u8; 0],
}
#[repr(C)]
pub struct call {
    _p: [u8; 0],
}
#[repr(C)]
pub struct account {
    _p: [u8; 0],
}
#[repr(C)]
pub struct audio {
    _p: [u8; 0],
}
#[repr(C)]
pub struct bevent {
    _p: [u8; 0],
}
#[repr(C)]
pub struct mqueue {
    _p: [u8; 0],
}
#[repr(C)]
pub struct mbuf {
    _p: [u8; 0],
}
#[repr(C)]
pub struct play {
    _p: [u8; 0],
}
#[repr(C)]
pub struct player {
    _p: [u8; 0],
}
#[repr(C)]
pub struct sip_msg {
    _p: [u8; 0],
}
#[repr(C)]
pub struct sip {
    _p: [u8; 0],
}
#[repr(C)]
pub struct config {
    _p: [u8; 0],
}

/// `enum bevent_ev`, in the order baresip.h declares it.
pub mod ev {
    pub const REGISTERING: u32 = 0;
    pub const REGISTER_OK: u32 = 1;
    pub const REGISTER_FAIL: u32 = 2;
    pub const UNREGISTERING: u32 = 3;
    pub const SHUTDOWN: u32 = 8;
    pub const EXIT: u32 = 9;
    pub const CALL_INCOMING: u32 = 10;
    pub const CALL_OUTGOING: u32 = 11;
    pub const CALL_RINGING: u32 = 12;
    pub const CALL_PROGRESS: u32 = 13;
    pub const CALL_ANSWERED: u32 = 14;
    pub const CALL_ESTABLISHED: u32 = 15;
    pub const CALL_CLOSED: u32 = 16;
    pub const CALL_DTMF_START: u32 = 20;
    pub const AUDIO_ERROR: u32 = 27;
    pub const CALL_HOLD: u32 = 30;
    pub const CALL_RESUME: u32 = 31;
    pub const SIPSESS_CONN: u32 = 36;
}

pub const VIDMODE_OFF: c_int = 0;
pub const KEYCODE_REL: c_char = 0x04;

pub type bevent_h = extern "C" fn(ev: u32, event: *mut bevent, arg: *mut c_void);
pub type mqueue_h = extern "C" fn(id: c_int, data: *mut c_void, arg: *mut c_void);
pub type ua_exit_h = extern "C" fn(arg: *mut c_void);
pub type re_signal_h = extern "C" fn(sig: c_int);

unsafe extern "C" {
    // libre
    pub fn libre_init() -> c_int;
    pub fn libre_close();
    pub fn re_main(signalh: Option<re_signal_h>) -> c_int;
    pub fn re_cancel();
    pub fn mem_deref(data: *mut c_void) -> *mut c_void;
    pub fn mqueue_alloc(mqp: *mut *mut mqueue, h: mqueue_h, arg: *mut c_void) -> c_int;
    pub fn mqueue_push(mq: *mut mqueue, id: c_int, data: *mut c_void) -> c_int;
    pub fn mbuf_alloc(size: usize) -> *mut mbuf;
    pub fn mbuf_write_mem(mb: *mut mbuf, buf: *const u8, size: usize) -> c_int;

    // baresip
    pub fn conf_configure_buf(buf: *const u8, sz: usize) -> c_int;
    pub fn conf_config() -> *mut config;
    pub fn conf_modules() -> c_int;
    pub fn conf_close();
    pub fn baresip_init(cfg: *mut config) -> c_int;
    pub fn baresip_close();
    pub fn baresip_player() -> *mut player;
    pub fn mod_close();
    pub fn module_app_unload();
    pub fn log_enable_info(enable: bool);
    pub fn log_enable_debug(enable: bool);

    pub fn ua_init(software: *const c_char, udp: bool, tcp: bool, tls: bool) -> c_int;
    pub fn ua_close();
    pub fn ua_stop_all(forced: bool);
    pub fn uag_set_exit_handler(exith: ua_exit_h, arg: *mut c_void);
    pub fn uag_call_find(id: *const c_char) -> *mut call;
    pub fn bevent_register(eh: bevent_h, arg: *mut c_void) -> c_int;
    pub fn bevent_unregister(eh: bevent_h);
    pub fn bevent_get_call(event: *const bevent) -> *mut call;
    pub fn bevent_get_ua(event: *const bevent) -> *mut ua;
    pub fn bevent_get_text(event: *const bevent) -> *const c_char;
    pub fn bevent_get_msg(event: *const bevent) -> *const sip_msg;
    pub fn bevent_stop(event: *mut bevent);
    pub fn uag_find_msg(msg: *const sip_msg) -> *mut ua;
    pub fn uag_sip() -> *mut sip;
    pub fn ua_accept(ua: *mut ua, msg: *const sip_msg) -> c_int;
    pub fn sip_treply(stp: *mut *mut c_void, sip: *mut sip, msg: *const sip_msg, scode: u16, reason: *const c_char) -> c_int;

    pub fn ua_alloc(uap: *mut *mut ua, aor: *const c_char) -> c_int;
    pub fn ua_destroy(ua: *mut ua) -> u32;
    pub fn ua_register(ua: *mut ua) -> c_int;
    pub fn ua_unregister(ua: *mut ua);
    pub fn ua_isregistered(ua: *const ua) -> bool;
    pub fn ua_account(ua: *const ua) -> *mut account;
    pub fn ua_connect(ua: *mut ua, callp: *mut *mut call, from_uri: *const c_char, req_uri: *const c_char, vmode: c_int) -> c_int;
    pub fn ua_answer(ua: *mut ua, call: *mut call, vmode: c_int) -> c_int;
    pub fn ua_hangup(ua: *mut ua, call: *mut call, scode: u16, reason: *const c_char);
    pub fn account_set_auth_pass(acc: *mut account, pass: *const c_char) -> c_int;
    pub fn account_aor(acc: *const account) -> *const c_char;

    pub fn call_id(call: *const call) -> *const c_char;
    pub fn call_peeruri(call: *const call) -> *const c_char;
    pub fn call_peername(call: *const call) -> *const c_char;
    pub fn call_scode(call: *const call) -> u16;
    pub fn call_is_outgoing(call: *const call) -> bool;
    pub fn call_hold(call: *mut call, hold: bool) -> c_int;
    pub fn call_send_digit(call: *mut call, key: c_char) -> c_int;
    pub fn call_audio(call: *const call) -> *mut audio;
    pub fn audio_mute(a: *mut audio, muted: bool);

    pub fn play_tone(
        playp: *mut *mut play,
        player: *mut player,
        tone: *mut mbuf,
        srate: u32,
        ch: u8,
        repeat: c_int,
        play_mod: *const c_char,
        play_dev: *const c_char,
    ) -> c_int;
}

/// A C string from baresip as an owned `String`; empty for null.
pub fn string(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

# Dialo

A small, keyboard-first SIP softphone for Linux, made for Hyprland (and any Wayland or
X11 desktop). One binary of about 7.5 MB, drawn with egui on the GPU, with SIP and audio
from [baresip](https://github.com/baresip/baresip) compiled in.

- Type a number or a name and press Enter. Contacts and recent calls match as you type.
- Every action is in the command palette (Ctrl+K) and has a key.
- Contacts come from Google (sign in once) or any vCard file.
- One Dialo per session: `dialo answer`, `dialo hangup` and the rest drive the running
  window, so Hyprland binds work from anywhere.
- G.711 and Opus, SRTP, STUN/TURN/ICE, UDP, TCP or TLS. Sound goes through PipeWire,
  with ALSA as the fallback.

## Build

You need Rust 1.88 or newer, a C compiler, CMake, and the -dev packages of OpenSSL,
Opus, PipeWire, ALSA and zlib.

```bash
# Arch
sudo pacman -S --needed base-devel cmake openssl opus pipewire alsa-lib zlib
# Debian / Ubuntu
sudo apt install build-essential cmake pkg-config libssl-dev libopus-dev libpipewire-0.3-dev libasound2-dev zlib1g-dev

tools/fetch-deps.sh          # libre and baresip, at the release Dialo is built against
cargo build --release
./target/release/dialo --install
```

`--install` puts Dialo in `~/.local/bin` and your launcher, and makes it open `tel:`,
`sip:` and `callto:` links. `--uninstall` takes those out again.

## First start

Dialo asks for your SIP account: the server (your PBX or provider), the extension and
the password. Settings (Ctrl+,) has the transport, SRTP, an outbound proxy, STUN and
the theme.

## Keys

| Action | Keys |
|---|---|
| Call, or answer | Enter |
| Hang up, or decline | Esc (twice while talking) |
| Command palette | Ctrl+K, Ctrl+Shift+P |
| Contacts | Ctrl+B |
| Import contacts from a vCard file | Ctrl+I (or drop the file on the window) |
| Sync Google Contacts | Ctrl+Shift+G |
| Mute, hold | Ctrl+M, Ctrl+H |
| Keypad | Ctrl+D |
| Call the last number again | Ctrl+R |
| Next theme | Ctrl+T |
| Settings | Ctrl+, |
| Every key | F1 |

During a call, typed digits, `*` and `#` go out as tones.

## Hyprland

Add these to `hyprland.conf` (rule syntax of Hyprland 0.48 and later; older versions
spell it `windowrulev2`):

```ini
# Answer and hang up from anywhere
bind = SUPER, F9,  exec, dialo answer
bind = SUPER, F10, exec, dialo hangup
bind = SUPER, F11, exec, dialo mute
bind = SUPER, P,   exec, dialo            # open Dialo, or bring it forward

# A phone-sized window that floats
windowrule = float, class:^(dialo)$
windowrule = size 380 640, class:^(dialo)$
windowrule = move 100%-400 60, class:^(dialo)$

# Start hidden in a special workspace, ready to ring
exec-once = [workspace special:phone silent] dialo
bind = SUPER, grave, togglespecialworkspace, phone
```

An incoming call raises a desktop notification with Answer and Decline buttons. mako,
dunst, swaync and the others all show it; it needs `notify-send` (libnotify).

## Commands

```
dialo                 open Dialo, or bring the running one forward
dialo call 201        call a number, an extension or a SIP address
dialo tel:+98...      the same, for links
dialo answer          answer the ringing call
dialo hangup          hang up, cancel, or decline
dialo mute | hold     toggle the microphone, or hold
dialo import a.vcf    add contacts from a vCard file
dialo sync-google     read your contacts from Google
dialo quit            close the running Dialo
```

## Google Contacts

Ctrl+Shift+G (or `dialo sync-google`) opens Google's sign-in in your browser the first
time, then reads the phone numbers of everyone in your contacts. Dialo asks only for
`contacts.readonly`, so it never changes them. Every sync replaces what came from Google
before; contacts imported from files stay.

Google needs an OAuth client. Until a release build carries one, make your own:

1. In [Google Cloud Console](https://console.cloud.google.com/), create a project and
   turn on the **People API**.
2. Under **Google Auth Platform > Clients**, create a client of type **Desktop app**.
3. On the **Audience** page, add your Google account as a test user.
4. Paste the client ID and secret into Dialo's Settings > Google Contacts.

While the app is in "Testing", Google asks you to sign in again every 7 days.

To build Dialo with a client in it, set `DIALO_GOOGLE_CLIENT_ID` and
`DIALO_GOOGLE_CLIENT_SECRET` when you run `cargo build`.

Without Google, export your contacts as vCard (Google Contacts > Export, or any phone)
and import the file.

## Files

| What | Where |
|---|---|
| Settings and the SIP account | `~/.config/dialo/config.json` (owner-only) |
| Google sign-in | `~/.config/dialo/google.json` (owner-only) |
| Contacts | `~/.local/share/dialo/contacts.json` |
| Recent calls | `~/.local/state/dialo/` |

## Development

```bash
cargo test           # unit tests
tools/loopback.sh    # a real SIP call between two Dialo engines, no PBX or sound card
```

## License

GPL-3.0-or-later. baresip and libre are BSD-licensed.

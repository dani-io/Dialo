#!/usr/bin/env bash
# A real SIP call on this machine, no PBX and no sound card: Dialo's engine calls a stock
# baresip that answers by itself, both sides reading and writing WAV files. Passes when
# the call is set up, talked over (audio both ways), and torn down.
#
#   tools/loopback.sh <path to a baresip built with g711, aufile, account, menu>
set -euo pipefail
cd "$(dirname "$0")/.."
PEER_BIN=${1:?give the path to a baresip binary}
W=$(mktemp -d)
trap 'kill $PEER 2>/dev/null || true; rm -rf "$W"' EXIT

# Two seconds of 440 Hz for each side to send.
python3 - "$W/tone.wav" <<'PY'
import math, struct, sys, wave
w = wave.open(sys.argv[1], "wb"); w.setnchannels(1); w.setsampwidth(2); w.setframerate(8000)
w.writeframes(b"".join(struct.pack("<h", int(8000 * math.sin(2 * math.pi * 440 * n / 8000))) for n in range(8000 * 2)))
PY

mkdir -p "$W/peer"
cat > "$W/peer/config" <<EOF
sip_listen 127.0.0.1:5070
audio_player aufile,$W/peer-heard.wav
audio_source aufile,$W/tone.wav
audio_alert aufile,$W/peer-ring.wav
module g711
module aufile
module_app ctrl_tcp
ctrl_tcp_listen 127.0.0.1:4444
module_app account
module_app menu
EOF
echo "<sip:peer@127.0.0.1:5070>;regint=0;answermode=auto" > "$W/peer/accounts"
"$PEER_BIN" -f "$W/peer" -n 127.0.0.1 > "$W/peer.log" 2>&1 < /dev/zero &
PEER=$!
sleep 1

cargo build -q
set +e
target/debug/dialo --engine-test 127.0.0.1:5070 me peer "$W/tone.wav" "$W/dialo-heard.wav" 1 | tee "$W/dialo.log"
code=${PIPESTATUS[0]}
set -e

sleep 0.5
heard() { python3 -c "import wave,sys; w=wave.open(sys.argv[1]); print(w.getnframes())" "$1" 2>/dev/null || echo 0; }
echo "dialo heard $(heard "$W/dialo-heard.wav") samples, the peer heard $(heard "$W/peer-heard.wav")"
if [ "$code" = 0 ] && [ "$(heard "$W/dialo-heard.wav")" -gt 3000 ] && [ "$(heard "$W/peer-heard.wav")" -gt 3000 ]; then
    echo "PASS: Dialo calls out"
else
    echo "FAIL calling out (exit $code)"; echo "--- peer log"; tail -30 "$W/peer.log"; exit 1
fi

# The other way: the peer calls Dialo, which rings and answers.
rm -f "$W/dialo-heard.wav" "$W/peer-heard.wav"
(sleep 1.5; python3 - <<'PY'
import json, socket
cmd = json.dumps({"command": "dial", "params": "sip:me@127.0.0.1:5072", "token": "t"}).encode()
s = socket.create_connection(("127.0.0.1", 4444)); s.sendall(b"%d:%s," % (len(cmd), cmd)); s.recv(4096); s.close()
PY
) &
set +e
target/debug/dialo --engine-test 127.0.0.1:5070 me - "$W/tone.wav" "$W/dialo-heard.wav" 1 | tee "$W/dialo2.log"
code=${PIPESTATUS[0]}
set -e
sleep 0.5
echo "dialo heard $(heard "$W/dialo-heard.wav") samples, the peer heard $(heard "$W/peer-heard.wav"); ringtone $(heard "$W/dialo-heard.wav.ring")"
if [ "$code" = 0 ] && grep -q "Incoming" "$W/dialo2.log" && [ "$(heard "$W/dialo-heard.wav")" -gt 3000 ] && [ "$(heard "$W/peer-heard.wav")" -gt 3000 ]; then
    echo "PASS: Dialo answers"
else
    echo "FAIL answering (exit $code)"; echo "--- peer log"; tail -30 "$W/peer.log"; cat "$W/dialo2.log"; exit 1
fi

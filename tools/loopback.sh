#!/usr/bin/env bash
# A real SIP call on this machine, no PBX and no sound card: one Dialo engine waits and
# answers, another calls it; both read and write WAV files. Passes when the call rings,
# is talked over (audio both ways), and is torn down.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -q
W=$(mktemp -d)
trap 'kill $CALLEE 2>/dev/null || true; rm -rf "${W:?}"' EXIT

python3 - "$W/tone.wav" <<'PY'
import math, struct, sys, wave
w = wave.open(sys.argv[1], "wb"); w.setnchannels(1); w.setsampwidth(2); w.setframerate(8000)
w.writeframes(b"".join(struct.pack("<h", int(8000 * math.sin(2 * math.pi * 440 * n / 8000))) for n in range(8000 * 3)))
PY

target/debug/dialo --engine-test 127.0.0.1:5072 callee - "$W/tone.wav" "$W/callee.wav" 2 > "$W/callee.log" 2>&1 &
CALLEE=$!
sleep 1.5
set +e
DIALO_LISTEN=127.0.0.1:5074 target/debug/dialo --engine-test 127.0.0.1:5072 caller sip:callee@127.0.0.1:5072 "$W/tone.wav" "$W/caller.wav" 2 > "$W/caller.log" 2>&1
code=$?
wait $CALLEE; callee_code=$?
set -e

frames() { python3 -c "import wave,sys; print(wave.open(sys.argv[1]).getnframes())" "$1" 2>/dev/null || echo 0; }
echo "caller heard $(frames "$W/caller.wav") samples, callee heard $(frames "$W/callee.wav"), ringtone $(frames "$W/callee.wav.ring")"
if [ $code = 0 ] && [ $callee_code = 0 ] && grep -q Incoming "$W/callee.log" \
    && [ "$(frames "$W/caller.wav")" -gt 8000 ] && [ "$(frames "$W/callee.wav")" -gt 8000 ] && [ "$(frames "$W/callee.wav.ring")" -gt 0 ]; then
    echo PASS
else
    echo "FAIL (caller $code, callee $callee_code)"; echo "--- caller"; cat "$W/caller.log"; echo "--- callee"; cat "$W/callee.log"; exit 1
fi

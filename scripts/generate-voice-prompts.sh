#!/usr/bin/env bash
set -euo pipefail

kokoro_url="${BTS_KOKORO_URL:-http://127.0.0.1:8880/v1/audio/speech}"
output_dir="${BTS_ASTERISK_SOUNDS_DIR:-/var/lib/asterisk/sounds/en/bts}"
temporary_dir="$(mktemp -d)"

cleanup() {
    rm -rf -- "$temporary_dir"
}

trap cleanup EXIT

for command in curl ffmpeg install; do
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "Required command not found: $command" >&2
        exit 1
    fi
done

echo "Generating the BTS welcome prompt with Kokoro Emma..."

curl \
    --fail \
    --silent \
    --show-error \
    --connect-timeout 5 \
    --max-time 120 \
    --header 'Content-Type: application/json' \
    --data '{
        "model": "kokoro",
        "voice": "bf_emma",
        "input": "Welcome to Bansleben Telephone Services. Press two for the time. Press three for the weather. Press zero to clear the display.",
        "response_format": "wav",
        "speed": 1.0
    }' \
    "$kokoro_url" \
    --output "$temporary_dir/welcome-kokoro.wav"

ffmpeg \
    -hide_banner \
    -loglevel error \
    -y \
    -i "$temporary_dir/welcome-kokoro.wav" \
    -ar 8000 \
    -ac 1 \
    -c:a pcm_s16le \
    "$temporary_dir/welcome.wav"

install -d "$output_dir"
install -m 0644 "$temporary_dir/welcome.wav" "$output_dir/welcome.wav"

echo "Installed $output_dir/welcome.wav"

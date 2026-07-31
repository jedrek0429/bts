#!/usr/bin/env bash
set -euo pipefail

kokoro_url="${BTS_KOKORO_URL:-http://127.0.0.1:8880/v1/audio/speech}"
output_dir="${BTS_ASTERISK_SOUNDS_DIR:-/var/lib/asterisk/sounds/en/bts}"
voice="${BTS_KOKORO_VOICE:-bf_emma}"
speed="${BTS_KOKORO_SPEED:-1.05}"
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

if [[ ! "$voice" =~ ^[a-z0-9_+().-]+$ ]]; then
    echo "BTS_KOKORO_VOICE contains unsupported characters" >&2
    exit 1
fi

if [[ ! "$speed" =~ ^[0-9]+([.][0-9]+)?$ ]]; then
    echo "BTS_KOKORO_SPEED must be a positive number" >&2
    exit 1
fi

generate_prompt() {
    local name="$1"
    local text="$2"
    local kokoro_wav="$temporary_dir/$name-kokoro.wav"
    local telephone_wav="$temporary_dir/$name.wav"

    echo "Generating $name with Kokoro voice $voice..."

    curl \
        --fail \
        --silent \
        --show-error \
        --connect-timeout 5 \
        --max-time 120 \
        --header 'Content-Type: application/json' \
        --data "$(printf \
            '{"model":"kokoro","voice":"%s","input":"%s","response_format":"wav","speed":%s}' \
            "$voice" "$text" "$speed")" \
        "$kokoro_url" \
        --output "$kokoro_wav"

    ffmpeg \
        -hide_banner \
        -loglevel error \
        -y \
        -i "$kokoro_wav" \
        -ar 8000 \
        -ac 1 \
        -c:a pcm_s16le \
        "$telephone_wav"

    install -m 0644 "$telephone_wav" "$output_dir/$name.wav"
    echo "Installed $output_dir/$name.wav"
}

install -d "$output_dir"

generate_prompt "welcome" "Welcome to Bansleben Telephone Services!"
generate_prompt "press-2-time" "Press two for the time."
generate_prompt "press-3-weather" "Press three for the weather."
generate_prompt "press-4-clear" "Press four to clear the display."

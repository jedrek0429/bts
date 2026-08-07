#!/usr/bin/env bash
set -euo pipefail

repository_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
test_root=$(mktemp -d)
trap 'rm -rf "$test_root"' EXIT
install -d "$test_root/bin" "$test_root/config" "$test_root/state"
ln -s "$repository_root/scripts/test-support/fake-tmux" "$test_root/bin/tmux"
log=$test_root/tmux.log

run_dev() {
    PATH="$test_root/bin:$PATH" \
        BTS_DEV_CONFIG_DIR="$test_root/config" \
        BTS_DEV_STATE_DIR="$test_root/state" \
        BTS_DEV_LEGACY_ENV="$test_root/no-legacy.env" \
        BTS_DEV_DETACH=1 \
        BTS_TMUX_TEST_LOG="$log" \
        "$repository_root/scripts/bts-dev" "$@"
}

# Core is independently runnable and receives isolated state without ARI settings.
: > "$log"
BTS_DEV_SESSION=core-test run_dev up core > "$test_root/core-output"
grep -q '^new-session .*core' "$log"
grep -q 'BTS_CORE_TERMINAL_STATE_PATH' "$log"
grep -q 'unset.*BTS_ARI_PASSWORD' "$log"
grep -q "$test_root/state/core-test/terminals.json" "$log"
if grep -Eq 'addons|telephony|ARI password|ari-tunnel' "$log"; then
    echo "Core-only development unexpectedly selected a dependent component." >&2
    exit 1
fi
grep -q 'Components: core' "$test_root/core-output"

# Component files remain scoped to their own pane and readiness follows Core.
printf '%s\n' 'BTS_CORE_BIND=127.0.0.1:3200' > "$test_root/config/core.env"
printf '%s\n' 'BTS_CORE_HTTP_URL=http://core.test:3200' > "$test_root/config/addons.env"
: > "$log"
BTS_DEV_SESSION=services-test run_dev up addons core > "$test_root/services-output"
grep -q "$test_root/config/core.env" "$log"
grep -q "$test_root/config/addons.env" "$log"
grep -q 'new-session .*core' "$log"
grep -q 'new-window .*addons' "$log"
if grep -q 'telephony' "$log"; then
    echo "Addons selection unexpectedly started Telephony." >&2
    exit 1
fi

# Telephony alone owns ARI prompting and optional tunnel setup.
printf '%s\n' \
    'BTS_CORE_URL=http://core.test:3100' \
    'BTS_ARI_SSH_HOST=admin@ari.test' \
    'BTS_ARI_TUNNEL_PORT=19088' > "$test_root/config/telephony.env"
: > "$log"
BTS_DEV_SESSION=telephony-test run_dev up telephony > "$test_root/telephony-output"
grep -q 'ari-tunnel' "$log"
grep -q 'BTS_ARI_PASSWORD' "$log"
grep -q '19088' "$log"
if grep -Eq 'bts-core|bts-addons' "$log"; then
    echo "Telephony-only development unexpectedly started Core or Addons." >&2
    exit 1
fi

# A matching existing session is reused without creating windows again.
: > "$log"
PATH="$test_root/bin:$PATH" \
    BTS_DEV_CONFIG_DIR="$test_root/config" \
    BTS_DEV_STATE_DIR="$test_root/state" \
    BTS_DEV_LEGACY_ENV="$test_root/no-legacy.env" \
    BTS_DEV_SESSION=core-test \
    BTS_DEV_DETACH=1 \
    BTS_TMUX_TEST_LOG="$log" \
    BTS_TMUX_TEST_SESSION_EXISTS=0 \
    "$repository_root/scripts/bts-dev" up core > "$test_root/reuse-output"
grep -q 'already exists; reusing it' "$test_root/reuse-output"
if grep -q '^new-session ' "$log"; then
    echo "The launcher recreated an existing session." >&2
    exit 1
fi
PATH="$test_root/bin:$PATH" \
    BTS_DEV_CONFIG_DIR="$test_root/config" \
    BTS_DEV_STATE_DIR="$test_root/state" \
    BTS_DEV_SESSION=core-test \
    BTS_TMUX_TEST_LOG="$log" \
    BTS_TMUX_TEST_SESSION_EXISTS=0 \
    "$repository_root/scripts/bts-dev" status core > "$test_root/status-output"
grep -q 'Status: running' "$test_root/status-output"

# Existing profiles remain data-driven when the headless selector is added.
: > "$log"
BTS_DEV_SESSION=voice-test run_dev up voice > "$test_root/voice-output"
grep -q 'Components: core,addons,telephony' "$test_root/voice-output"
grep -q 'Readiness order: core, addons, ARI tunnel, telephony, headless terminals, displays.' "$test_root/voice-output"

# The #34 profile uses the reusable headless runtime with two stable identities.
: > "$log"
BTS_DEV_SESSION=two-terminal-test run_dev up two-terminals > "$test_root/two-terminal-output"
grep -q 'Components: core,terminal:bedroom-display,terminal:dining-display' "$test_root/two-terminal-output"
grep -q 'new-session .*core' "$log"
grep -q 'new-window .*terminal-bedroom-display' "$log"
grep -q 'new-window .*terminal-dining-display' "$log"
grep -q 'bts-terminal-simulator' "$log"
grep -q 'BTS_TERMINAL_ID.*bedroom-display' "$log"
grep -q 'BTS_TERMINAL_ID.*dining-display' "$log"
if grep -Eq 'bts-display|bts-telephony|ARI password|ari-tunnel' "$log"; then
    echo "The two-terminal profile unexpectedly selected graphical or telephony components." >&2
    exit 1
fi

# Legacy shared development configuration is rejected with migration guidance.
printf '%s\n' 'RUST_LOG=debug' > "$test_root/dev.env"
if PATH="$test_root/bin:$PATH" \
    BTS_DEV_CONFIG_DIR="$test_root/config" \
    BTS_DEV_STATE_DIR="$test_root/state" \
    BTS_DEV_LEGACY_ENV="$test_root/dev.env" \
    BTS_DEV_SESSION=legacy-test \
    BTS_DEV_DETACH=1 \
    BTS_TMUX_TEST_LOG="$log" \
    "$repository_root/scripts/bts-dev" up core > "$test_root/legacy-output" 2>&1; then
    echo "The launcher accepted the legacy shared environment." >&2
    exit 1
fi
grep -q 'is no longer loaded' "$test_root/legacy-output"

# Named Display configuration is selectable, but graphical verification stays manual.
cp "$repository_root/deploy/dev/display.env.example" "$test_root/config/display-bedroom.env"
: > "$log"
BTS_DEV_SESSION=display-test run_dev up display:bedroom > "$test_root/display-output"
grep -q 'bts-display' "$log"
grep -q "$test_root/config/display-bedroom.env" "$test_root/display-output"

# Compatibility wrapper keeps the old command useful while selecting the explicit all profile.
: > "$log"
PATH="$test_root/bin:$PATH" \
    BTS_DEV_CONFIG_DIR="$test_root/config" \
    BTS_DEV_STATE_DIR="$test_root/state" \
    BTS_DEV_LEGACY_ENV="$test_root/no-legacy.env" \
    BTS_DEV_SESSION=wrapper-test \
    BTS_DEV_DETACH=1 \
    BTS_TMUX_TEST_LOG="$log" \
    "$repository_root/scripts/bts-tmux" > "$test_root/wrapper-output"
grep -q 'Components: core,addons,telephony' "$test_root/wrapper-output"

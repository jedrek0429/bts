#!/usr/bin/env bash
set -euo pipefail

repository_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
test_root=$(mktemp -d)
trap 'rm -rf "$test_root"' EXIT
install -d "$test_root/bin" "$test_root/state"
ln -s "$repository_root/scripts/test-support/fake-tmux" "$test_root/bin/tmux"
log="$test_root/tmux.log"
environment="$test_root/dev.env"
printf '%s\n' \
    'BTS_CORE_HTTP_URL=http://core.test:3100' \
    "BTS_ADDON_DATA_ROOT=$test_root/addons" > "$environment"

PATH="$test_root/bin:$PATH" \
    BTS_TMUX_ENV="$environment" \
    BTS_TMUX_SESSION=bts-test \
    BTS_TMUX_DETACH=1 \
    BTS_TMUX_TEST_LOG="$log" \
    "$repository_root/scripts/bts-tmux" > "$test_root/output"

api_version=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["core_api"])' "$repository_root/compatibility.json")
grep -q '^new-session ' "$log"
grep -q "BTS_CORE_API_PREFIX /api/v$api_version" "$log"
grep -q "BTS_CORE_WS_URL ws://127.0.0.1:3100/api/v$api_version/events/ws" "$log"
grep -q "BTS_ADDON_DATA_ROOT $test_root/addons" "$log"
grep -q 'new-window.*addons' "$log"
grep -q 'new-window.*telephony' "$log"
if grep -q 'ari-tunnel' "$log"; then
    echo "The default launcher unexpectedly created an ARI tunnel." >&2
    exit 1
fi

: > "$log"
PATH="$test_root/bin:$PATH" \
    BTS_TMUX_ENV="$environment" \
    BTS_TMUX_SESSION=bts-test \
    BTS_TMUX_DETACH=1 \
    BTS_TMUX_TEST_LOG="$log" \
    BTS_TMUX_TEST_SESSION_EXISTS=0 \
    "$repository_root/scripts/bts-tmux" > "$test_root/output"
grep -q 'already exists; recreate it to apply configuration changes' "$test_root/output"
if grep -q '^new-session ' "$log"; then
    echo "The launcher recreated an existing session." >&2
    exit 1
fi

if grep -q '^BTS_CORE_WS_URL=' "$repository_root/deploy/bts-dev.env.example"; then
    echo "The development environment must not override the generated Core API path." >&2
    exit 1
fi
if grep -q '^BTS_ARI_SSH_HOST=' "$repository_root/deploy/bts-dev.env.example"; then
    echo "The development environment must not enable an ARI tunnel by default." >&2
    exit 1
fi

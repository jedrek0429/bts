# Bansleben Telephone Services (BTS)

BTS is a phone-controlled home information system written in Rust. Asterisk handles calls, BTS routes telephone events through a central service, and independent addons update a dedicated graphical display.

The processes communicate over HTTP and WebSockets:

* `bts-core` retains display state and distributes events.
* `bts-telephony` bridges Asterisk ARI calls and DTMF to BTS.
* `bts-addons` supplies clock, weather and message behaviour.
* `bts-display` renders the current state full-screen.
* `bts-protocol` contains shared event and state types.

## Deployment on Arch Linux

The package installs the release binaries, systemd units, Cage kiosk session and a persistent configuration file. On boot, `bts-display` takes ownership of `tty1` through Cage and renders directly to the connected DRM display, while Core, Addons and Telephony run as isolated system services.

Build and install from a checkout:

```sh
makepkg -si
sudoedit /etc/bts/bts.env
sudo bts-install
```

Set at least `BTS_ARI_PASSWORD` in `/etc/bts/bts.env`. The installer creates dedicated service accounts, grants the display account access to DRM/input devices, reserves `tty1`, enables `bts.target`, and starts every configured component.

Useful commands:

```sh
systemctl status bts.target
systemctl status bts-core bts-addons bts-telephony bts-display
journalctl -u 'bts-*' -f
sudo systemctl restart bts.target
```

The display service deliberately conflicts with `getty@tty1.service`. Other virtual terminals remain available. To restore a login prompt on tty1:

```sh
sudo systemctl disable --now bts.target
sudo systemctl unmask --now getty@tty1.service
```

### Sony TV and DRM output

Cage normally selects the connected DRM output automatically. Where a host has several graphics devices, set `WLR_DRM_DEVICES=/dev/dri/card0` in `/etc/bts/bts.env`. Output mode and overscan remain compositor/DRM concerns; BTS itself fills the surface supplied by Cage.

### Continuous integration and delivery

Every pull request runs formatting, compilation, Clippy, tests, ShellCheck and systemd unit validation. Tags matching `v*` build an Arch package as a GitHub Actions artifact. Installing a newer package preserves `/etc/bts/bts.env` and reloads systemd unit definitions through a pacman hook.

## Voice prompts

BTS uses a local Kokoro service with the British `bf_emma` voice. The welcome and each menu option are generated separately in Asterisk's sounds directory and played as one playlist. Calls work without internet access after initial setup.

Start Kokoro:

```sh
docker run -d \
    --name kokoro \
    --restart unless-stopped \
    -p 127.0.0.1:8880:8880 \
    ghcr.io/remsky/kokoro-fastapi-cpu:v0.6.0
```

Generate prompts:

```sh
sudo -E /usr/lib/bts/generate-voice-prompts
```

Supported prompt settings:

```env
BTS_KOKORO_URL=http://127.0.0.1:8880/v1/audio/speech
BTS_ASTERISK_SOUNDS_DIR=/var/lib/asterisk/sounds/en/bts
BTS_KOKORO_VOICE=bf_emma
BTS_KOKORO_SPEED=1.05
```

## Development

```sh
cargo build --workspace
cargo test --workspace
cargo check --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Run the processes manually in separate terminals:

```sh
cargo run -p bts-core
cargo run -p bts-telephony
cargo run -p bts-addons
cage -- cargo run -p bts-display
```

## Configuration

```env
BTS_CORE_URL=http://127.0.0.1:3100
BTS_CORE_HTTP_URL=http://127.0.0.1:3100
BTS_CORE_WS_URL=ws://127.0.0.1:3100/api/v1/events/ws
BTS_ARI_URL=http://127.0.0.1:8088
BTS_ARI_USERNAME=bts
BTS_ARI_PASSWORD=CHANGE_ME
BTS_MENU_MEDIA_URIS=sound:bts/welcome,sound:bts/press-2-time,sound:bts/press-3-weather,sound:bts/press-0-clear
```

Core listens on port 3100:

* `POST /api/v1/events`
* `GET /api/v1/events/ws`
* `GET /api/v1/state`
* `GET /health`

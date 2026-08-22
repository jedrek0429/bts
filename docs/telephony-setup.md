# Setting up BTS Telephony

BTS Telephony connects four independently operated services:

```text
bts-telephony
    |
    +-- HTTP --> bts-core
    +-- HTTP --> Asterisk ARI
    +-- HTTP --> Kokoro TTS
```

Asterisk and Kokoro are external services used by BTS Telephony; the BTS
installer does not install or manage them. They may run on the BTS Telephony
computer or on other computers reachable over the network. `127.0.0.1` means
“this computer”. Replace it with a service computer's hostname or IP address
when the service is elsewhere. The easiest first setup is to run everything on
one computer.

## What the components do

- Asterisk handles the telephone call.
- ARI is the HTTP API that BTS uses to control Asterisk.
- Kokoro converts BTS text prompts into speech.
- `bts-telephony` connects Asterisk and Kokoro to BTS Core.

## Install Asterisk

On Debian, Ubuntu or Raspberry Pi OS:

```sh
sudo apt update
sudo apt install asterisk
sudo systemctl enable --now asterisk
```

On Arch Linux, Asterisk is supplied through the Arch User Repository (AUR), not
the official `pacman` repositories. Install the build tools, then build the
`asterisk` AUR package as your normal (non-root) user:

```sh
sudo pacman -S --needed base-devel git
git clone https://aur.archlinux.org/asterisk.git
cd asterisk
makepkg -si
```

Review an AUR package's `PKGBUILD` before building it. Then start the installed
`asterisk.service`:

```sh
sudo systemctl enable --now asterisk
```

Confirm that it is running:

```sh
systemctl status asterisk --no-pager
```

## Configure Asterisk ARI

ARI uses Asterisk's built-in HTTP server. For a same-computer setup, edit
`/etc/asterisk/http.conf`:

```ini
[general]
enabled = yes
bindaddr = 127.0.0.1
bindport = 8088
```

Create the BTS ARI account in `/etc/asterisk/ari.conf`. Replace the example
password with a long, unique password:

```ini
[general]
enabled = yes
pretty = yes

[bts]
type = user
read_only = no
password = REPLACE_WITH_A_STRONG_PASSWORD
```

Do not expose ARI to the Internet. If Asterisk is on another computer, replace
`127.0.0.1` in `bindaddr` with that computer's trusted LAN interface address and
allow TCP port 8088 through its firewall only from the BTS Telephony computer.

Apply the changes:

```sh
sudo systemctl restart asterisk
```

## Configure extension 200

`bts-telephony` listens for the Asterisk Stasis application named `bts`. Add a
dialplan context like this to `/etc/asterisk/extensions.conf`:

```ini
[bts]
exten => 200,1,NoOp(Enter BTS)
 same => n,Stasis(bts)
 same => n,Hangup()
```

Configure the telephone or SIP endpoint that will call BTS to use the `bts`
context. Reload the dialplan and inspect extension 200:

```sh
sudo asterisk -rx "dialplan reload"
sudo asterisk -rx "dialplan show 200@bts"
```

## Verify ARI before involving BTS

This request checks both the HTTP connection and the ARI username and password.
`curl` asks for the password instead of placing it in the command:

```sh
curl --fail --user bts \
  http://127.0.0.1:8088/ari/api-docs/resources.json >/dev/null \
  && echo "ARI is ready"
```

When Asterisk is remote, replace `127.0.0.1` with its hostname or IP address.

## Install Kokoro locally: easiest path

Kokoro-FastAPI provides the speech API consumed by BTS. Docker is one convenient
way to run it, but Docker and Kokoro are not BTS dependencies and remain under
the administrator's control.

```sh
docker run -d \
  --name kokoro \
  --restart unless-stopped \
  -p 127.0.0.1:8880:8880 \
  ghcr.io/remsky/kokoro-fastapi-cpu:v0.6.0
```

This binding exposes Kokoro only to the same computer. Check the API and render
real test speech using the same request as BTS:

```sh
curl --fail http://127.0.0.1:8880/docs >/dev/null
curl --fail \
  --header 'Content-Type: application/json' \
  --data '{"model":"kokoro","voice":"bf_emma","input":"BTS speech test","response_format":"wav","speed":1.05}' \
  --output /tmp/bts-kokoro-test.wav \
  http://127.0.0.1:8880/v1/audio/speech
file /tmp/bts-kokoro-test.wav
```

The final command should identify WAV audio.

## Run Kokoro on another computer

Suppose the Kokoro computer's trusted LAN address is `192.168.1.50`. Bind the
container specifically to that address:

```sh
docker run -d \
  --name kokoro \
  --restart unless-stopped \
  -p 192.168.1.50:8880:8880 \
  ghcr.io/remsky/kokoro-fastapi-cpu:v0.6.0
```

The BTS Telephony configuration then uses:

```env
BTS_KOKORO_URL=http://192.168.1.50:8880/v1/audio/speech
```

Generated speech defaults to `/var/lib/asterisk/sounds/en/bts-generated`. If
Asterisk uses another sound tree, set an absolute namespace owned only by BTS:

```env
BTS_ASTERISK_GENERATED_SOUNDS_DIR=/srv/asterisk/sounds/en/bts-generated
```

`bts-install` reconciles traversal of the configured parents for the
Telephony service identity and creates only this generated namespace as
writable by BTS. `doctor` checks the same configured path as that identity.

The Kokoro computer must listen on an address reachable from the BTS computer.
Limit firewall access to the BTS host or trusted LAN; remote TTS does not need
to be Internet-facing.

## Configure BTS Telephony

A fresh Telephony, server or full installation asks for the service addresses
and ARI credentials automatically:

```sh
sudo bts-install install telephony
```

To change an existing installation, use the same configuration flow:

```sh
sudo bts-install configure telephony
```

The same-computer defaults are:

```text
Asterisk ARI URL: http://127.0.0.1:8088
ARI username: bts
Kokoro TTS URL: http://127.0.0.1:8880/v1/audio/speech
```

Enter the password from `/etc/asterisk/ari.conf` when prompted. The installer
saves configuration even when an external service is temporarily unavailable;
it does not install Asterisk, Docker or Kokoro.

## Verify the complete setup

Run:

```sh
sudo bts-install doctor
```

BTS Core, Asterisk ARI credentials, Kokoro reachability and test speech should
all pass. Then call extension 200 and confirm that the BTS welcome prompt plays.
Telephone, audio and DTMF behaviour must be verified on real hardware.

## Troubleshooting

- **Asterisk ARI unreachable:** check `systemctl status asterisk` and
  `ss -ltn | grep 8088`. Confirm that the configured address names the Asterisk
  computer, not necessarily `127.0.0.1`.
- **ARI authentication failed:** compare the username and password in
  `/etc/asterisk/ari.conf`, then run `sudo bts-install configure telephony`.
- **Kokoro unreachable:** run `docker ps --filter name=kokoro` on the Kokoro
  computer and repeat the `/docs` request from the BTS computer.
- **Kokoro returned invalid speech:** repeat the synthesis request above and run
  `docker logs kokoro`. Confirm the model is `kokoro`, voice is `bf_emma`, and
  response format is `wav`.
- **BTS Telephony is not running:** run
  `systemctl status bts-telephony --no-pager` and
  `journalctl -u bts-telephony -n 50 --no-pager`.
- **Local/remote address confusion:** `127.0.0.1` always refers to the computer
  running the command. Use the other service computer's LAN hostname or IP
  address and check its firewall when Asterisk, Kokoro or Core is remote.

After correcting a problem, run `sudo bts-install doctor` again.

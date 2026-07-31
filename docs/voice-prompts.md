# Voice prompts

BTS uses a local Kokoro service with the British `bf_emma` voice. Prompts work without internet access after generation.

Start Kokoro:

```sh
docker run -d \
  --name kokoro \
  --restart unless-stopped \
  -p 127.0.0.1:8880:8880 \
  ghcr.io/remsky/kokoro-fastapi-cpu:v0.6.0
```

Generate and install the prompts:

```sh
sudo -E /usr/lib/bts/generate-voice-prompts
```

The generator accepts these environment variables:

```env
BTS_KOKORO_URL=http://127.0.0.1:8880/v1/audio/speech
BTS_ASTERISK_SOUNDS_DIR=/var/lib/asterisk/sounds/en/bts
BTS_KOKORO_VOICE=bf_emma
BTS_KOKORO_SPEED=1.05
```

The welcome message and each menu option are generated separately and played as one Asterisk playlist.

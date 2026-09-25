# Matrix Welcome Bot

A small Matrix bot that invites people into a set of rooms when they send a trigger phrase (`!welcome` by default) in an allow-listed room. Built on the [Matrix Rust SDK](https://github.com/matrix-org/matrix-rust-sdk), it handles end-to-end encryption properly — including cross-signing recovery and historical key sharing — so invitees can actually read the room's encrypted history via an exported element-keys.txt, not just join a room they can't decrypt anything in.

## How it works

1. The bot logs in (or restores a saved session) as a dedicated Matrix account.
2. It recovers its cross-signing identity from a recovery key, so the account is verified rather than an untrusted device.
3. It optionally imports a historical room-key export file (e.g. from Element), so it holds decryption keys for messages sent before the bot joined.
4. It watches for a trigger phrase, sent from an allow-listed set of rooms.
5. On trigger, it invites the sender into a configured set of target rooms, sharing message-history decryption keys with them on invite (MSC4268).

## Configuration

All configuration is via environment variables — see [`.env.example`](./.env.example) for the full list with comments. The important ones:

| Variable | Required | Description |
|---|---|---|
| `MATRIX_USERNAME` / `MATRIX_PASSWORD` | ✅ | Bot account credentials |
| `SPACE_CHILD_ROOMS` | ✅ | Comma-separated room IDs the bot invites people into |
| `TRIGGER_ROOMS` | ✅ | Comma-separated room IDs the trigger phrase is accepted from — **required**, no default. Anywhere else, the trigger is ignored. |
| `BOT_RECOVERY_KEY` | recommended | Recovery key used to establish cross-signing trust for this device |
| `KEY_EXPORT_PASSPHRASE` | optional | Passphrase for importing a historical Element key export |
| `WELCOME_TRIGGER` | optional | Defaults to `!welcome` |
| `WELCOME_DEBOUNCE_SECONDS` | optional | Defaults to `30` — ignores repeat triggers from the same sender within this window |
| `MATRIX_HOMESERVER` | optional | Defaults to `https://matrix.org` |

Startup fails fast with a clear error if a required variable is missing or a room ID is malformed, rather than silently misbehaving at runtime.

## Key Export
Only tested using Element Web.
1. In Element Web go to Settings>Encryption
2. Under Advanced export recovery keys with a secure password that should be entered into the KEY_EXPORT_PASSPHRASE variable
3. Rename the downloaded element-keys.txt to element-keys-shareable.txt and place it into the root bot directory

## Running it

### With Docker

```bash
docker build -t matrix-welcome-bot .
docker run -d \
  --env-file .env \
  -v $(pwd)/data:/data \
  -e SESSION_FILE=/data/session.json \
  -e CRYPTO_STORE_PATH=/data/bot-store \
  matrix-welcome-bot
```

### With Docker Compose / Portainer

See [`docker-compose.yml`](./docker-compose.yml). The image is built automatically by GitHub Actions on every push (see [`.github/workflows/docker-build.yml`](./.github/workflows/docker-build.yml)) and published to GHCR, so Portainer only ever needs to *pull* it — no build step required on the deployment host.

To deploy: fill in your own values in a copy of `.env.example`, upload `docker-compose.yml` as a stack in Portainer, paste in the env vars, and deploy.

### Persistent data

The bot writes several files that need to survive restarts — point these at a persistent volume/dataset, not the container's writable layer:

- `session.json` — the bot's login session. **Treat this like a password.** Anyone with this file can act as the bot.
- `bot-store/` — the SQLite crypto store (Olm/Megolm sessions, device state).
- Optional key export/import/diagnostic files, if used.

On restart, the bot restores the existing session rather than logging in fresh — this matters because the crypto store is tied to a specific device ID, and logging in fresh every time would mint a new device that doesn't match the existing store, breaking encryption state.

## Security notes

- **`BOT_RECOVERY_KEY` is a master key, not a room-scoped one.** It unlocks Secure Secret Storage — cross-signing keys and key backup — covering every encrypted room the account has ever seen, not just the configured target rooms. Treat it accordingly, and rotate it if the host is ever compromised.
- **`TRIGGER_ROOMS` is a hard allow-list.** Without it, any room the bot happens to be a member of would accept the trigger phrase. Keep this list tight — anyone who can message the bot in an allow-listed room can self-serve an invite (and full historical decryption keys) into every target room.
- **History sharing** Invitees get decryption keys for the room's *entire* history the bot holds keys for, not just messages sent after they join.
- Don't commit `.env`, `session.json`, or the crypto store to version control.

## License


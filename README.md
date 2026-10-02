# sms-discord-relay

A compact Rust HTTP server for [SMS Gateway for Android](https://github.com/capcom6/android-sms-gateway). It authenticates incoming webhooks, turns received text SMS into Discord webhook JSON, and optionally forwards it directly to Discord. No bot, database, MMS handling, or queue.

## Build and run

```sh
cargo build --locked --release
cp .env.example .env
chmod 600 .env
# Edit .env: copy Android's signing key and your Discord webhook URL.
# Only source a file you control; this is shell syntax, not a dotenv loader.
set -a; . ./.env; set +a
./target/release/sms-discord-relay
```

The binary reads environment variables, **not** `.env` automatically:

| Variable | Meaning |
| --- | --- |
| `SMS_GATEWAY_SIGNING_KEY` | Required nonempty text key from Android **Settings → Webhooks → Signing Key**. No insecure default; whitespace-only keys are rejected. Key bytes are used as entered, not hex/base64-decoded. |
| `DISCORD_WEBHOOK_URL` | Optional; omit entirely for transform-only use. If set, must be `https://discord.com/api/webhooks/<numeric-id>/<token>`, optionally `?thread_id=<numeric-id>`. No other query arguments, fragments, credentials, alternate hosts, or non-443 ports. Blank is invalid. The relay adds `wait=true`. |
| `BIND_ADDR` | Numeric socket address; defaults to `0.0.0.0:8080`. Use `127.0.0.1:8080` behind a same-host reverse proxy. |

`GET /healthz` returns `200 ok`. It is a liveness probe, not a Discord connectivity/readiness test. SIGINT/SIGTERM stop accepting connections and gracefully finish active requests.

### Container (Linux amd64 and arm64)

The multi-stage `Containerfile` uses a digest-pinned, multi-architecture Rust/Alpine builder to produce a native static musl binary, then runs it as UID/GID 65532 in **`scratch`**. The runtime image contains only `/sms-discord-relay`: no Alpine userspace, shell, package manager, OpenSSL, or CA-file package. Rustls uses embedded WebPKI trust roots. CI builds and tests on native amd64 and arm64 runners (no emulation). `RUST_IMAGE` can override the builder; update its default digest deliberately when upgrading the toolchain. Local builds default to your host architecture; cross-architecture builds need emulation:

```sh
podman build -f Containerfile -t sms-discord-relay:local .
podman run --rm --detach --name sms-discord-relay \
  --env-file .env -p 127.0.0.1:8080:8080 \
  --read-only --cap-drop=ALL --security-opt=no-new-privileges \
  sms-discord-relay:local
curl --fail http://127.0.0.1:8080/healthz
```

For a published release, substitute `ghcr.io/lucination/sms-discord-relay:v0.1.0` for the local image. Version tags `vX.Y.Z` publish version-tagged and `latest` GHCR images, plus GitHub Release artifacts: static Linux amd64 and arm64 binaries, per-architecture Docker-compatible image archives, and `SHA256SUMS`. These are release outputs, not a production deployment. Pin a version/digest instead of `latest` for unattended production use. The executable inside the container is `/sms-discord-relay` and the exposed port is 8080.

## Android setup

Expose this server through a **trusted HTTPS** reverse proxy or tunnel. This binary speaks plain HTTP on its listening socket; it does not terminate inbound TLS. Keep that socket private. Protect proxy access/error logs from recording SMS bodies, signing headers, or Discord credentials.

1. Find the signing key in Android **Settings → Webhooks → Signing Key** and set the matching `SMS_GATEWAY_SIGNING_KEY` on the relay. Set synchronized clocks on device and server.
2. Create a Discord channel webhook and set its URL in `DISCORD_WEBHOOK_URL`.
3. Register a webhook with the device's API. The JSON body is:

```json
{
  "url": "https://relay.example.com/webhook",
  "event": "sms:received"
}
```

Register `sms:batch:received` separately if exporting historical inbox messages; each registration supports one event. Optionally add `"device_id": "YOUR_DEVICE_ID"` for Cloud/Private mode.

Cloud example (curl prompts for the password, rather than putting it in shell history):

```sh
curl --user YOUR_GATEWAY_USERNAME \
  --header 'Content-Type: application/json' \
  --data '{"url":"https://relay.example.com/webhook","event":"sms:received"}' \
  https://api.sms-gate.app/3rdparty/v1/webhooks
```

Local mode uses the device's `/webhooks` endpoint; Private mode uses your server's `/3rdparty/v1/webhooks`. See the [authoritative webhook guide](https://docs.sms-gate.app/features/webhooks/) for registration and transport requirements. Signing keys are configured in the app, **not** included in registration JSON.

## HTTP contract

Both POST routes require:

- `X-Timestamp`: Unix seconds (unsigned decimal), within **±300 seconds** of the server's current clock.
- `X-Signature`: 64 hexadecimal characters containing HMAC-SHA256 over **raw body bytes followed immediately by the exact timestamp header text**. Signature bytes are compared in constant time. Missing, malformed, duplicate, stale, or invalid authentication headers produce `401`. Authentication precedes JSON parsing; oversized requests may instead produce `413` and overload/timeouts `503`.

Example input:

```json
{
  "event": "sms:received",
  "payload": {
    "messageId": "abc123",
    "sender": "+15555550123",
    "message": "Hello from Android",
    "recipient": null,
    "simNumber": 1,
    "receivedAt": "2024-06-22T15:46:11.000+07:00"
  }
}
```

`sender`, `message`, and `receivedAt` are required strings. Legacy `phoneNumber` is accepted as an alias for `sender`; supplying both is rejected as ambiguous. `recipient` and `simNumber` may be omitted or null. SIM numbers must be unsigned 32-bit integers. Envelope IDs, device IDs, webhook IDs, and message IDs are not required or retained. Extra fields are ignored. The received timestamp is displayed as supplied, not normalized.

### `POST /transform`

Single events return `200` with one Discord webhook payload:

```json
{
  "content": "From: +15555550123\nSIM: 1\nReceived: 2024-06-22T15:46:11.000+07:00\n\nHello from Android",
  "allowed_mentions": {"parse": []}
}
```

Batches use `{"event":"sms:batch:received","payload":{"messages":[...]}}` and return `200` with a **JSON array** of webhook payloads in input order. This array is **not** directly accepted by Discord: an external consumer must POST each element separately. Empty batches return `[]`. No forwarding occurs on this route, even when a Discord URL is configured. Registering this route with Android acknowledges transformation only; Android will not consume the response JSON to deliver it elsewhere.

### `POST /webhook`

Forwards single messages, or each batch message sequentially in input order. Returns `204` only after every outbound request receives a Discord 2xx status with `wait=true`. Discord error/rate-limit responses, redirects, connection errors, and client timeouts return `502`; missing forwarding configuration, overload, and total request timeout return `503`. Upstream response bodies are discarded and are never reflected to the sender. There are no application-level retries in the relay.

For both routes, authenticated other events (including data SMS, MMS, outbound status events, and ping) are safely ignored with `204`. Malformed JSON/supported payloads return `400`. Limits are **256 KiB raw body**, **100 messages per batch**, **32 active requests**, **3s connect timeout**, **8s per Discord request**, and **25s total request budget** (including incoming body read and the entire batch). Overflow returns `413`, not a partial batch. Work is not queued when capacity is exhausted. Proxy/header/connection limits should also be configured; the application semaphore limits active handlers, not all TCP connections.

Sender, recipient, receive time, and optional SIM labels precede the SMS text. Each potentially large metadata field is capped at 160 UTF-16 units to leave room for the message. Final content, including labels and truncation marker, is capped at **2000 UTF-16 units**, without splitting a Unicode scalar. Truncation uses `…`. `allowed_mentions.parse=[]` prevents user/role/everyone pings; text still uses Discord's normal Markdown rendering.

## Delivery and security limitations

- **At-least-once attempts, possible duplicates; no durable queue or deduplication.** Android retries failed deliveries (default exponential 10s, 20s, 40s, etc., up to 14 retries), but finite retries can eventually lose messages. A network timeout can happen after Discord accepted a message. A failed/timed-out batch can have already posted a prefix; Android retries the whole batch and may duplicate that prefix. Restart/crash does not preserve pending work. A 100-message batch may exhaust the deadline or hit Discord rate limits; prefer live single events for reliable low-volume relaying.
- The ±300s timestamp check narrows replay exposure but does **not** prevent replay within that window. There is no event-ID cache. Retries must carry a fresh valid signing timestamp; a replay of an old signed request is rejected after five minutes.
- One shared signing key authenticates the sender; it does not enforce device-ID or webhook-ID allowlists. Use a dedicated registration/key/server if you need isolation.
- Incoming TLS is your proxy's responsibility. Outgoing requests are HTTPS-only to the validated Discord host, use rustls, reject redirects, disable environment proxies and automatic retries. Local HTTP mock targets exist only in compiled test code; no environment bypass exists.
- Only static startup failure messages go to stderr; there are no request, URL, token, key, or SMS logs. HTTP failure responses are empty or framework-generated generic body-limit errors, without incoming/upstream contents. Review any added proxy/observability configuration separately.
- Discord receives private SMS content, potentially including OTPs. Restrict channel access, secure env-file permissions and host access, and rotate both credentials when needed. Never commit `.env`.

## Verification

```sh
cargo test --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
```

Tests use only local ephemeral HTTP listeners (no real Discord calls). They cover pure transformation/signatures, current and legacy fields, UTF-16 truncation, real executable startup, authentication rejection, bounded forwarding, failures and rate limits, redirect rejection, partial batches, overload, and shared deadlines. `TDD.md` records representative RED→GREEN evidence. Release uses size optimization, LTO, one codegen unit, stripped symbols, and abort-on-panic.

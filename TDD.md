# Vertical TDD evidence

Behavior slices were implemented one at a time: test → run RED → minimal implementation → full suite GREEN. Initial function/HTTP stubs failed behavior assertions (not unresolved-symbol compiler errors). Representative captured outputs:

| Slice | RED observation | GREEN |
| --- | --- | --- |
| Current SMS transformation | `current_sms_becomes_safe_discord_payload ... FAILED`, `unwrap()` on `Err(Invalid)` | 1 test passed |
| Legacy batch | `legacy_batch_preserves_recipient ... FAILED`, `Err(Invalid)` | 2 passed |
| Ignore unrelated events | assertion `matches!(..., Ok(Delivery::Ignored))` failed | 3 passed |
| 100-message bound | expected error, got `Ok(Batch(...))` for 101 messages | 4 passed |
| UTF-16 truncation | assertion `encode_utf16().count() <= 2000` failed | 5 passed |
| Raw-body HMAC | `left: Err(AuthError), right: Ok(())` | 6 passed |
| Key/timestamp validation | `SigningKey::new("").is_err()` failed | 7 passed |
| HTTPS Discord target | valid URL rejected by initial stub | 8 passed |
| Authenticated HTTP transformation | `left: 404, right: 200` | 9 passed |
| Local HTTP Discord forwarding | `left: 503, right: 204` | 10 passed |
| Raw body bound | `left: 400, right: 413` | 11 passed |
| Shared batch deadline | `left: 204, right: 503` (five 100ms sends exceeded 150ms total test budget) | 12 passed |
| Overload rejection | `left: 204, right: 503` | 13 passed |
| Invalid executable configuration | assertion `!output.status.success()` failed | startup rejection passed |
| Executable listener + SIGTERM | `server exited before listening` | executable integration passed |

Additional regression tests cover existing failure paths, missing/duplicate/bad signatures, no requests on auth failure, malformed input, array shape, connection failure, no redirects/retries, upstream privacy, partial-batch stopping, and client timeouts. Short test-only timeout/concurrency settings make HTTP bounds verifiable without 25-second tests.

Final verification commands:

```sh
cargo test --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
```

Final result: 20 library tests + 2 executable integration tests passed; fmt and warning-denying clippy passed. The stripped local GNU release binary is 2,299,104 bytes. A separate release executable smoke verified healthz=200, signed transform=200, unsigned webhook=401, SIGTERM exit=0 and empty logs.

All server/client mocks are localhost; no external Discord request was made. Container image verification is a separate parent-agent step; local GNU release build is verified here.

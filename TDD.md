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

Original 0.1.0 verification: 20 library tests + 2 executable integration tests passed; fmt and warning-denying clippy passed. The stripped local GNU release binary is 2,299,104 bytes. A separate release executable smoke verified healthz=200, signed transform=200, unsigned webhook=401, SIGTERM exit=0 and empty logs.

All server/client mocks are localhost; no external Discord request was made. Container image verification is a separate parent-agent step; local GNU release build is verified here.

## 0.1.1 tracing and Python visual-format parity

The interrupted implementation's live transcript was inspected rather than restarted: `/home/hal/.hermes/cache/delegation/live/deleg_ef433380/task-0.log`.

| Slice | Observed RED | Observed GREEN |
| --- | --- | --- |
| Startup/shutdown stderr events | Prior transcript: executable startup integration FAILED before tracing initialization | Prior transcript: startup test passed |
| Real LogTracer normalization + hard target allowlist | Prior transcript: `app_allowlist_intersects_env_filter_for_native_and_bridged_events ... FAILED` | Passed after AppTargets layer; retained real `log` bridge tests |
| Safe request spans/outcomes and debug-only health probes | Prior transcript: `executable_logs_safe_request_outcomes_without_health_info_spam ... FAILED` | Passed after bounded middleware instrumentation |
| Structured static startup failure | Prior transcript: configuration-rejection test FAILED before event/reason fields | Startup tests passed |
| Resume existing work | Parent observed E0599: `.with_current_subscriber()` missing `WithSubscriber` import | Minimal test-only import fix: full existing suite passed (20 library + 4 integration tests) |
| Forward status/count/reason events | `forwarding_logs_safe_counts_and_upstream_status ... FAILED`: missing `event="forward"` | Passed with accepted/status failure events; real local HTTP upstream, no raw body/URL/error |
| Transport failure event | `forwarding_logs_transport_failure_without_raw_error ... FAILED`: missing transport reason | Passed with static transport enum reason and completed count |
| Python normal-message snapshot | `current_sms_becomes_safe_discord_payload ... FAILED`: old `From:/SIM:/Received:` layout versus exact emoji/bold/footer snapshot | Exact snapshot passed |
| Python missing/null/empty defaults | `python_format_defaults_and_optional_footer ... FAILED`: `Err(Invalid)` for missing fields | Passed; explicit null message remains invalid |

Formatter reference: [pinned Python app.py](https://github.com/lucination/sms-discord-relay-py/blob/fdc4956a542d88d69de10f71086acc3e83ae2ee1/app.py). Public raw fetch returned 404; authenticated `gh api` retrieval succeeded. The user-supplied exact formatter statements were executed in an isolated Python AST module (no Flask/requests imports or forwarding): normal, missing-fields, null sender/timestamp, and legacy-equivalent sender snapshots produced the expected strings. A subsequent attempt to execute AST-extracted statements directly from the authenticated source was blocked pending security approval, so no claim is made that this second oracle ran.

Retained safety differences: mandatory HMAC authentication, no sensitive logs, disabled mentions, 2000 UTF-16-unit whole-scalar truncation with ellipsis, typed non-string rejection. Sender/receive-time defaults now match Python; recipient/SIM labels are not rendered. Existing malformed-input tests were changed from empty payload (now valid) to explicit null/non-string message. Original long-metadata truncation test now verifies the whole formatted content boundary.

Final local verification: `cargo test --locked` passed **23 library + 4 integration tests**; `cargo fmt --all -- --check`, `cargo clippy --locked --all-targets -- -D warnings`, and `cargo build --locked --release` all passed. Package and lockfile version are 0.1.1. No commit, push, release publication, container edit, or deployment was performed by this continuation.

### Current gateway dual-field correction

The user reported that current phone deliveries contain both `sender` and `phoneNumber`. `accepts_current_delivery_with_both_sender_fields` reproduced RED (`Err(Invalid)`) with a full envelope, then passed after replacing the alias with separate optional fields and preferring `sender`. Null current sender falls back to legacy `phoneNumber`; signed HTTP forwarding/batch and executable fixtures now contain both fields.

A separate RED test (`rejects_missing_sender_fields_in_single_and_batch`) observed accepted sender-less payloads before implementing the user-requested `400` when both sender fields are absent/null. This intentionally supersedes the earlier missing/null-sender defaults above; a present empty string still renders `unknown sender`. Invalid batch messages are rejected before any forwarding.

Final corrected local gates: 25 library + 4 integration tests passed; fmt, all-target clippy, and release build passed.

## 0.1.2 existing-binary healthcheck

These vertical slices were written and exercised separately, each reaching GREEN before the next behavior was introduced. RED observations below are actual local `cargo test --locked` output, not hypothetical failures.

| Slice | Observed RED | Observed GREEN |
| --- | --- | --- |
| Actual executable healthcheck without credentials/logging | `healthcheck_succeeds_without_credentials_or_logging ... FAILED`: stderr contained `startup_failure` / missing signing key | Passed after dispatching the probe before configuration and logging; exactly `GET /healthz`, exit 0, empty streams, invalid Discord URL ignored |
| Only HTTP 200 succeeds | `only_http_200_is_healthy ... FAILED`: 204 returned `Some(0)` instead of `Some(1)` | Passed with exact `StatusCode::OK`; local 204 and 500 both fail |
| Do not follow redirects | `redirects_are_not_followed ... FAILED`: 302 followed to 200 and exited `Some(0)` | Passed with redirects disabled; listener records only one request |
| Ignore environment proxies | `environment_proxy_cannot_report_false_health ... FAILED`: local proxy's 200 masked the closed target, exit `Some(0)` | Passed with `.no_proxy()`; exit 1, no proxy connection |
| Bounded stalled response | `stalled_headers_fail_within_two_second_budget ... FAILED`: elapsed `2.60867299s`, exceeding 2.4s assertion | Passed with 1s connect / 2s total timeout; stalled response takes 1.8–2.4s including executable startup, under container's 3s timeout |
| Numeric socket addresses only | `hostnames_are_rejected_before_network_access ... FAILED`: `localhost` accepted, exit `Some(0)` | Passed after parsing `SocketAddr`; rejected before any request |
| Wildcard address mapping | `wildcard_addresses_use_matching_loopback_family_and_preserve_port ... FAILED`: `0.0.0.0:8080` instead of `127.0.0.1:8080` | Passed for IPv4/IPv6 wildcards and explicit-address/port preservation |
| Reject port zero | `invalid_addresses_and_zero_ports_are_rejected ... FAILED`: `accepted 127.0.0.1:0` | Passed for IPv4/IPv6 zero ports and malformed addresses |
| Default bind address | `absent_bind_defaults_to_ipv4_loopback_port_8080 ... FAILED`: unwrap of `Err(())` | Passed with absent `BIND_ADDR` defaulting to wildcard IPv4 port 8080, then mapped to loopback |
| Reject unsupported CLI arguments quietly | `unsupported_arguments_fail_quietly_without_starting_server ... FAILED`: stderr startup/signing-key failure | Passed: unknown, extra, duplicate, and unsafe HTTP flag all exit 1 before logging/configuration |

Additional regression coverage (these already passed on their first run, and are not claimed as RED slices): a closed listener, dropped connection with exactly one attempt, malformed/non-Unicode BIND_ADDR, non-Unicode credential environment ignored by a successful probe, and actual IPv4/IPv6 wildcard/exact local connections. Reqwest's no-retry policy is set explicitly rather than depending on its current defaults. The private binary-only healthcheck module separates pure address construction from HTTP/environment effects; existing library/server/logging code is untouched. No dependency was added.

Final local verification on 0.1.2:

- `cargo test --locked`: **44 tests passed** — all original **25 library + 4 executable integration** tests, plus **3 address unit + 12 healthcheck executable integration** tests.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --locked --all-targets -- -D warnings`: passed. Initial clippy caught unchecked read counts in test servers and test-module ordering; those were corrected before the final run.
- `cargo build --locked --release`: passed.
- Actual optimized release executable smoke: local HTTP 200 → exit 0, HTTP 204 → exit 1, IPv4 wildcard → loopback success, IPv6 wildcard → loopback success; each sent exactly one `GET /healthz`, with stdout/stderr empty under `RUST_LOG=trace` and invalid Discord URL.

Cargo package and lockfile are 0.1.2; dependency versions are unchanged. README documents the binary mode, exec healthcheck timing, Podman manual checks/inspection and Docker-format image builds, liveness vs readiness, no implicit unhealthy restart, and shared major/minor/exact/latest multiarch release aliases. No commit, push, deployment, external Discord request, Containerfile edit, or CI edit was performed by this subagent; container/CI verification belongs to the parent agent.

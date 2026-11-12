# Dependency refresh for v0.1.4

Registry/release checks: 2026-11-12. The preceding public release and newest
remote tag are v0.1.3. This patch prepares release artifacts; it does not deploy
the relay or change its HTTP/authentication/formatting contract.

## Direct dependency floors

These are explicit compatible minimum versions, not exact pins. `Cargo.lock`
records the reproducible resolution. Versions were checked against crates.io's
latest non-prerelease stable versions, including incompatible upgrades.

| Crate | Minimum |
| --- | --- |
| axum | 0.8.9 |
| hex | 0.4.3 |
| hmac | 0.13.0 |
| sha2 | 0.11.1 |
| serde | 1.0.229 |
| serde_json | 1.0.154 |
| reqwest | 0.13.5 |
| tokio | 1.54.1 |
| tracing | 0.1.44 |
| tracing-log | 0.2.0 |
| tracing-subscriber | 0.3.23 |
| webpki-root-certs | 1.0.12 |
| log (dev) | 0.4.35 |

## Compatibility adaptations

- HMAC 0.13/SHA-2 0.11 require importing `KeyInit` for `new_from_slice`.
  Exact raw-body-then-timestamp signing, freshness, and constant-time MAC
  verification are unchanged.
- Reqwest 0.13 replaces the old WebPKI-root feature with rustls/platform trust.
  Explicit `tls_certs_only` with compiled-in Mozilla/WebPKI X.509 roots preserves
  the scratch image's embedded-only trust policy. It neither loads nor merges
  platform/environment CA files. `webpki-root-certs` supplies full DER certificates
  for the public reqwest API, avoiding a brittle preconfigured-rustls downcast.
- The HTTP-only executable healthcheck explicitly uses an empty root set rather
  than attempting to initialize platform trust. It remains credential-free,
  quiet, bounded, proxy-free, redirect-free, and retry-free.
- Reqwest's rustls feature uses AWS-LC; CMake is a builder-only prerequisite.
  No build tool, platform CA file, OpenSSL runtime, or userspace is copied into
  the final scratch image. UID/GID 65532 and the embedded exec healthcheck remain.

## Build and CI dependencies

- Stable Rust 1.99.1 for CI; native multiarch `rust:alpine` index pinned to
  `sha256:44819b27be4c97b0e4d7203bd2e02c4820f97700211b6a37fb4c246a2de59383`.
- GitHub action release tags were resolved to commit SHAs through the GitHub
  releases and Git ref APIs: checkout v7.0.1, setup-buildx v4.4.1,
  upload-artifact v7.0.2, download-artifact v8.0.2. These use Node 24 on current
  GitHub-hosted Ubuntu runners. Artifact archiving and digest checking are
  explicit. SHA pinning is retained.
- GHCR-only publication, native amd64/arm64 builds, Docker platform media types,
  and Docker-format downloadable archives remain unchanged. Publication and
  native dual-architecture CI are release gates, not claims of this local check.

## Upstream constraints

All 184 distinct crates in the lockfile were checked against crates.io (187
registry package/version entries). The older versions below cannot be replaced
with the latest stable version while honoring the latest direct dependencies'
published requirements. No fork, Git dependency, `[patch]`, or fabricated version
override is used. Newer versions can coexist with required older copies;
lockfile presence also includes optional and non-Linux target dependencies.

| Locked crate | Latest stable | Requiring upstream constraint |
| --- | --- | --- |
| getrandom 0.2.17 | 0.4.3 | ring 0.17.14: `^0.2.10` |
| matchit 0.8.4 | 0.9.2 | axum 0.8.9: **`=0.8.4`** (even 0.8.6 is excluded) |
| quinn-udp 0.5.16 | 0.6.3 | quinn 0.11.12: `^0.5` |
| r-efi 6.0.0 | 7.1.0 | getrandom 0.4.3: `^6` |
| syn 2.0.119 | 3.0.10 | tracing-attributes 0.1.31 / jni-macros 0.22.4: `^2.0`; jni-sys-macros 0.4.1: `^2` |
| tower-http 0.6.11 | 0.7.1 | reqwest 0.13.5: `^0.6.8` |
| wasi 0.11.1+wasi-snapshot-preview1 | 0.14.7+wasi-0.2.4 | getrandom 0.2.17: `^0.11`; mio 1.2.4: `^0.11.0` |
| windows-link 0.2.1 | 0.100.0 | jni 0.22.4: `^0.2`; windows-sys 0.61.2: `^0.2.1` |
| windows-sys 0.52.0 | 0.61.2 | ring 0.17.14: `^0.52` |
| windows-targets 0.52.6 | 0.53.5 | windows-sys 0.52.0: `^0.52.0` |
| windows_aarch64_gnullvm 0.52.6 | 0.53.1 | windows-targets 0.52.6: `^0.52.6` |
| windows_aarch64_msvc 0.52.6 | 0.53.1 | windows-targets 0.52.6: `^0.52.6` |
| windows_i686_gnu 0.52.6 | 0.53.1 | windows-targets 0.52.6: `^0.52.6` |
| windows_i686_gnullvm 0.52.6 | 0.53.1 | windows-targets 0.52.6: `^0.52.6` |
| windows_i686_msvc 0.52.6 | 0.53.1 | windows-targets 0.52.6: `^0.52.6` |
| windows_x86_64_gnu 0.52.6 | 0.53.1 | windows-targets 0.52.6: `^0.52.6` |
| windows_x86_64_gnullvm 0.52.6 | 0.53.1 | windows-targets 0.52.6: `^0.52.6` |
| windows_x86_64_msvc 0.52.6 | 0.53.1 | windows-targets 0.52.6: `^0.52.6` |

The registry version and dependency API snapshots, complete inventory, and audit
output are retained with the review evidence. These are upstream compatibility
constraints, not suppressed security findings.

`cargo-audit` 0.23.0 reported **zero vulnerabilities and zero warnings**, with no
ignored advisories, against RustSec advisory database commit
`af7b3ec33460b8e2de8d422e16d628518891a519` (1,490 advisories).

## Verification

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo audit
docker build --network=host -f Containerfile -t sms-discord-relay:review .
python3 tests/container_tls_roots.py sms-discord-relay:review
```

The final command is an additional Linux Docker/OpenSSL regression test; it
requires an unused, bindable loopback port 443. It maps `discord.com` to loopback
inside the test container and injects a private test CA through `SSL_CERT_FILE`.
The relay must reject that CA (502, no local HTTP POST). No real Discord endpoint
is contacted. Before the trust adaptation, the migrated reqwest client accepted
the platform-only CA and returned 204: an assertion-level RED, not merely a
dependency compile failure. All existing Rust tests remain in place.

Native arm64 execution, release publication, and published-image/archive
verification are performed separately by the release workflow and parent review.

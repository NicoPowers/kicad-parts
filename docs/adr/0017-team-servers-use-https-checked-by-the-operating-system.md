# Team servers use HTTPS, checked by the operating system

The user hosts a team server for each client on the internet, so the app must reach them over HTTPS. Signing in sends a password, and every request after it sends a session token. Decided 2026-10-18 (kicad-parts-5ac5).

The REST client and the realtime client share one TLS setup, `ServerTrust` in `crates/collaboration/src/tls.rs`. It uses rustls with the ring crypto provider, and rustls-platform-verifier checks each certificate with the operating system: Windows' certificate store, macOS' keychain, or Linux's CA bundle. A company's own root certificate therefore works once its IT has installed it. The app keeps no list of trusted authorities of its own.

The REST client keeps ureq 2, without its TLS feature. It gets TLS through ureq's `TlsConnector` trait, implemented by `ServerTrust`. The realtime client keeps its own socket, so it can still drop the connection on demand, and wraps that socket in the same TLS setup.

Plain `http://` stays for a server on this computer only, such as a local test server: there the password never leaves the computer. Every other address must use `https://` (see `crates/collaboration/src/address.rs`). `PocketBaseClient::authenticate` checks this too, so no caller can skip it. An address may end in a path, such as `https://parts.example.com/acme`, so several team servers can share one proxy.

## Considered options

- **Move to ureq 3.** It is the maintained line. Its stable TLS options take a list of roots, or the platform verifier with no way to add a test root, so tests would check certificates differently from the app. `Agent::with_parts` takes a custom `Connector`, which could carry `ServerTrust`, but that API is "unversioned": it may change in any release, outside semver. Rejected for now.
- **ureq 2 with its own `tls` feature.** Rejected, because it builds in Mozilla's list of authorities and ties our rustls version to ureq's.
- **native-tls.** Rejected, because it uses OpenSSL on Linux, a system library that differs from machine to machine.
- **aws-lc-rs instead of ring.** It is rustls' default and builds on all three platforms. We keep ring because it is smaller and quicker to build, and rcgen in the tests uses it too, so the tree has one crypto provider. Revisit if we need FIPS or post-quantum key exchange.

## Consequences

- ureq 2 gets no new releases. Moving to ureq 3, or to another HTTP client, changes only `client.rs`: `ServerTrust` and its tests stay. Revisit when ureq 3 makes its `Connector` API stable, or can take a rustls configuration or a custom verifier.
- Certificate problems, timeouts and unreachable servers each have one plain message, in `ConnectionProblem` (`crates/collaboration/src/connection.rs`).
- An `https://` client refuses redirects to `http://`, so a misconfigured server can't make it send the password unencrypted.
- Tests never change the operating system's certificate store. The `test-trust` feature adds a certificate authority made at test time (`crates/collaboration/tests/https.rs`). The app never turns it on.
- A reverse proxy in front of PocketBase must pass `/api/realtime` through without buffering, or live updates arrive late.
- A live-updates stream silent for 6 minutes counts as gone, and the app reconnects and catches up. PocketBase closes an idle stream after 5 minutes, so a longer silence means the network went away.

Mulch records: mx-b959cf (supersedes mx-8ffc3d), mx-5a94dc, mx-a42231.

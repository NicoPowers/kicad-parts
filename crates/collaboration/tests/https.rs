//! HTTPS to a team server, without the internet and without touching the
//! operating system's certificate store.
//!
//! Each test makes its own certificate authority with rcgen and trusts it
//! through `ServerTrust::system_and_test_root` (the `test-trust` feature).
//! A small TLS front on 127.0.0.1 forwards to a real, disposable
//! PocketBase, so REST and realtime run end to end over HTTPS, checked by
//! the operating system's verifier, as in the app.

mod support;

use collaboration::{
    CollabError, ConnectionProblem, EditRecord, PocketBaseClient, RealtimeConnection,
    ServerAddress, ServerTrust,
};
use native_services::Superuser;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, date_time_ymd,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use support::{all_migrations, create_member};

const WAIT: Duration = Duration::from_secs(15);

/// A test certificate authority, made fresh for each test.
struct TestAuthority {
    issuer: Issuer<'static, KeyPair>,
    der: CertificateDer<'static>,
}

impl TestAuthority {
    fn new(name: &str) -> Self {
        let mut params = CertificateParams::new(Vec::new()).unwrap();
        params.distinguished_name.push(DnType::CommonName, name);
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        params.not_before = date_time_ymd(2020, 1, 1);
        params.not_after = date_time_ymd(2100, 1, 1);
        let key = KeyPair::generate().unwrap();
        let der = params.self_signed(&key).unwrap().der().clone();
        Self {
            issuer: Issuer::new(params, key),
            der,
        }
    }

    /// Trusts what this computer trusts, plus this authority.
    fn trust(&self) -> ServerTrust {
        ServerTrust::system_and_test_root(self.der.to_vec())
    }

    /// A server certificate for `name`, valid from `from` to `until`
    /// (years, January 1st).
    fn certificate(&self, name: &str, from: i32, until: i32) -> ServerCertificate {
        let mut params = CertificateParams::new(vec![name.to_owned()]).unwrap();
        params.distinguished_name.push(DnType::CommonName, name);
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.not_before = date_time_ymd(from, 1, 1);
        params.not_after = date_time_ymd(until, 1, 1);
        let key = KeyPair::generate().unwrap();
        let certificate = params.signed_by(&key, &self.issuer).unwrap();
        ServerCertificate {
            chain: vec![certificate.der().clone()],
            key: PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
        }
    }
}

struct ServerCertificate {
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

/// A TLS front on 127.0.0.1 that forwards each connection to `upstream`,
/// as a reverse proxy in front of a hosted PocketBase would.
struct TlsFront {
    port: u16,
    stop: Arc<AtomicBool>,
}

impl TlsFront {
    fn start(certificate: ServerCertificate, upstream: &str) -> Self {
        Self::start_under(certificate, upstream, "")
    }

    /// Like `start`, but serves PocketBase under `prefix`, such as
    /// `/acme`, as a proxy for several team servers on one name would:
    /// `/acme/api/...` goes to `/api/...`, and `/api/...` goes nowhere.
    fn start_under(certificate: ServerCertificate, upstream: &str, prefix: &str) -> Self {
        let config =
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(certificate.chain, certificate.key)
                .unwrap();
        let config = Arc::new(config);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let upstream = upstream.to_owned();
        let prefix = prefix.to_owned();
        let stopping = stop.clone();
        std::thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((client, _)) => {
                        let (config, upstream, prefix, stopping) = (
                            config.clone(),
                            upstream.clone(),
                            prefix.clone(),
                            stopping.clone(),
                        );
                        std::thread::spawn(move || {
                            let _ = forward(client, config, &upstream, &prefix, &stopping);
                        });
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        });
        Self { port, stop }
    }

    /// The address a person would type: a name, so the certificate's
    /// name is checked.
    fn url(&self) -> String {
        format!("https://localhost:{}", self.port)
    }
}

impl Drop for TlsFront {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Copies bytes both ways between a TLS client and `upstream` until either
/// side closes, taking `prefix` out of request lines. One thread polls
/// both with short timeouts, because a rustls stream can't be split
/// between two threads.
fn forward(
    client: TcpStream,
    config: Arc<ServerConfig>,
    upstream: &str,
    prefix: &str,
    stop: &AtomicBool,
) -> io::Result<()> {
    client.set_nonblocking(false)?;
    let mut tls = StreamOwned::new(ServerConnection::new(config).unwrap(), client);
    // The handshake comes first, so a client that refuses the certificate
    // gets to say so, whatever is upstream.
    while tls.conn.is_handshaking() {
        tls.conn.complete_io(&mut tls.sock)?;
    }
    let poll = Some(Duration::from_millis(5));
    tls.sock.set_read_timeout(poll)?;
    let mut server = TcpStream::connect(upstream)?;
    server.set_read_timeout(poll)?;
    let mut buffer = [0u8; 16 * 1024];
    while !stop.load(Ordering::SeqCst) {
        match tls.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => server.write_all(&without_prefix(&buffer[..count], prefix))?,
            Err(error) if is_timeout(&error) => {}
            Err(error) => return Err(error),
        }
        match server.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                tls.write_all(&buffer[..count])?;
                tls.flush()?;
            }
            Err(error) if is_timeout(&error) => {}
            Err(error) => return Err(error),
        }
    }
    let _ = server.shutdown(Shutdown::Both);
    let _ = tls.sock.shutdown(Shutdown::Both);
    Ok(())
}

/// `request` with `GET {prefix}/api/` made `GET /api/`, and a request for
/// `/api/` without the prefix sent to a path PocketBase doesn't have. The
/// clients write each request head in one piece, so a request line never
/// spans two reads here.
fn without_prefix(request: &[u8], prefix: &str) -> Vec<u8> {
    if prefix.is_empty() {
        return request.to_vec();
    }
    let unprefixed = replace(request, b" /api/", b" /not-under-the-prefix/api/");
    replace(&unprefixed, format!(" {prefix}/api/").as_bytes(), b" /api/")
}

fn replace(haystack: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(haystack.len());
    let mut rest = haystack;
    while let Some(at) = rest.windows(from.len()).position(|window| window == from) {
        out.extend_from_slice(&rest[..at]);
        out.extend_from_slice(to);
        rest = &rest[at + from.len()..];
    }
    out.extend_from_slice(rest);
    out
}

fn is_timeout(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

/// The address of a port where nothing listens: the failing tests never
/// get past the handshake, so they never reach it.
fn nowhere() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().to_string()
}

/// A plain HTTP server on 127.0.0.1 that answers every request with
/// `response`, then closes the connection. Returns its address.
fn answering(response: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for mut connection in listener.incoming().flatten() {
            // Read the request head, then answer.
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && connection.read(&mut byte).unwrap_or(0) == 1 {
                head.push(byte[0]);
            }
            let _ = connection.write_all(response.as_bytes());
            let _ = connection.shutdown(Shutdown::Both);
        }
    });
    address
}

/// Asserts that REST and realtime both fail against `front` with a
/// problem `is_expected` accepts, and that the message is plain words.
fn assert_refused(
    front: &TlsFront,
    trust: &ServerTrust,
    is_expected: impl Fn(&ConnectionProblem) -> bool,
) {
    let client = PocketBaseClient::with_trust(&front.url(), trust.clone());
    for (what, failure) in [
        ("REST", client.get("/api/health").err()),
        ("realtime", RealtimeConnection::connect(&client, WAIT).err()),
    ] {
        let Some(CollabError::Transport(problem)) = failure else {
            panic!("{what}: {failure:?}");
        };
        assert!(is_expected(&problem), "{what}: {problem:?}");
        let message = problem.to_string();
        assert!(
            message.starts_with("The team server's certificate"),
            "{message}"
        );
        assert!(!message.contains("rustls"), "{message}");
    }
}

/// Windows reports a certificate that is not valid yet as expired, and
/// macOS reports an expired one as some other certificate problem.
fn out_of_date(problem: &ConnectionProblem) -> bool {
    matches!(
        problem,
        ConnectionProblem::CertificateOutOfDate | ConnectionProblem::CertificateNotAccepted(_)
    )
}

#[test]
fn a_trusted_certificate_works_for_rest_and_realtime() {
    let temporary = tempfile::tempdir().unwrap();
    let migrations: Vec<String> = all_migrations();
    let migrations: Vec<&str> = migrations.iter().map(String::as_str).collect();
    let options = support::options(temporary.path(), &migrations);
    let (server, admin) = support::start(&options, &Superuser::generate());
    let alice_member = create_member(&admin, "alice");
    let bob_member = create_member(&admin, "bob");

    let authority = TestAuthority::new("kicad-parts test authority");
    let upstream = server.url().trim_start_matches("http://");
    let front = TlsFront::start(authority.certificate("localhost", 2020, 2100), upstream);
    let trust = authority.trust();
    let sign_in = |member: &support::Member| {
        PocketBaseClient::with_trust(&front.url(), trust.clone())
            .authenticate("users", &member.email, &member.password)
            .unwrap()
    };
    let alice = sign_in(&alice_member);
    let bob = sign_in(&bob_member);

    // Bob follows changes over HTTPS; Alice saves over HTTPS.
    let mut stream = RealtimeConnection::connect(&bob, WAIT).unwrap();
    stream.subscribe(&bob, &["collab_changes"]).unwrap();
    let reply = alice
        .execute(&EditRecord::new("res-1", 0, "Resistor", "10k 0603"))
        .unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
    let event = stream.next_on("collab_changes", WAIT).unwrap().unwrap();
    assert_eq!(event.data["record"]["record_key"], "res-1");

    // Disconnecting on demand still works over TLS, at once.
    let started = Instant::now();
    stream.disconnect();
    assert!(started.elapsed() < Duration::from_secs(5));

    // An https:// address for a server that speaks plain HTTP fails
    // plainly too.
    let plain_port = server.url().rsplit(':').next().unwrap();
    let wrong_scheme = format!("https://localhost:{plain_port}");
    let failure = PocketBaseClient::with_trust(&wrong_scheme, trust.clone())
        .get("/api/health")
        .unwrap_err();
    assert!(
        matches!(
            failure,
            CollabError::Transport(ConnectionProblem::NotSecure(_))
        ),
        "{failure:?}"
    );
    assert!(
        failure
            .to_string()
            .starts_with("Could not make a secure connection")
    );
    let client = PocketBaseClient::with_trust(&wrong_scheme, trust.clone());
    assert!(
        matches!(
            RealtimeConnection::connect(&client, WAIT).err(),
            Some(CollabError::Transport(ConnectionProblem::NotSecure(_)))
        ),
        "realtime"
    );
}

#[test]
fn a_server_under_a_path_works_for_rest_and_realtime() {
    let temporary = tempfile::tempdir().unwrap();
    let migrations: Vec<String> = all_migrations();
    let migrations: Vec<&str> = migrations.iter().map(String::as_str).collect();
    let options = support::options(temporary.path(), &migrations);
    let (server, admin) = support::start(&options, &Superuser::generate());
    let member = create_member(&admin, "carol");

    let authority = TestAuthority::new("kicad-parts test authority");
    let upstream = server.url().trim_start_matches("http://");
    let front = TlsFront::start_under(
        authority.certificate("localhost", 2020, 2100),
        upstream,
        "/acme",
    );
    // The address as typed at sign-in, with its path.
    let address = ServerAddress::for_sign_in(&format!("{}/acme/", front.url())).unwrap();
    let carol = PocketBaseClient::with_trust(&address.url(), authority.trust())
        .authenticate("users", &member.email, &member.password)
        .unwrap();
    let mut stream = RealtimeConnection::connect(&carol, WAIT).unwrap();
    stream.subscribe(&carol, &["collab_changes"]).unwrap();
    let reply = carol
        .execute(&EditRecord::new("cap-1", 0, "Capacitor", "100n 0402"))
        .unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
    let event = stream.next_on("collab_changes", WAIT).unwrap().unwrap();
    assert_eq!(event.data["record"]["record_key"], "cap-1");

    // Without the path, the front has no team server: the path matters.
    let missing = PocketBaseClient::with_trust(&front.url(), authority.trust())
        .get("/api/health")
        .unwrap()
        .status;
    assert_eq!(missing, 404);
}

#[test]
fn a_certificate_from_an_unknown_authority_is_not_trusted() {
    let trusted = TestAuthority::new("trusted test authority");
    let unknown = TestAuthority::new("unknown test authority");
    let front = TlsFront::start(unknown.certificate("localhost", 2020, 2100), &nowhere());
    assert_refused(&front, &trusted.trust(), |problem| {
        *problem == ConnectionProblem::CertificateNotTrusted
    });
}

#[test]
fn a_certificate_for_another_name_is_refused() {
    let authority = TestAuthority::new("test authority");
    let front = TlsFront::start(
        authority.certificate("team.example.com", 2020, 2100),
        &nowhere(),
    );
    assert_refused(&front, &authority.trust(), |problem| {
        *problem == ConnectionProblem::CertificateForAnotherName
    });
}

#[test]
fn an_expired_certificate_is_refused() {
    let authority = TestAuthority::new("test authority");
    let front = TlsFront::start(authority.certificate("localhost", 2020, 2021), &nowhere());
    assert_refused(&front, &authority.trust(), out_of_date);
}

#[test]
fn a_certificate_not_valid_yet_is_refused() {
    let authority = TestAuthority::new("test authority");
    let front = TlsFront::start(authority.certificate("localhost", 2090, 2100), &nowhere());
    assert_refused(&front, &authority.trust(), out_of_date);
}

#[test]
fn a_redirect_to_plain_http_is_refused() {
    let authority = TestAuthority::new("test authority");
    let plain = answering(String::new()); // Never reached.
    let redirect = format!(
        "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{plain}/api/health\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    let front = TlsFront::start(
        authority.certificate("localhost", 2020, 2100),
        &answering(redirect),
    );
    let failure = PocketBaseClient::with_trust(&front.url(), authority.trust())
        .get("/api/collections/users/records")
        .unwrap_err();
    assert_eq!(
        failure,
        CollabError::Transport(ConnectionProblem::RedirectToUnencrypted)
    );
    assert!(
        failure
            .to_string()
            .starts_with("The team server tried to switch to an unencrypted connection.")
    );
}

#[test]
fn an_error_answer_to_live_updates_is_said_plainly() {
    // As a proxy answers when PocketBase behind it is down.
    let proxy = answering(
        "HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
    );
    let client = PocketBaseClient::new(&format!("http://{proxy}"));
    let failure = RealtimeConnection::connect(&client, WAIT).err();
    let Some(CollabError::Transport(problem)) = failure else {
        panic!("{failure:?}");
    };
    assert_eq!(
        problem,
        ConnectionProblem::LiveUpdatesRefused("HTTP/1.1 502 Bad Gateway".into())
    );
    assert!(
        problem
            .to_string()
            .contains("The app tries again by itself.")
    );
}

#[test]
fn a_server_that_never_answers_times_out() {
    // Accepts connections and never says a word, not even a TLS hello.
    let silent = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("https://localhost:{}", silent.local_addr().unwrap().port());
    let started = Instant::now();
    let client = PocketBaseClient::with_trust(&url, TestAuthority::new("test authority").trust());
    let failure = RealtimeConnection::connect(&client, Duration::from_secs(1)).err();
    assert_eq!(
        failure,
        Some(CollabError::Transport(ConnectionProblem::TimedOut))
    );
    assert!(started.elapsed() < WAIT);
    drop(silent);
}

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
    CollabError, ConnectionProblem, EditRecord, PocketBaseClient, RealtimeConnection, ServerTrust,
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
        let stopping = stop.clone();
        std::thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((client, _)) => {
                        let (config, upstream, stopping) =
                            (config.clone(), upstream.clone(), stopping.clone());
                        std::thread::spawn(move || {
                            let _ = forward(client, config, &upstream, &stopping);
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
/// side closes. One thread polls both with short timeouts, because a
/// rustls stream can't be split between two threads.
fn forward(
    client: TcpStream,
    config: Arc<ServerConfig>,
    upstream: &str,
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
            Ok(count) => server.write_all(&buffer[..count])?,
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

/// Asserts that REST and realtime both fail with `expected` against
/// `front`, and that the message says what to do in plain words.
fn assert_refused(front: &TlsFront, trust: &ServerTrust, expected: ConnectionProblem) {
    let client = PocketBaseClient::with_trust(&front.url(), trust.clone());
    assert_eq!(
        client.get("/api/health").unwrap_err(),
        CollabError::Transport(expected.clone()),
        "REST"
    );
    assert_eq!(
        RealtimeConnection::connect(&client, WAIT).err(),
        Some(CollabError::Transport(expected.clone())),
        "realtime"
    );
    let message = CollabError::Transport(expected).to_string();
    assert!(
        message.starts_with("The team server's certificate"),
        "{message}"
    );
    assert!(!message.contains("rustls"), "{message}");
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
}

#[test]
fn a_certificate_from_an_unknown_authority_is_not_trusted() {
    let trusted = TestAuthority::new("trusted test authority");
    let unknown = TestAuthority::new("unknown test authority");
    let front = TlsFront::start(unknown.certificate("localhost", 2020, 2100), &nowhere());
    assert_refused(
        &front,
        &trusted.trust(),
        ConnectionProblem::CertificateNotTrusted,
    );
}

#[test]
fn a_certificate_for_another_name_is_refused() {
    let authority = TestAuthority::new("test authority");
    let front = TlsFront::start(
        authority.certificate("team.example.com", 2020, 2100),
        &nowhere(),
    );
    assert_refused(
        &front,
        &authority.trust(),
        ConnectionProblem::CertificateForAnotherName,
    );
}

#[test]
fn an_expired_certificate_is_refused() {
    let authority = TestAuthority::new("test authority");
    let front = TlsFront::start(authority.certificate("localhost", 2020, 2021), &nowhere());
    assert_refused(
        &front,
        &authority.trust(),
        ConnectionProblem::CertificateExpired,
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

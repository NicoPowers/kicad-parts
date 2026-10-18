//! HTTPS for the team server: one TLS setup for the REST client and the
//! realtime client (ADR 0017).
//!
//! The operating system decides which certificates to trust, through
//! rustls-platform-verifier: Windows' certificate store, macOS' keychain,
//! or Linux's CA bundle. So a company's own root certificate works once IT
//! has installed it, and the app keeps no list of its own.

use crate::connection::ConnectionProblem;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, StreamOwned};
use rustls_platform_verifier::Verifier;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, OnceLock};

/// Which certificates the app trusts when it talks to a team server.
/// Cloning is cheap: clones share one TLS setup.
#[derive(Clone)]
pub struct ServerTrust {
    /// An error if the operating system's certificates could not be read,
    /// kept so every HTTPS request can say why it failed.
    config: Result<Arc<ClientConfig>, ConnectionProblem>,
}

impl fmt::Debug for ServerTrust {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ServerTrust")
    }
}

impl ServerTrust {
    /// Trusts what the operating system trusts. Built once per process,
    /// because Linux reads its whole CA bundle to build it.
    pub fn system() -> Self {
        static SYSTEM: OnceLock<ServerTrust> = OnceLock::new();
        SYSTEM
            .get_or_init(|| Self::with_verifier(Verifier::new(crypto())))
            .clone()
    }

    /// Trusts what the operating system trusts, and also `root`, a test
    /// certificate authority in DER form. Tests use it so they never add
    /// anything to the operating system's store.
    #[cfg(feature = "test-trust")]
    pub fn system_and_test_root(root: Vec<u8>) -> Self {
        let root = rustls::pki_types::CertificateDer::from(root);
        Self::with_verifier(Verifier::new_with_extra_roots([root], crypto()))
    }

    fn with_verifier(verifier: Result<Verifier, rustls::Error>) -> Self {
        let config = verifier
            .and_then(|verifier| {
                Ok(ClientConfig::builder_with_provider(crypto())
                    .with_safe_default_protocol_versions()?
                    .dangerous() // Not dangerous: this is the platform's full check.
                    .with_custom_certificate_verifier(Arc::new(verifier))
                    .with_no_client_auth())
            })
            .map(Arc::new)
            .map_err(|error| ConnectionProblem::NoCertificateStore(error.to_string()));
        Self { config }
    }

    /// Starts TLS on `socket` for `host` (a name or an IP address, IPv6 in
    /// brackets or not) and finishes the handshake, so a bad certificate
    /// fails here, with its plain reason. The socket's timeouts apply.
    pub(crate) fn secure<S: Read + Write>(
        &self,
        host: &str,
        mut socket: S,
    ) -> Result<StreamOwned<ClientConnection, S>, ConnectionProblem> {
        let config = self.config.clone()?;
        let bare_host = host.trim_start_matches('[').trim_end_matches(']');
        let name = ServerName::try_from(bare_host)
            .map_err(|_| ConnectionProblem::NotReachable(format!("`{host}` is not a server name")))?
            .to_owned();
        let mut connection = ClientConnection::new(config, name)
            .map_err(|error| ConnectionProblem::from_tls(&error))?;
        while connection.is_handshaking() {
            connection
                .complete_io(&mut socket)
                .map_err(|io| ConnectionProblem::while_connecting(&io))?;
        }
        Ok(StreamOwned::new(connection, socket))
    }
}

/// ring, the one crypto provider in the dependency tree (ADR 0017).
fn crypto() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Lets `ureq` use the same TLS setup as the realtime client.
impl ureq::TlsConnector for ServerTrust {
    fn connect(
        &self,
        dns_name: &str,
        io: Box<dyn ureq::ReadWrite>,
    ) -> Result<Box<dyn ureq::ReadWrite>, ureq::Error> {
        match self.secure(dns_name, io) {
            Ok(stream) => Ok(Box::new(UreqTlsStream(stream))),
            // ConnectionProblem::from_ureq finds the problem in here again.
            Err(problem) => Err(io::Error::other(problem).into()),
        }
    }
}

/// A TLS stream that `ureq` can pool and reuse.
struct UreqTlsStream(StreamOwned<ClientConnection, Box<dyn ureq::ReadWrite>>);

impl ureq::ReadWrite for UreqTlsStream {
    fn socket(&self) -> Option<&TcpStream> {
        self.0.get_ref().socket()
    }
}

impl Read for UreqTlsStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.0.read(buffer)
    }
}

impl Write for UreqTlsStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl fmt::Debug for UreqTlsStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("UreqTlsStream")
    }
}

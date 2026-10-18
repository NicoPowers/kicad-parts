//! PocketBase realtime notifications over server-sent events.
//!
//! The connection is an HTTP/1.1 stream on its own socket, read by a
//! background thread, so [`RealtimeConnection::disconnect`] can drop it at
//! any moment, as a network failure would. An `https://` server gets TLS
//! from the same setup as the REST client ([`ServerTrust`]).

use crate::address::ServerAddress;
use crate::client::PocketBaseClient;
use crate::connection::ConnectionProblem;
use crate::{CollabError, CollabResult};
use rustls::{ClientConnection, StreamOwned};
use serde_json::Value;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long the stream may stay silent before the app counts it as gone.
/// PocketBase closes a stream that has had no events for 5 minutes, so a
/// longer silence means the network went away without a word, as after
/// sleep or a Wi-Fi change. The reader then stops, and the follower
/// reconnects and catches up.
const IDLE_LIMIT: Duration = Duration::from_secs(6 * 60);

/// One server-sent event.
#[derive(Clone, Debug, PartialEq)]
pub struct RealtimeEvent {
    /// The SSE event name: `PB_CONNECT` or the subscribed topic, such as a
    /// collection name.
    pub topic: String,
    pub data: Value,
}

enum Message {
    Event(RealtimeEvent),
    Closed(ConnectionProblem),
}

/// A plain or TLS stream, which the reader thread owns.
enum Stream {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Stream {
    /// The socket under the stream, where its read timeout is set.
    fn socket(&self) -> &TcpStream {
        match self {
            Self::Plain(socket) => socket,
            Self::Tls(stream) => stream.get_ref(),
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(socket) => socket.read(buffer),
            Self::Tls(stream) => stream.read(buffer),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(socket) => socket.write(buffer),
            Self::Tls(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(socket) => socket.flush(),
            Self::Tls(stream) => stream.flush(),
        }
    }
}

/// An open realtime stream.
pub struct RealtimeConnection {
    client_id: String,
    /// The TCP socket the stream's socket was cloned from, kept to shut
    /// it down from here while the reader thread is blocked reading.
    socket: TcpStream,
    events: Receiver<Message>,
    reader: Option<JoinHandle<()>>,
    closed: Option<ConnectionProblem>,
}

impl RealtimeConnection {
    /// Opens `GET {path}/api/realtime` on `client`'s server and waits for
    /// PocketBase's `PB_CONNECT` event, which carries this connection's
    /// client ID. An `https://` server's certificate is checked the way
    /// `client` checks it.
    pub fn connect(client: &PocketBaseClient, timeout: Duration) -> CollabResult<Self> {
        Self::connect_with_idle_limit(client, timeout, IDLE_LIMIT)
    }

    fn connect_with_idle_limit(
        client: &PocketBaseClient,
        timeout: Duration,
        idle_limit: Duration,
    ) -> CollabResult<Self> {
        let base_url = client.base_url();
        let trust = client.trust();
        let address = ServerAddress::from_url(base_url)
            .map_err(|_| CollabError::Protocol(format!("`{base_url}` is not a team server URL")))?;
        // The reader thread gets a clone of the socket; `close` shuts down
        // this one. On Windows, timeouts belong to each handle, not to the
        // connection, so the reader's are set on its own clone.
        let socket = open_socket(&address, timeout)?;
        let reader_socket = socket.try_clone().map_err(connecting)?;
        // The handshake and the request get `timeout`. The stream is quiet
        // between events, so its reads get `idle_limit` (set below).
        reader_socket
            .set_read_timeout(Some(timeout))
            .map_err(connecting)?;
        reader_socket
            .set_write_timeout(Some(timeout))
            .map_err(connecting)?;
        let mut stream = if address.is_secure() {
            Stream::Tls(Box::new(
                trust
                    .secure(address.host(), reader_socket)
                    .map_err(CollabError::Transport)?,
            ))
        } else {
            Stream::Plain(reader_socket)
        };
        write!(
            stream,
            "GET {}/api/realtime HTTP/1.1\r\nHost: {}\r\nAccept: text/event-stream\r\nCache-Control: no-cache\r\n\r\n",
            address.path(),
            address.authority()
        )
        .and_then(|()| stream.flush())
        .map_err(connecting)?;
        stream
            .socket()
            .set_read_timeout(Some(idle_limit))
            .map_err(connecting)?;
        let (sender, events) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let reason = match read_stream(stream, &sender) {
                Ok(()) => ConnectionProblem::Lost("the server closed the stream".into()),
                Err(io) => ConnectionProblem::after_connecting(&io),
            };
            let _ = sender.send(Message::Closed(reason));
        });
        let mut connection = Self {
            client_id: String::new(),
            socket,
            events,
            reader: Some(reader),
            closed: None,
        };
        let connect = connection
            .next_event(timeout)?
            .ok_or(CollabError::Transport(ConnectionProblem::TimedOut))?;
        if connect.topic != "PB_CONNECT" {
            return Err(CollabError::Protocol(format!(
                "expected PB_CONNECT, got `{}`",
                connect.topic
            )));
        }
        connection.client_id = connect.data["clientId"]
            .as_str()
            .ok_or_else(|| CollabError::Protocol("PB_CONNECT has no clientId".into()))?
            .to_owned();
        Ok(connection)
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// Sets this connection's subscriptions. PocketBase checks the
    /// collection's list/view rules against `client`'s auth for every
    /// event it would deliver.
    pub fn subscribe(&self, client: &PocketBaseClient, topics: &[&str]) -> CollabResult<()> {
        client
            .post_json(
                "/api/realtime",
                &serde_json::json!({ "clientId": self.client_id, "subscriptions": topics }),
            )?
            .success()
    }

    /// The next event, or `None` if none arrives within `timeout`. An error
    /// means the stream has ended.
    pub fn next_event(&mut self, timeout: Duration) -> CollabResult<Option<RealtimeEvent>> {
        if let Some(reason) = &self.closed {
            return Err(CollabError::Transport(reason.clone()));
        }
        match self.events.recv_timeout(timeout) {
            Ok(Message::Event(event)) => Ok(Some(event)),
            Ok(Message::Closed(reason)) => {
                self.closed = Some(reason.clone());
                Err(CollabError::Transport(reason))
            }
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(CollabError::Transport(
                ConnectionProblem::Lost("the realtime reader stopped".into()),
            )),
        }
    }

    /// Waits up to `timeout` for an event on `topic`, skipping others.
    pub fn next_on(
        &mut self,
        topic: &str,
        timeout: Duration,
    ) -> CollabResult<Option<RealtimeEvent>> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.next_event(remaining)? {
                Some(event) if event.topic == topic => return Ok(Some(event)),
                Some(_) => {}
                None => return Ok(None),
            }
        }
    }

    /// Drops the connection, like a network failure, and waits for the
    /// reader thread to stop. The server sees the stream end at once.
    ///
    /// On Linux and macOS the reader stops at once too. On Windows,
    /// shutting down a socket never wakes a read blocked on it, whichever
    /// handle is shut down. The read ends when the server closes its side
    /// in reply, which is quick, or, with the network gone, at the idle
    /// limit (6 minutes). Signing out does not wait for this, because
    /// nothing joins the follower's thread.
    pub fn disconnect(mut self) {
        self.close();
    }

    fn close(&mut self) {
        let _ = self.socket.shutdown(Shutdown::Both);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl Drop for RealtimeConnection {
    fn drop(&mut self) {
        self.close();
    }
}

fn connecting(error: io::Error) -> CollabError {
    CollabError::Transport(ConnectionProblem::while_connecting(&error))
}

/// Connects to the first of the host's addresses that answers. `localhost`
/// often has two, `::1` and `127.0.0.1`, and a server may listen on one.
fn open_socket(address: &ServerAddress, timeout: Duration) -> CollabResult<TcpStream> {
    let host = address.host().trim_start_matches('[').trim_end_matches(']');
    let mut last_error = None;
    for ip in (host, address.port())
        .to_socket_addrs()
        .map_err(connecting)?
    {
        match TcpStream::connect_timeout(&ip, timeout) {
            Ok(socket) => return Ok(socket),
            Err(error) => last_error = Some(error),
        }
    }
    Err(match last_error {
        Some(error) => connecting(error),
        None => CollabError::Transport(ConnectionProblem::NotReachable(format!(
            "`{host}` has no address"
        ))),
    })
}

fn read_stream(stream: Stream, sender: &mpsc::Sender<Message>) -> io::Result<()> {
    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    reader.read_line(&mut status)?;
    if !status.starts_with("HTTP/1.1 200") {
        // Carried inside the io::Error, so the plain message survives.
        return Err(io::Error::other(ConnectionProblem::LiveUpdatesRefused(
            status.trim_end().to_owned(),
        )));
    }
    let mut chunked = false;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            return Ok(());
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("transfer-encoding")
                && value.trim().eq_ignore_ascii_case("chunked")
            {
                chunked = true;
            }
        }
    }
    let body: Box<dyn BufRead> = if chunked {
        Box::new(BufReader::new(Chunked::new(reader)))
    } else {
        Box::new(reader)
    };
    read_events(body, |event| sender.send(Message::Event(event)).is_ok())
}

/// Parses an SSE stream, calling `deliver` for each complete event until it
/// returns false or the stream ends.
fn read_events(
    mut body: impl BufRead,
    mut deliver: impl FnMut(RealtimeEvent) -> bool,
) -> io::Result<()> {
    let mut topic = String::new();
    let mut data = String::new();
    loop {
        let mut line = String::new();
        if body.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if !data.is_empty() || !topic.is_empty() {
                let value = serde_json::from_str(&data).unwrap_or(Value::String(data.clone()));
                let event = RealtimeEvent {
                    topic: if topic.is_empty() {
                        "message".into()
                    } else {
                        std::mem::take(&mut topic)
                    },
                    data: value,
                };
                data.clear();
                if !deliver(event) {
                    return Ok(());
                }
            }
            continue;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => topic = value.to_owned(),
            "data" => {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(value);
            }
            _ => {}
        }
    }
}

/// Decodes an HTTP/1.1 chunked body.
struct Chunked<R> {
    inner: R,
    remaining: usize,
    done: bool,
}

impl<R: BufRead> Chunked<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            remaining: 0,
            done: false,
        }
    }
}

impl<R: BufRead> Read for Chunked<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.done || buffer.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut size = String::new();
            if self.inner.read_line(&mut size)? == 0 {
                self.done = true;
                return Ok(0);
            }
            let size = size.trim_end();
            let size = size.split(';').next().unwrap_or_default();
            self.remaining = usize::from_str_radix(size, 16).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("bad chunk size `{size}`"),
                )
            })?;
            if self.remaining == 0 {
                self.done = true;
                return Ok(0);
            }
        }
        let wanted = buffer.len().min(self.remaining);
        let read = self.inner.read(&mut buffer[..wanted])?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.remaining -= read;
        if self.remaining == 0 {
            let mut crlf = String::new();
            self.inner.read_line(&mut crlf)?;
        }
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_parsed_from_a_chunked_stream() {
        let payload = "id:x\nevent:PB_CONNECT\ndata:{\"clientId\":\"x\"}\n\nevent:collab_changes\ndata:{\"action\":\"create\"}\n\n";
        let (first, second) = payload.split_at(20);
        let chunked = format!(
            "{:x}\r\n{first}\r\n{:x};ext=1\r\n{second}\r\n0\r\n\r\n",
            first.len(),
            second.len()
        );
        let body = BufReader::new(Chunked::new(chunked.as_bytes()));
        let mut events = Vec::new();
        read_events(body, |event| {
            events.push(event);
            true
        })
        .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].topic, "PB_CONNECT");
        assert_eq!(events[0].data["clientId"], "x");
        assert_eq!(events[1].topic, "collab_changes");
        assert_eq!(events[1].data["action"], "create");
    }

    /// A plain HTTP server that sends `PB_CONNECT`, then nothing more:
    /// a network that went away without closing the connection.
    fn silent_after_connecting() -> PocketBaseClient {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = connection.read(&mut request);
            let _ = connection.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\nevent:PB_CONNECT\ndata:{\"clientId\":\"x\"}\n\n",
            );
            std::thread::sleep(Duration::from_secs(30));
        });
        PocketBaseClient::new(&url)
    }

    #[test]
    fn a_stream_silent_for_too_long_counts_as_gone() {
        let client = silent_after_connecting();
        let mut connection = RealtimeConnection::connect_with_idle_limit(
            &client,
            Duration::from_secs(5),
            Duration::from_millis(500),
        )
        .unwrap();
        assert_eq!(connection.client_id(), "x");
        let started = Instant::now();
        assert_eq!(
            connection.next_event(Duration::from_secs(10)),
            Err(CollabError::Transport(ConnectionProblem::TimedOut))
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn only_http_and_https_urls_are_accepted() {
        assert!(matches!(
            RealtimeConnection::connect(
                &PocketBaseClient::new("ws://127.0.0.1:9"),
                Duration::from_secs(1)
            ),
            Err(CollabError::Protocol(_))
        ));
    }
}

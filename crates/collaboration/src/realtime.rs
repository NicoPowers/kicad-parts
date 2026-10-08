//! PocketBase realtime notifications over server-sent events.
//!
//! The connection is a plain HTTP/1.1 stream on its own socket, read by a
//! background thread, so [`RealtimeConnection::disconnect`] can drop it at
//! any moment, as a network failure would. It supports `http://` URLs; a
//! deployed server behind TLS needs a TLS stream here (a follow-up for the
//! deployment work).

use crate::client::PocketBaseClient;
use crate::{CollabError, CollabResult};
use serde_json::Value;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

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
    Closed(String),
}

/// An open realtime stream.
pub struct RealtimeConnection {
    client_id: String,
    socket: TcpStream,
    events: Receiver<Message>,
    reader: Option<JoinHandle<()>>,
    closed: Option<String>,
}

impl RealtimeConnection {
    /// Opens `GET /api/realtime` and waits for PocketBase's `PB_CONNECT`
    /// event, which carries this connection's client ID.
    pub fn connect(base_url: &str, timeout: Duration) -> CollabResult<Self> {
        let authority = base_url
            .trim_end_matches('/')
            .strip_prefix("http://")
            .ok_or_else(|| {
                CollabError::Protocol(format!(
                    "realtime supports http:// URLs only, not `{base_url}`"
                ))
            })?;
        let address = authority
            .to_socket_addrs()
            .map_err(|io| CollabError::Transport(io.to_string()))?
            .next()
            .ok_or_else(|| CollabError::Transport(format!("`{authority}` has no address")))?;
        let socket = TcpStream::connect_timeout(&address, timeout)
            .map_err(|io| CollabError::Transport(io.to_string()))?;
        let mut writer = socket
            .try_clone()
            .map_err(|io| CollabError::Transport(io.to_string()))?;
        write!(
            writer,
            "GET /api/realtime HTTP/1.1\r\nHost: {authority}\r\nAccept: text/event-stream\r\nCache-Control: no-cache\r\n\r\n"
        )
        .map_err(|io| CollabError::Transport(io.to_string()))?;
        let reader_socket = socket
            .try_clone()
            .map_err(|io| CollabError::Transport(io.to_string()))?;
        let (sender, events) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let reason = match read_stream(reader_socket, &sender) {
                Ok(()) => "the server closed the stream".to_owned(),
                Err(io) => io.to_string(),
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
            .ok_or_else(|| CollabError::Transport("no PB_CONNECT event".into()))?;
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
            Err(RecvTimeoutError::Disconnected) => {
                Err(CollabError::Transport("the realtime reader stopped".into()))
            }
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

    /// Drops the connection immediately, like a network failure.
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

fn read_stream(socket: TcpStream, sender: &mpsc::Sender<Message>) -> io::Result<()> {
    let mut reader = BufReader::new(socket);
    let mut status = String::new();
    reader.read_line(&mut status)?;
    if !status.starts_with("HTTP/1.1 200") {
        return Err(io::Error::other(format!(
            "realtime answered `{}`",
            status.trim_end()
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

    #[test]
    fn only_http_urls_are_accepted() {
        assert!(matches!(
            RealtimeConnection::connect("https://example.invalid", Duration::from_secs(1)),
            Err(CollabError::Protocol(_))
        ));
    }
}

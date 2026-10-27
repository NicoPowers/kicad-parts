//! A proxy on 127.0.0.1 in front of a disposable PocketBase, for tests that
//! save something at an exact moment while a client reads, and that count
//! the client's requests.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub type Hook = Box<dyn FnOnce() + Send>;

pub struct Proxy {
    /// Where clients connect instead of the PocketBase.
    pub url: String,
    requests: Arc<AtomicUsize>,
}

impl Proxy {
    /// Passes every request on to `upstream`. Just before it passes on the
    /// `nth` request (from 1) whose path contains `path`, it runs `hook`
    /// once and waits for it to finish.
    pub fn start(upstream: &str, path: &'static str, nth: usize, hook: Hook) -> Self {
        let upstream = upstream.trim_start_matches("http://").to_owned();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let trigger = Arc::new(Trigger {
            path,
            nth,
            seen: Mutex::new(0),
            hook: Mutex::new(Some(hook)),
        });
        let counted = requests.clone();
        std::thread::spawn(move || {
            for client in listener.incoming().flatten() {
                let (upstream, trigger, counted) =
                    (upstream.clone(), trigger.clone(), counted.clone());
                std::thread::spawn(move || {
                    let _ = pass_on(client, &upstream, &trigger, &counted);
                });
            }
        });
        Self { url, requests }
    }

    /// How many requests the proxy has passed on so far.
    pub fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

struct Trigger {
    path: &'static str,
    nth: usize,
    seen: Mutex<usize>,
    hook: Mutex<Option<Hook>>,
}

impl Trigger {
    fn on_request(&self, head: &str) {
        let first_line = head.lines().next().unwrap_or_default();
        if !first_line.contains(self.path) {
            return;
        }
        let mut seen = self.seen.lock().unwrap();
        *seen += 1;
        if *seen == self.nth {
            let hook = self.hook.lock().unwrap().take();
            if let Some(hook) = hook {
                hook();
            }
        }
    }
}

fn pass_on(
    mut client: TcpStream,
    upstream: &str,
    trigger: &Trigger,
    requests: &AtomicUsize,
) -> std::io::Result<()> {
    let mut request = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        let read = client.read(&mut chunk)?;
        if read == 0 {
            return Ok(());
        }
        request.extend_from_slice(&chunk[..read]);
        if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&request[..head_end]).into_owned();
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    while request.len() < head_end + length {
        let read = client.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
    }
    requests.fetch_add(1, Ordering::SeqCst);
    trigger.on_request(&head);
    // One request per connection keeps the forwarding simple.
    let mut lines: Vec<&str> = head
        .trim_end()
        .lines()
        .filter(|line| !line.to_ascii_lowercase().starts_with("connection:"))
        .collect();
    lines.push("Connection: close");
    let mut server = TcpStream::connect(upstream)?;
    server.write_all(format!("{}\r\n\r\n", lines.join("\r\n")).as_bytes())?;
    server.write_all(&request[head_end..])?;
    std::io::copy(&mut server, &mut client)?;
    Ok(())
}

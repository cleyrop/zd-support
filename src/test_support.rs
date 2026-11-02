use serde_json::Value;
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use url::Url;

pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}
impl Response {
    pub fn json(value: Value) -> Self {
        Self {
            status: 200,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: value.to_string(),
        }
    }
}
pub struct Server {
    pub url: Url,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl Server {
    pub fn new(mut handle: impl FnMut(Request, &Url) -> Response + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let base = url.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Accepted sockets inherit nonblocking mode on macOS.
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let Some(request) = read_request(&mut stream) else {
                            continue;
                        };
                        let response = handle(request, &base);
                        let mut headers = format!(
                            "HTTP/1.1 {} Test\r\nConnection: close\r\nContent-Length: {}\r\n",
                            response.status,
                            response.body.len()
                        );
                        for (key, value) in response.headers {
                            headers.push_str(&format!("{key}: {value}\r\n"));
                        }
                        headers.push_str("\r\n");
                        headers.push_str(&response.body);
                        let _ = stream.write_all(headers.as_bytes());
                        // Close gracefully: dropping immediately can reset an unread response on macOS.
                        let _ = stream.shutdown(std::net::Shutdown::Write);
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                        let mut remaining = [0u8; 1024];
                        while matches!(stream.read(&mut remaining), Ok(n) if n > 0) {}
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("Mock listener error: {e}"),
                }
            }
        });
        Self {
            url,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let result = self.worker.take().unwrap().join();
        if !thread::panicking() {
            result.unwrap();
        }
    }
}
fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut bytes = Vec::new();
    let mut buf = [0u8; 4096];
    let end = loop {
        let count = stream.read(&mut buf).ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&buf[..count]);
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let text = String::from_utf8(bytes[..end].to_vec()).unwrap();
    let mut lines = text.lines();
    let mut first = lines.next().unwrap().split_whitespace();
    let method = first.next().unwrap().into();
    let path = first.next().unwrap().into();
    let headers: HashMap<_, _> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.to_lowercase(), v.trim().to_owned()))
        .collect();
    let length = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    while bytes.len() < end + length {
        let count = stream.read(&mut buf).ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&buf[..count]);
    }
    Some(Request {
        method,
        path,
        headers,
        body: bytes[end..end + length].to_vec(),
    })
}

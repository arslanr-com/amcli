//! Just enough HTTP/1.1 to serve a page on the loopback interface.
//!
//! GET and HEAD, one request per connection, `Connection: close` on every
//! response. That is deliberately all: the client is a browser on the same
//! machine talking to a page we wrote, so there is no negotiation to do, and
//! writing these hundred lines is cheaper — and easier to audit — than adding
//! a server framework to a binary whose whole point is having no dependencies
//! beyond a shell.
//!
//! Server threads never print. `main` holds the stdio locks until it hands
//! over to the server, and even after that the terminal belongs to the person
//! who ran the command, not to a request log.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use super::state::State;

/// The largest request head we will read. A browser's is under two kilobytes;
/// anything near this is not a browser.
const MAX_HEAD: usize = 16 * 1024;

/// How long a connection may sit idle before its thread is reclaimed. Chrome
/// and Edge open speculative sockets they never write to; without this each
/// would pin a thread for as long as the tab is open.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub struct Request {
    pub method: String,
    /// Percent-decoded, query stripped.
    pub path: String,
    /// Everything after the `?`, raw. Most paths ignore it — the page uses one
    /// only to defeat a cache — but `/api/layout` asks a question with it, so
    /// it is kept and decoded per value rather than whole: a value may contain
    /// an escaped `&` or `=` and splitting a decoded query would lose it.
    pub query: String,
    /// The `Host` header, lower-cased.
    pub host: String,
}

impl Request {
    /// The value of one query parameter, percent-decoded. Empty when absent,
    /// which for every parameter here means the same as "not given".
    pub fn param(&self, name: &str) -> String {
        for pair in self.query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            if percent_decode(k) == name {
                return percent_decode(&v.replace('+', " "));
            }
        }
        String::new()
    }
}

pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    /// Extra headers, already `Name: value`.
    pub extra: Vec<String>,
}

impl Response {
    pub fn new(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Response {
        Response { status, content_type, body: body.into(), extra: Vec::new() }
    }

    pub fn json(status: u16, body: String) -> Response {
        Response::new(status, "application/json; charset=utf-8", body)
    }

    pub fn error(status: u16, message: &str) -> Response {
        Response::json(status, format!("{{\"error\":{}}}", crate::output::quote(message)))
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        _ => "",
    }
}

/// Serve until the listener fails for good. Every connection gets a thread; a
/// page fetches a handful of things at once and the OS is better at scheduling
/// that than a pool we would have to size.
pub fn serve(listener: TcpListener, state: Arc<State>) {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let state = Arc::clone(&state);
                std::thread::spawn(move || handle(stream, &state));
            }
            // Out of descriptors, or a connection reset between accept and
            // return: neither is a reason to stop serving.
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn handle(mut stream: TcpStream, state: &State) {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_nodelay(true);

    let response = match read_head(&mut stream) {
        Ok(head) => match parse(&head) {
            Ok(req) => {
                if req.method != "GET" && req.method != "HEAD" {
                    let mut r = Response::error(405, "only GET and HEAD are served");
                    r.extra.push("Allow: GET, HEAD".to_string());
                    r
                } else if !host_allowed(&req.host, state.port, &state.allow_hosts) {
                    // A page on some other origin that resolved a name to
                    // 127.0.0.1 could otherwise read the model through the
                    // visitor's browser.
                    Response::error(403, "this page is served to localhost and --allow-host names")
                } else {
                    let mut r = super::api::route(&req, state);
                    if req.method == "HEAD" {
                        r.body.clear();
                    }
                    r
                }
            }
            Err(status) => Response::error(status, "malformed request"),
        },
        Err(status) => Response::error(status, "could not read request"),
    };
    let head_only = response.body.is_empty();
    write_response(&mut stream, &response, head_only);
    let _ = stream.shutdown(Shutdown::Both);
}

fn read_head(stream: &mut TcpStream) -> Result<Vec<u8>, u16> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).map_err(|_| 400u16)?;
        if n == 0 {
            return Err(400);
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.windows(2).any(|w| w == b"\n\n") {
            return Ok(buf);
        }
        if buf.len() > MAX_HEAD {
            return Err(413);
        }
    }
}

/// The request line and the one header we look at. Tolerates bare `\n` line
/// ends, which some tools send and no server should choke on.
pub fn parse(head: &[u8]) -> Result<Request, u16> {
    let text = std::str::from_utf8(head).map_err(|_| 400u16)?;
    let mut lines = text.split('\n').map(|l| l.trim_end_matches('\r'));
    let line = lines.next().ok_or(400u16)?;
    let mut parts = line.split(' ');
    let method = parts.next().filter(|m| !m.is_empty()).ok_or(400u16)?.to_string();
    let target = parts.next().ok_or(400u16)?;
    if !target.starts_with('/') {
        return Err(400);
    }
    let (raw_path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q.to_string()),
        None => (target, String::new()),
    };
    let path = percent_decode(raw_path);
    if path.contains('\0') {
        return Err(400);
    }
    let mut host = String::new();
    for l in lines {
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':')
            && k.eq_ignore_ascii_case("host")
        {
            host = v.trim().to_ascii_lowercase();
        }
    }
    Ok(Request { method, path, query, host })
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn host_is_local(host: &str, port: u16) -> bool {
    // A missing Host is HTTP/1.0 or a hand-typed request; both are local.
    if host.is_empty() {
        return true;
    }
    let bare = host.strip_suffix(&format!(":{port}")).unwrap_or(host);
    matches!(bare, "127.0.0.1" | "localhost" | "[::1]")
}

/// The loopback names, plus whatever `--allow-host` added.
///
/// A reverse proxy passes the name the reader typed, which is neither
/// loopback nor on the port we bound, so a deployment has to say that name
/// out loud. Naming it is the whole check: an origin that is not on the list
/// is still refused, so the rebinding defence survives the container.
fn host_allowed(host: &str, port: u16, allowed: &[String]) -> bool {
    if host_is_local(host, port) {
        return true;
    }
    let bare = strip_port(host);
    allowed.iter().any(|a| a == "*" || a == host || a == bare)
}

/// `example.test:8080` → `example.test`, leaving an IPv6 literal alone: its
/// colons are the address, and its port would be outside the brackets.
fn strip_port(host: &str) -> &str {
    let at = match host.rfind(']') {
        Some(b) => host[b..].find(':').map(|i| b + i),
        None if host.matches(':').count() == 1 => host.find(':'),
        None => None,
    };
    match at {
        Some(i) if host[i + 1..].bytes().all(|b| b.is_ascii_digit()) => &host[..i],
        _ => host,
    }
}

fn write_response(stream: &mut TcpStream, r: &Response, head_only: bool) {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
         Connection: close\r\nCache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Content-Security-Policy: default-src 'none'; script-src 'self'; \
         style-src 'self' 'unsafe-inline'; img-src 'self' data:; \
         connect-src 'self'; base-uri 'none'; frame-ancestors 'none'\r\n",
        r.status,
        reason(r.status),
        r.content_type,
        r.body.len()
    );
    for h in &r.extra {
        head.push_str(h);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    if !head_only {
        let _ = stream.write_all(&r.body);
    }
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_browser_request_line_and_host() {
        let r = parse(
            b"GET /api/view/a%20b.svg?c=1 HTTP/1.1\r\nHost: 127.0.0.1:8080\r\nAccept: */*\r\n\r\n",
        )
        .unwrap();
        assert_eq!(r.method, "GET");
        assert_eq!(r.path, "/api/view/a b.svg");
        assert_eq!(r.query, "c=1");
        assert_eq!(r.host, "127.0.0.1:8080");
    }

    #[test]
    fn reads_query_parameters_one_value_at_a_time() {
        let r =
            parse(b"GET /api/layout?e=0-9,12&hiderel=Access%2CServing HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(r.path, "/api/layout");
        assert_eq!(r.param("e"), "0-9,12");
        // An escaped separator survives, which it would not if the whole
        // query were decoded before being split.
        assert_eq!(r.param("hiderel"), "Access,Serving");
        assert_eq!(r.param("missing"), "");
    }

    #[test]
    fn tolerates_bare_newlines_and_missing_host() {
        let r = parse(b"HEAD / HTTP/1.1\n\n").unwrap();
        assert_eq!(r.method, "HEAD");
        assert_eq!(r.path, "/");
        assert!(r.host.is_empty());
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse(b"\r\n\r\n").unwrap_err(), 400);
        assert_eq!(parse(b"GET foo HTTP/1.1\r\n\r\n").unwrap_err(), 400);
    }

    #[test]
    fn only_loopback_hosts_pass() {
        assert!(host_is_local("localhost:5000", 5000));
        assert!(host_is_local("127.0.0.1:5000", 5000));
        assert!(host_is_local("", 5000));
        assert!(!host_is_local("evil.example:5000", 5000));
        assert!(!host_is_local("127.0.0.1:5001", 5000));
    }

    /// Behind a proxy the Host is the name the reader typed, on the proxy's
    /// port rather than ours. Only the names that were asked for get in.
    #[test]
    fn an_allowed_host_gets_in_and_nothing_else_does() {
        let allowed = ["amcli.example.test".to_string()];
        assert!(host_allowed("amcli.example.test", 3000, &allowed));
        assert!(host_allowed("amcli.example.test:443", 3000, &allowed));
        assert!(host_allowed("localhost:3000", 3000, &allowed), "the healthcheck still passes");
        assert!(!host_allowed("evil.example", 3000, &allowed));
        assert!(!host_allowed("amcli.example.test.evil", 3000, &allowed));
        // Nothing named, nothing but loopback: the default is unchanged.
        assert!(!host_allowed("amcli.example.test", 3000, &[]));
    }

    #[test]
    fn a_port_comes_off_a_name_but_never_off_an_address() {
        assert_eq!(strip_port("amcli.example.test:8080"), "amcli.example.test");
        assert_eq!(strip_port("amcli.example.test"), "amcli.example.test");
        assert_eq!(strip_port("[::1]:8080"), "[::1]");
        assert_eq!(strip_port("[fe80::1]"), "[fe80::1]");
        assert_eq!(strip_port("host:notaport"), "host:notaport");
    }

    #[test]
    fn percent_decoding_leaves_bad_escapes_alone() {
        assert_eq!(percent_decode("a%2Fb%zz%4"), "a/b%zz%4");
    }
}

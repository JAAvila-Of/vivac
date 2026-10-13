use super::PreviewRevision;
use crate::failure::Failure;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const LIFETIME: Duration = Duration::from_secs(15 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
const STACK_BYTES: usize = 128 * 1024;
const WRAPPER_POLICY: &str = "default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; frame-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'; object-src 'none'";
const CHILD_POLICY: &str = "sandbox allow-scripts; default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data: blob:; media-src data: blob:; connect-src 'none'; frame-src 'none'; worker-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'self'";
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

struct Slot;

impl Slot {
    fn reserve() -> Result<Self, Failure> {
        let mut active = ACTIVE.load(Ordering::Acquire);
        loop {
            if active >= 4 {
                return Err(Failure::usage(
                    "At most four retained HTML preview servers can be active at once.",
                ));
            }
            match ACTIVE.compare_exchange_weak(
                active,
                active + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(Self),
                Err(observed) => active = observed,
            }
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) struct Preview {
    url: String,
    worker: JoinHandle<()>,
    #[cfg(test)]
    stats: std::sync::mpsc::Receiver<usize>,
}

impl Preview {
    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    pub(crate) fn wait(self) {
        let _ = self.worker.join();
    }
}

pub(crate) fn start(selected: PreviewRevision) -> Result<Preview, Failure> {
    start_for(selected, LIFETIME)
}

fn start_for(selected: PreviewRevision, lifetime: Duration) -> Result<Preview, Failure> {
    let slot = Slot::reserve()?;
    let mut random = [0; 32];
    getrandom::getrandom(&mut random)
        .map_err(|error| Failure::Io(io::Error::other(error.to_string())))?;
    let token: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    listener.set_nonblocking(true)?;
    let host = format!("localhost:{}", listener.local_addr()?.port());
    let origin = format!("http://{host}");
    let path = format!("/{token}");
    let url = format!("{origin}{path}");
    let deadline = Instant::now() + lifetime.min(LIFETIME);
    #[cfg(test)]
    let (stats_sender, stats) = std::sync::mpsc::channel();
    let worker = thread::Builder::new()
        .name("artifact-preview".into())
        .stack_size(STACK_BYTES)
        .spawn(move || {
            let _slot = slot;
            #[cfg(test)]
            {
                let (_, peak) = crate::test_alloc::measure(|| {
                    serve(listener, selected, &host, &origin, &path, &token, deadline)
                });
                let _ = stats_sender.send(peak);
            }
            #[cfg(not(test))]
            serve(listener, selected, &host, &origin, &path, &token, deadline);
        })?;
    Ok(Preview {
        url,
        worker,
        #[cfg(test)]
        stats,
    })
}

fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn wrapper(selected: &PreviewRevision, token: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><meta name=\"referrer\" content=\"origin\"><title>Preview retained revision</title><style>body{{font-family:system-ui;margin:0;background:#f5f7fa;color:#17212b}}main{{padding:20px;max-width:1400px;margin:auto}}h1{{margin:0;font-size:22px}}code,dd{{overflow-wrap:anywhere;word-break:break-word}}.small{{font-size:13px}}.controls{{display:flex;gap:12px;flex-wrap:wrap}}form{{display:inline}}button{{padding:10px 14px}}iframe{{display:block;width:100%;height:calc(100vh - 260px);min-height:420px;box-sizing:border-box;border:1px solid #ccd4df;border-radius:8px;margin-top:16px;background:white}}</style></head><body><main><h1>Preview retained revision</h1><p>Source: <code>{}</code><br>Revision: <code>{}</code></p><details><summary>Revision details</summary><dl><dt>Artifact</dt><dd>{}</dd><dt>Producer</dt><dd>{}</dd><dt>Digest</dt><dd>{}</dd></dl></details><p class=\"small\">JavaScript runs with browser restrictions. Complete network isolation is not guaranteed. Preview does not imply approval.</p><p class=\"small\">The local server expires after 15 minutes. Close this tab to stop HTML that is already running.</p><div class=\"controls\"><form action=\"/run\" method=\"post\" target=\"retained\"><input type=\"hidden\" name=\"token\" value=\"{token}\"><button type=\"submit\">Run HTML</button></form><form action=\"/close\" method=\"post\" target=\"_self\"><input type=\"hidden\" name=\"token\" value=\"{token}\"><button type=\"submit\">Close preview</button></form></div><iframe name=\"retained\" sandbox=\"allow-scripts\" referrerpolicy=\"origin\" title=\"Retained HTML\"></iframe></main></body></html>",
        escape(&selected.source),
        escape(&selected.revision),
        escape(&selected.artifact),
        escape(&selected.producer),
        escape(&selected.digest),
    )
}

fn serve(
    listener: TcpListener,
    mut selected: PreviewRevision,
    host: &str,
    origin: &str,
    path: &str,
    token: &str,
    deadline: Instant,
) {
    while Instant::now() < deadline {
        match listener.accept() {
            Ok((mut socket, _)) => {
                if Instant::now() >= deadline {
                    break;
                }
                if socket.set_nonblocking(false).is_err() {
                    continue;
                }
                let request_deadline = deadline.min(Instant::now() + REQUEST_TIMEOUT);
                let action = receive(&mut socket, host, origin, path, token, request_deadline);
                let response_deadline = deadline.min(Instant::now() + RESPONSE_TIMEOUT);
                match action {
                    Ok(Action::Wrapper) => {
                        let body = wrapper(&selected, token);
                        let _ = respond(
                            &mut socket,
                            200,
                            ResponsePolicy::Wrapper,
                            &mut body.as_bytes(),
                            body.len() as u64,
                            response_deadline,
                        );
                    }
                    Ok(Action::Run) => {
                        if selected.reader.rewind().is_ok() {
                            let size = selected.reader.size();
                            let _ = respond(
                                &mut socket,
                                200,
                                ResponsePolicy::Child,
                                &mut selected.reader,
                                size,
                                response_deadline,
                            );
                        }
                    }
                    Ok(Action::Close) => {
                        let body =
                            "<!doctype html><title>Preview closed.</title><p>Preview closed.</p>";
                        let _ = respond(
                            &mut socket,
                            200,
                            ResponsePolicy::Trusted,
                            &mut body.as_bytes(),
                            body.len() as u64,
                            response_deadline,
                        );
                        break;
                    }
                    Err(status) => {
                        let body = "Preview request refused.";
                        let _ = respond(
                            &mut socket,
                            status,
                            ResponsePolicy::Trusted,
                            &mut body.as_bytes(),
                            body.len() as u64,
                            response_deadline,
                        );
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(
                    Duration::from_millis(20)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            Err(_) => break,
        }
    }
}

#[derive(Clone, Copy)]
enum Action {
    Wrapper,
    Run,
    Close,
}

enum ResponsePolicy {
    Wrapper,
    Child,
    Trusted,
}

fn unique<'a>(headers: &'a [httparse::Header<'a>], name: &str) -> Result<Option<&'a [u8]>, u16> {
    let mut found = headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case(name));
    let value = found.next().map(|header| header.value);
    if found.next().is_some() {
        return Err(400);
    }
    Ok(value)
}

fn admit(
    request: &httparse::Request<'_, '_>,
    host: &str,
    origin: &str,
    path: &str,
) -> Result<(Action, usize), u16> {
    if request.version != Some(1) {
        return Err(400);
    }
    if unique(request.headers, "Host")? != Some(host.as_bytes()) {
        return Err(403);
    }
    let offered_origin = unique(request.headers, "Origin")?;
    if offered_origin.is_some_and(|value| value != origin.as_bytes()) {
        return Err(403);
    }
    if unique(request.headers, "Transfer-Encoding")?.is_some()
        || unique(request.headers, "Expect")?.is_some()
        || unique(request.headers, "Upgrade")?.is_some()
    {
        return Err(400);
    }
    let length = match unique(request.headers, "Content-Length")? {
        Some(value) if !value.is_empty() && value.iter().all(u8::is_ascii_digit) => {
            std::str::from_utf8(value)
                .map_err(|_| 400u16)?
                .parse::<usize>()
                .map_err(|_| 400u16)?
        }
        Some(_) => return Err(400),
        None => 0,
    };
    if length > 256 {
        return Err(400);
    }
    match (request.method, request.path) {
        (Some("GET"), Some(offered)) if offered == path && length == 0 => Ok((Action::Wrapper, 0)),
        (Some("POST"), Some("/run" | "/close")) => {
            if offered_origin != Some(origin.as_bytes()) {
                return Err(403);
            }
            if unique(request.headers, "Content-Type")?
                != Some(b"application/x-www-form-urlencoded")
                || unique(request.headers, "Content-Length")?.is_none()
            {
                return Err(400);
            }
            Ok((
                if request.path == Some("/run") {
                    Action::Run
                } else {
                    Action::Close
                },
                length,
            ))
        }
        (Some("GET" | "POST"), _) => Err(404),
        _ => Err(405),
    }
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Preview deadline expired.",
        ))
    } else {
        Ok(remaining)
    }
}

fn read_before(socket: &mut TcpStream, buffer: &mut [u8], deadline: Instant) -> io::Result<usize> {
    socket.set_read_timeout(Some(remaining(deadline)?))?;
    socket.read(buffer)
}

fn receive(
    socket: &mut TcpStream,
    host: &str,
    origin: &str,
    path: &str,
    token: &str,
    deadline: Instant,
) -> Result<Action, u16> {
    let mut buffer = [0; 8192];
    let mut used = 0;
    let end = loop {
        let mut headers = [httparse::EMPTY_HEADER; 32];
        let mut request = httparse::Request::new(&mut headers);
        match request.parse(&buffer[..used]) {
            Ok(httparse::Status::Complete(end)) => break end,
            Err(_) => return Err(400),
            Ok(httparse::Status::Partial) => {}
        }
        if used == buffer.len() {
            return Err(400);
        }
        let count = read_before(socket, &mut buffer[used..], deadline).map_err(|_| 400u16)?;
        if count == 0 {
            return Err(400);
        }
        used += count;
    };
    let (action, length) = {
        let mut headers = [httparse::EMPTY_HEADER; 32];
        let mut request = httparse::Request::new(&mut headers);
        request.parse(&buffer[..used]).map_err(|_| 400u16)?;
        admit(&request, host, origin, path)?
    };
    if used - end > length {
        return Err(400);
    }
    let mut body = [0; 256];
    let mut received = used - end;
    body[..received].copy_from_slice(&buffer[end..used]);
    while received < length {
        let count =
            read_before(socket, &mut body[received..length], deadline).map_err(|_| 400u16)?;
        if count == 0 {
            return Err(400);
        }
        received += count;
    }
    socket.set_nonblocking(true).map_err(|_| 400u16)?;
    let mut extra = [0; 1];
    let pipelined = match socket.peek(&mut extra) {
        Ok(count) => count > 0,
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => false,
        Err(_) => true,
    };
    socket.set_nonblocking(false).map_err(|_| 400u16)?;
    if pipelined {
        return Err(400);
    }
    if !matches!(action, Action::Wrapper) {
        let expected = format!("token={token}");
        if length != expected.len() || !body[..length].is_ascii() {
            return Err(403);
        }
        let different = body[..length]
            .iter()
            .zip(expected.bytes())
            .fold(0, |different, (left, right)| different | (left ^ right));
        if different != 0 {
            return Err(403);
        }
    }
    remaining(deadline).map_err(|_| 400u16)?;
    Ok(action)
}

fn write_before(socket: &mut TcpStream, mut bytes: &[u8], deadline: Instant) -> io::Result<()> {
    while !bytes.is_empty() {
        socket.set_write_timeout(Some(remaining(deadline)?))?;
        match socket.write(bytes) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "Preview response could not be written.",
                ))
            }
            Ok(count) => bytes = &bytes[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn respond(
    socket: &mut TcpStream,
    status: u16,
    policy: ResponsePolicy,
    reader: &mut impl Read,
    size: u64,
    deadline: Instant,
) -> io::Result<()> {
    let (policy, referrer_policy) = match policy {
        ResponsePolicy::Wrapper => (WRAPPER_POLICY, "origin"),
        ResponsePolicy::Child => (CHILD_POLICY, "no-referrer"),
        ResponsePolicy::Trusted => (WRAPPER_POLICY, "no-referrer"),
    };
    let reason = match status {
        200 => "OK",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Bad Request",
    };
    let header = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {size}\r\nConnection: close\r\nContent-Security-Policy: {policy}\r\nReferrer-Policy: {referrer_policy}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nX-DNS-Prefetch-Control: off\r\nPermissions-Policy: camera=(), microphone=(), geolocation=()\r\nCross-Origin-Opener-Policy: same-origin\r\n\r\n");
    write_before(socket, header.as_bytes(), deadline)?;
    let mut buffer = [0; 8192];
    let mut left = size;
    while left != 0 {
        remaining(deadline)?;
        let count = reader.read(&mut buffer[..left.min(8192) as usize])?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Preview retained bytes ended early.",
            ));
        }
        write_before(socket, &buffer[..count], deadline)?;
        left -= count as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpStream};
    use std::sync::Mutex;

    static SERVERS: Mutex<()> = Mutex::new(());

    fn selected(bytes: &[u8]) -> PreviewRevision {
        let root = super::super::standalone_root("preview-runner");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("mockup.html"), bytes).unwrap();
        let revision = crate::id::ulid();
        let saved = super::super::files::capture(&root, "mockup.html", &revision).unwrap();
        let reader =
            super::super::files::download(&root, &saved.object, &saved.digest, saved.size).unwrap();
        fs::write(root.join("mockup.html"), b"later source").unwrap();
        fs::remove_dir_all(root).unwrap();
        PreviewRevision {
            reader,
            artifact: crate::id::ulid(),
            revision,
            source: "mockup.html".into(),
            producer: crate::id::ulid(),
            digest: saved.digest,
        }
    }

    fn parts(url: &str) -> (&str, &str) {
        url.strip_prefix("http://")
            .unwrap()
            .split_once('/')
            .unwrap()
    }

    fn request(url: &str, method: &str, path: &str, headers: &str, body: &str) -> String {
        let (host, _) = parts(url);
        request_host(url, host, method, path, headers, body)
    }

    fn request_host(
        url: &str,
        host: &str,
        method: &str,
        path: &str,
        headers: &str,
        body: &str,
    ) -> String {
        let (address, _) = parts(url);
        let port: u16 = address.strip_prefix("localhost:").unwrap().parse().unwrap();
        let mut socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socket.write_all(format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n{headers}\r\n{body}").as_bytes()).unwrap();
        socket.shutdown(Shutdown::Write).unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).unwrap();
        response
    }

    fn post(url: &str, action: &str) -> String {
        let (host, token) = parts(url);
        let body = format!("token={token}");
        request(url, "POST", &format!("/{action}"), &format!("Origin: http://{host}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n",body.len()), &body)
    }

    #[test]
    fn preview_runner_separates_escaped_wrapper_from_active_html_and_closes() {
        let _guard = SERVERS.lock().unwrap_or_else(|error| error.into_inner());
        let mut delivery = selected(b"<html><script>window.retained = true;</script></html>");
        delivery.source = "mockup\"><script>&'file.html".into();
        let preview = start_for(delivery, Duration::from_secs(2)).unwrap();
        let (_, token) = parts(preview.url());
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
        let wrapper = request(preview.url(), "GET", &format!("/{token}"), "", "");
        assert!(wrapper.starts_with("HTTP/1.1 200"));
        assert!(wrapper.contains("Preview retained revision"));
        assert!(wrapper.contains("mockup&quot;&gt;&lt;script&gt;&amp;&#39;file.html"));
        assert!(!wrapper.contains("window.retained"));
        assert!(wrapper.contains("sandbox=\"allow-scripts\""));
        assert!(wrapper.contains("Run HTML"));
        assert!(wrapper.contains("Complete network isolation is not guaranteed."));
        assert!(wrapper.contains("script-src 'none'"));
        assert!(wrapper.contains("form-action 'self'"));
        assert!(wrapper.contains("frame-ancestors 'none'"));
        assert!(wrapper.contains("Cross-Origin-Opener-Policy: same-origin"));
        assert!(wrapper.contains("Referrer-Policy: origin\r\n"));
        assert!(wrapper.contains("name=\"referrer\" content=\"origin\""));
        assert!(wrapper.contains("referrerpolicy=\"origin\""));
        let run = post(preview.url(), "run");
        assert!(run.starts_with("HTTP/1.1 200"));
        assert!(run.contains("sandbox allow-scripts"));
        assert!(run.contains("connect-src 'none'"));
        assert!(run.contains("worker-src 'none'"));
        assert!(run.contains("script-src 'unsafe-inline'"));
        assert!(!run.contains("'unsafe-eval'"));
        assert!(!run.contains(token));
        assert!(run.contains("Referrer-Policy: no-referrer\r\n"));
        assert!(run.ends_with("<html><script>window.retained = true;</script></html>"));
        assert!(post(preview.url(), "close").contains("Preview closed."));
        preview.wait();
    }

    #[test]
    fn preview_runner_gates_origin_host_method_path_and_bounded_form() {
        let _guard = SERVERS.lock().unwrap_or_else(|error| error.into_inner());
        let preview =
            start_for(selected(b"<html>selected</html>"), Duration::from_secs(2)).unwrap();
        let (host, token) = parts(preview.url());
        let path = String::from("/run");
        let body = format!("token={token}");
        let valid = format!("Origin: http://{host}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n", body.len());
        let forged = format!(
            "token={}{}",
            if token.starts_with('0') { '1' } else { '0' },
            &token[1..]
        );
        assert_eq!(forged.len(), body.len());
        let refused = request(preview.url(), "POST", &path, &valid, &forged);
        assert!(refused.starts_with("HTTP/1.1 403"));
        assert!(!refused.contains("<html>selected</html>"));
        assert!(!request_host(
            preview.url(),
            "127.0.0.1",
            "GET",
            &format!("/{token}"),
            "",
            ""
        )
        .starts_with("HTTP/1.1 200"));
        for (method, path, headers, body) in [
            ("GET", path.as_str(), "".into(), "".into()),
            ("POST", path.as_str(), format!("Origin: http://127.0.0.1\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n",body.len()), body.clone()),
            ("POST", path.as_str(), format!("Origin: null\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n",body.len()), body.clone()),
            ("POST", path.as_str(), format!("{valid}Origin: http://{host}\r\n"), body.clone()),
            ("POST", path.as_str(), format!("{valid}Host: {host}\r\n"), body.clone()),
            ("POST", path.as_str(), format!("Origin: http://{host}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n",body.len()), body.clone()),
            ("POST", path.as_str(), format!("{valid}Content-Length: {}\r\n", body.len()), body.clone()),
            ("POST", path.as_str(), format!("{valid}Transfer-Encoding: chunked\r\n"), body.clone()),
            ("POST", path.as_str(), format!("{valid}Expect: 100-continue\r\n"), body.clone()),
            ("POST", path.as_str(), valid.clone(), format!("{body}GET / HTTP/1.1\r\n\r\n")),
            ("POST", path.as_str(), format!("Origin: http://{host}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 257\r\n"), "x".repeat(257)),
            ("GET", "/arbitrary-file", "".into(), "".into()),
            ("PUT", path.as_str(), valid.clone(), body.clone()),
        ] {
            let response = request(preview.url(), method, path, &headers, &body);
            assert!(!response.starts_with("HTTP/1.1 200"), "{method} {path} {headers}");
            assert!(!response.contains("<html>selected</html>"));
        }
        assert!(post(preview.url(), "run").contains("<html>selected</html>"));
        post(preview.url(), "close");
        preview.wait();
    }

    #[test]
    fn preview_runner_streams_and_rewinds_a_private_snapshot_with_security_headers() {
        let _guard = SERVERS.lock().unwrap_or_else(|error| error.into_inner());
        let bytes = format!("<html>{}</html>", " x ".repeat(48 * 1024));
        let preview = start_for(selected(bytes.as_bytes()), Duration::from_secs(2)).unwrap();
        for _ in 0..2 {
            let response = post(preview.url(), "run");
            assert!(response.ends_with(&bytes));
            for header in [
                "Referrer-Policy: no-referrer",
                "Cache-Control: no-store",
                "X-Content-Type-Options: nosniff",
                "X-DNS-Prefetch-Control: off",
                "Permissions-Policy: camera=(), microphone=(), geolocation=()",
            ] {
                assert!(response.contains(header), "{header}");
            }
            assert!(response.contains(&format!("Content-Length: {}", bytes.len())));
        }
        post(preview.url(), "close");
        preview.wait();
    }

    #[test]
    fn preview_runner_streams_maximum_delivery_with_bounded_worker_memory() {
        let _guard = SERVERS.lock().unwrap_or_else(|error| error.into_inner());
        let mut bytes = vec![b' '; super::super::MAX_FILE_BYTES as usize];
        bytes[..6].copy_from_slice(b"<html>");
        let preview = start_for(selected(&bytes), Duration::from_secs(5)).unwrap();
        let response = post(preview.url(), "run");
        let body = response.split_once("\r\n\r\n").unwrap().1;
        assert_eq!(body.as_bytes(), bytes);
        post(preview.url(), "close");
        let peak = preview.stats.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(
            peak + 128 * 1024 <= 2 * 1024 * 1024,
            "preview worker allocated {peak} auxiliary bytes"
        );
        preview.wait();
    }

    #[test]
    fn preview_runner_expires_connections_with_incomplete_headers_or_body() {
        let _guard = SERVERS.lock().unwrap_or_else(|error| error.into_inner());
        for body in [false, true] {
            let preview = start_for(
                selected(b"<html>retained</html>"),
                Duration::from_millis(300),
            )
            .unwrap();
            let (host, _) = parts(preview.url());
            let port: u16 = host.strip_prefix("localhost:").unwrap().parse().unwrap();
            let mut socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            if body {
                socket.write_all(format!("POST /run HTTP/1.1\r\nHost: {host}\r\nOrigin: http://{host}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 70\r\n\r\ntoken=").as_bytes()).unwrap();
            } else {
                socket.write_all(b"GET /").unwrap();
            }
            preview
                .stats
                .recv_timeout(Duration::from_secs(2))
                .expect("incomplete requests must not retain the worker beyond expiry");
            preview.wait();
        }
    }

    #[test]
    fn preview_runner_expires_a_blocked_maximum_response_without_waiting_for_the_client() {
        let _guard = SERVERS.lock().unwrap_or_else(|error| error.into_inner());
        let bytes = vec![b' '; super::super::MAX_FILE_BYTES as usize];
        let preview = start_for(selected(&bytes), Duration::from_millis(300)).unwrap();
        let (host, token) = parts(preview.url());
        let port: u16 = host.strip_prefix("localhost:").unwrap().parse().unwrap();
        let mut socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let body = format!("token={token}");
        socket.write_all(format!("POST /run HTTP/1.1\r\nHost: {host}\r\nOrigin: http://{host}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",body.len()).as_bytes()).unwrap();
        preview
            .stats
            .recv_timeout(Duration::from_secs(2))
            .expect("a blocked response must release the worker at expiry");
        preview.wait();
        drop(socket);
    }

    #[test]
    fn preview_runner_request_deadline_is_absolute_for_incomplete_headers_and_body() {
        let _guard = SERVERS.lock().unwrap_or_else(|error| error.into_inner());
        for body in [false, true] {
            let preview =
                start_for(selected(b"<html>retained</html>"), Duration::from_secs(5)).unwrap();
            let (host, _) = parts(preview.url());
            let port: u16 = host.strip_prefix("localhost:").unwrap().parse().unwrap();
            let mut socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let started = Instant::now();
            if body {
                socket.write_all(format!("POST /run HTTP/1.1\r\nHost: {host}\r\nOrigin: http://{host}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 70\r\n\r\ntoken=").as_bytes()).unwrap();
            } else {
                socket.write_all(b"GET /").unwrap();
            }
            for byte in *b"xy" {
                std::thread::sleep(Duration::from_millis(800));
                socket.write_all(&[byte]).unwrap();
            }
            let mut refused = String::new();
            socket
                .read_to_string(&mut refused)
                .expect("late bytes must not renew the request deadline");
            assert!(started.elapsed() < Duration::from_millis(2800));
            assert!(refused.starts_with("HTTP/1.1 400"));
            assert!(!refused.contains("<html>retained</html>"));
            assert!(post(preview.url(), "close").contains("Preview closed."));
            preview.wait();
        }
    }

    #[test]
    fn preview_runner_response_deadline_releases_a_slow_client_before_server_expiry() {
        let _guard = SERVERS.lock().unwrap_or_else(|error| error.into_inner());
        let bytes = vec![b' '; super::super::MAX_FILE_BYTES as usize];
        let preview = start_for(selected(&bytes), Duration::from_secs(15)).unwrap();
        let (host, token) = parts(preview.url());
        let port: u16 = host.strip_prefix("localhost:").unwrap().parse().unwrap();
        let mut blocked = TcpStream::connect(("127.0.0.1", port)).unwrap();
        blocked
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let body = format!("token={token}");
        blocked.write_all(format!("POST /run HTTP/1.1\r\nHost: {host}\r\nOrigin: http://{host}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",body.len()).as_bytes()).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let mut closer = TcpStream::connect(("127.0.0.1", port)).unwrap();
        closer
            .set_read_timeout(Some(Duration::from_secs(12)))
            .unwrap();
        let started = Instant::now();
        closer.write_all(format!("POST /close HTTP/1.1\r\nHost: {host}\r\nOrigin: http://{host}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",body.len()).as_bytes()).unwrap();
        let mut closed = String::new();
        closer
            .read_to_string(&mut closed)
            .expect("the response deadline must admit the queued close before server expiry");
        assert!(started.elapsed() >= Duration::from_secs(8));
        assert!(started.elapsed() < Duration::from_secs(12));
        assert!(closed.contains("Preview closed."));
        let mut partial = Vec::new();
        blocked.read_to_end(&mut partial).unwrap();
        assert!(partial.starts_with(b"HTTP/1.1 200"));
        assert!(
            partial.len() < bytes.len(),
            "the client must actually block a partial response"
        );
        preview.wait();
    }

    #[test]
    fn preview_runner_survives_handle_drop_and_expires_admission() {
        let _guard = SERVERS.lock().unwrap_or_else(|error| error.into_inner());
        let preview = start_for(
            selected(b"<html>retained</html>"),
            Duration::from_millis(300),
        )
        .unwrap();
        let url = preview.url().to_owned();
        drop(preview);
        let (_, token) = parts(&url);
        assert!(request(&url, "GET", &format!("/{token}"), "", "").starts_with("HTTP/1.1 200"));
        std::thread::sleep(Duration::from_millis(500));
        let (host, _) = parts(&url);
        let port: u16 = host.strip_prefix("localhost:").unwrap().parse().unwrap();
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    }

    #[test]
    fn preview_runner_limits_four_views_and_releases_capacity_on_close() {
        let _guard = SERVERS.lock().unwrap_or_else(|error| error.into_inner());
        let previews: Vec<_> = (0..4)
            .map(|_| start_for(selected(b"<html>retained</html>"), Duration::from_secs(2)).unwrap())
            .collect();
        assert!(start_for(selected(b"<html>excess</html>"), Duration::from_secs(2)).is_err());
        for preview in previews {
            post(preview.url(), "close");
            preview.wait();
        }
        let next = start_for(selected(b"<html>next</html>"), Duration::from_secs(2)).unwrap();
        post(next.url(), "close");
        next.wait();
    }
}

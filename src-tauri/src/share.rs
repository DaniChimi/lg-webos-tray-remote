//! Temporary sharing of a PC file with the TV.
//!
//! Starts a small HTTP server on the LAN that serves **only** the chosen file, under a
//! random-token path. The TV opens it from its browser. The server stops after
//! one hour, or when another file is shared.

use std::{
    collections::hash_map::RandomState,
    fs,
    hash::{BuildHasher, Hasher},
    io::{Read, Seek, SeekFrom, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Request log (to see what the TV does): %TEMP%	vtray-share.log
fn log(line: &str) {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let path = std::env::temp_dir().join("tvtray-share.log");
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "[{secs}] {line}");
    }
}

static CURRENT: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

const LIFETIME: Duration = Duration::from_secs(60 * 60);

fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("mp4") | Some("m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        Some("mp3") => "audio/mpeg",
        Some("m4a") => "audio/mp4",
        Some("wav") => "audio/wav",
        Some("pdf") => "application/pdf",
        Some("txt") => "text/plain; charset=utf-8",
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Token that cannot be guessed on the LAN: system random seed + time.
fn token() -> String {
    let mut h = RandomState::new().build_hasher();
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    h.write_u128(nanos);
    let a = h.finish();
    let mut h2 = RandomState::new().build_hasher();
    h2.write_u64(a);
    format!("{:016x}{:016x}", a, h2.finish())
}

/// Parses `Range: bytes=a-b`. Returns (start, inclusive end) or None if invalid.
fn parse_range(value: &str, len: u64) -> Option<(u64, u64)> {
    let spec = value.trim().strip_prefix("bytes=")?;
    let (a, b) = spec.split_once('-')?;
    if a.is_empty() {
        // last N bytes
        let n: u64 = b.parse().ok()?;
        if n == 0 || len == 0 {
            return None;
        }
        return Some((len.saturating_sub(n), len - 1));
    }
    let start: u64 = a.parse().ok()?;
    let end: u64 = if b.is_empty() { len.checked_sub(1)? } else { b.parse::<u64>().ok()?.min(len.checked_sub(1)?) };
    (start <= end && start < len).then_some((start, end))
}

fn respond(s: &mut TcpStream, status: &str, extra: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(body);
}

fn handle(mut s: TcpStream, path: &Path, prefix: &str, mime: &str, len: u64) {
    let peer = s.peer_addr().map(|a| a.to_string()).unwrap_or_default();
    log(&format!("connection from {peer}"));
    let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
    let mut buf = [0u8; 4096];
    let n = match s.read(&mut buf) {
        Ok(n) if n > 0 => n,
        _ => {
            log(&format!("{peer}: no request received (connection closed or timed out)"));
            return;
        }
    };
    let req = String::from_utf8_lossy(&buf[..n]).to_string();
    log(&format!("{peer}: request:
{}", req.trim_end()));
    let mut lines = req.lines();
    let first = lines.next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));

    if !(method == "GET" || method == "HEAD") || !target.starts_with(prefix) {
        respond(&mut s, "404 Not Found", "", b"");
        return;
    }
    let range_hdr = lines.find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case("range").then(|| v.trim().to_string())
    });

    let (start, end, partial) = match range_hdr {
        Some(r) => match parse_range(&r, len) {
            Some((a, b)) => (a, b, true),
            None => {
                let extra = format!("Content-Range: bytes */{len}\r\n");
                respond(&mut s, "416 Range Not Satisfiable", &extra, b"");
                return;
            }
        },
        None => (0, len.saturating_sub(1), false),
    };

    let size = if len == 0 { 0 } else { end - start + 1 };
    let status = if partial { "206 Partial Content" } else { "200 OK" };
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {size}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n"
    );
    if partial {
        head.push_str(&format!("Content-Range: bytes {start}-{end}/{len}\r\n"));
    }
    head.push_str("\r\n");
    log(&format!("{peer}: response {status}, {size} bytes, {mime}"));
    if s.write_all(head.as_bytes()).is_err() || method == "HEAD" || size == 0 {
        return;
    }

    let Ok(mut f) = fs::File::open(path) else { return };
    if f.seek(SeekFrom::Start(start)).is_err() {
        return;
    }
    let mut remaining = size;
    let mut sent: u64 = 0;
    let mut chunk = vec![0u8; 64 * 1024];
    while remaining > 0 {
        let want = chunk.len().min(remaining as usize);
        match f.read(&mut chunk[..want]) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if s.write_all(&chunk[..n]).is_err() {
                    break; // the TV closed the connection (normal with videos)
                }
                remaining -= n as u64;
                sent += n as u64;
            }
        }
    }
    log(&format!("{peer}: sent {sent} of {size} bytes"));
}

/// Starts the sharing and returns the address to give to the TV.
pub fn start(path: PathBuf, local_ip: Ipv4Addr) -> Result<String, String> {
    let meta = fs::metadata(&path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err(crate::i18n::t("not_file"));
    }
    let len = meta.len();

    // stop any previous sharing
    if let Some(old) = CURRENT.lock().unwrap().take() {
        old.store(true, Ordering::SeqCst);
    }

    let listener = TcpListener::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();

    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string();
    let prefix = format!("/{}/", token());
    let url = format!("http://{local_ip}:{port}{prefix}{}", percent_encode(&name));

    log(&format!("sharing started: {url} (file {}, {len} bytes)", path.display()));
    let stop = Arc::new(AtomicBool::new(false));
    *CURRENT.lock().unwrap() = Some(stop.clone());

    let mime = mime_for(&path);
    thread::spawn(move || {
        let deadline = Instant::now() + LIFETIME;
        while !stop.load(Ordering::SeqCst) && Instant::now() < deadline {
            match listener.accept() {
                Ok((conn, _)) => {
                    let _ = conn.set_nonblocking(false);
                    let (p, pre) = (path.clone(), prefix.clone());
                    thread::spawn(move || handle(conn, &p, &pre, mime, len));
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(_) => break,
            }
        }
    });
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=900-5000", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("bytes=50-10", 1000), None);
        assert_eq!(parse_range("items=0-1", 1000), None);
    }

    #[test]
    fn encoding() {
        assert_eq!(percent_encode("a b&è.mp4"), "a%20b%26%C3%A8.mp4");
    }

    #[test]
    fn tokens_differ() {
        assert_ne!(token(), token());
        assert_eq!(token().len(), 32);
    }

    #[test]
    fn serves_only_with_token_and_supports_range() {
        use std::net::TcpStream;
        let dir = std::env::temp_dir().join("tvtray-share-test");
        let _ = fs::create_dir_all(&dir);
        let file = dir.join("test.txt");
        fs::write(&file, b"0123456789").unwrap();
        let url = start(file, Ipv4Addr::LOCALHOST).unwrap();
        let rest = url.strip_prefix("http://").unwrap();
        let (hostport, path) = rest.split_once('/').unwrap();
        let path = format!("/{path}");

        let get = |p: &str, extra: &str| -> String {
            let mut s = TcpStream::connect(hostport).unwrap();
            s.write_all(format!("GET {p} HTTP/1.1\r\nHost: x\r\n{extra}\r\n").as_bytes()).unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            out
        };
        let full = get(&path, "");
        assert!(full.starts_with("HTTP/1.1 200 OK"), "{full}");
        assert!(full.ends_with("0123456789"));
        let part = get(&path, "Range: bytes=2-4\r\n");
        assert!(part.starts_with("HTTP/1.1 206"), "{part}");
        assert!(part.contains("Content-Range: bytes 2-4/10"));
        assert!(part.ends_with("234"));
        let wrong = get("/wrong-token/test.txt", "");
        assert!(wrong.starts_with("HTTP/1.1 404"), "{wrong}");
    }
}

//! Download of the screenshot image from the TV (HTTPS with a self-signed certificate).

use std::{
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    time::Duration,
};

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Decodes an HTTP body sent with `Transfer-Encoding: chunked`.
fn dechunk(mut b: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let i = find(b, b"\r\n")?;
        let line = std::str::from_utf8(&b[..i]).ok()?;
        let size = usize::from_str_radix(line.split(';').next()?.trim(), 16).ok()?;
        b = &b[i + 2..];
        if size == 0 {
            return Some(out);
        }
        if b.len() < size {
            return None;
        }
        out.extend_from_slice(&b[..size]);
        b = &b[size..];
        if b.starts_with(b"\r\n") {
            b = &b[2..];
        }
    }
}

/// Splits the headers from the body of an HTTP response; returns the body if the status is 200.
fn parse_response(raw: &[u8]) -> Result<Vec<u8>, String> {
    let split = find(raw, b"\r\n\r\n").ok_or_else(|| crate::i18n::t("http_incomplete"))?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let status = head.lines().next().unwrap_or("");
    if !status.contains(" 200") {
        return Err(format!("la TV ha risposto: {status}"));
    }
    let body = &raw[split + 4..];
    if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
        dechunk(body).ok_or_else(|| crate::i18n::t("body_invalid"))
    } else {
        Ok(body.to_vec())
    }
}

pub fn https_get(url: &str) -> Result<Vec<u8>, String> {
    let rest = url.strip_prefix("https://").ok_or_else(|| crate::i18n::t("not_https"))?;
    let (hostport, path) = rest
        .split_once('/')
        .map(|(h, p)| (h, format!("/{p}")))
        .unwrap_or((rest, "/".to_string()));
    let host = hostport.split(':').next().unwrap_or(hostport);
    let addr = if hostport.contains(':') { hostport.to_string() } else { format!("{hostport}:443") };
    let sock = addr
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or_else(|| crate::i18n::t("unresolvable"))?;

    let tcp = TcpStream::connect_timeout(&sock, Duration::from_secs(5)).map_err(|e| e.to_string())?;
    tcp.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
    tcp.set_write_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;

    let connector = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .build()
        .map_err(|e| e.to_string())?;
    let mut tls = connector.connect(host, tcp).map_err(|e| e.to_string())?;

    let req = format!("GET {path} HTTP/1.1\r\nHost: {hostport}\r\nConnection: close\r\n\r\n");
    tls.write_all(req.as_bytes()).map_err(|e| e.to_string())?;

    let mut raw = Vec::new();
    // Some servers close without close_notify: if we already have data, the error is ignored.
    if let Err(e) = tls.read_to_end(&mut raw) {
        if raw.is_empty() {
            return Err(e.to_string());
        }
    }
    parse_response(&raw)
}

/// Local date and time for the file name: `20261013-153045`.
#[cfg(windows)]
pub fn local_stamp() -> String {
    #[repr(C)]
    struct SystemTime {
        year: u16,
        month: u16,
        dow: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        millis: u16,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(st: *mut SystemTime);
    }
    let mut st = SystemTime { year: 0, month: 0, dow: 0, day: 0, hour: 0, minute: 0, second: 0, millis: 0 };
    // SAFETY: `st` is a valid, writable SYSTEMTIME structure.
    unsafe { GetLocalTime(&mut st) };
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        st.year, st.month, st.day, st.hour, st.minute, st.second
    )
}

#[cfg(not(windows))]
pub fn local_stamp() -> String {
    let s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_body() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc";
        assert_eq!(parse_response(raw).unwrap(), b"abc");
    }

    #[test]
    fn chunked_body() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n";
        assert_eq!(parse_response(raw).unwrap(), b"abcde");
    }

    #[test]
    fn non_200_is_error() {
        let raw = b"HTTP/1.1 404 Not Found\r\n\r\n";
        assert!(parse_response(raw).is_err());
    }

    #[test]
    fn truncated_chunk_is_error() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n9\r\nabc";
        assert!(parse_response(raw).is_err());
    }
}

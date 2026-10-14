//! Ricerca delle TV LG webOS sulla rete locale.
//!
//! 1. SSDP: richiesta multicast mirata al servizio "second screen" di LG
//!    (le TV compatibili rispondono, nessuna scansione).
//! 2. Ripiego: connessione TCP alla porta 3001 degli host della sottorete /24.

use serde::Serialize;
use std::{
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

const SSDP_ADDR: &str = "239.255.255.250:1900";
const LG_ST: &str = "urn:lge-com:service:webos-second-screen:1";

#[derive(Serialize, Clone)]
pub struct FoundTv {
    pub ip: String,
    /// Altri indirizzi dello stesso dispositivo (es. TV con Wi-Fi e cavo attivi).
    pub also: Vec<String>,
    pub mac: Option<String>,
    pub name: Option<String>,
    pub model: Option<String>,
    /// "ssdp" se ha risposto alla ricerca LG, "porta 3001" se trovata dal ripiego.
    pub via: &'static str,
}

/// Chiede al sistema il MAC dell'indirizzo IP indicato (richiesta ARP sulla LAN).
/// Funziona solo se il dispositivo è acceso e nella stessa sottorete; blocca fino a qualche secondo.
#[cfg(windows)]
pub fn arp_lookup(ip: Ipv4Addr) -> Option<[u8; 6]> {
    #[link(name = "iphlpapi")]
    extern "system" {
        fn SendARP(dest: u32, src: u32, mac: *mut u8, len: *mut u32) -> u32;
    }
    let mut buf = [0u8; 8];
    let mut len: u32 = buf.len() as u32;
    let dest = u32::from_ne_bytes(ip.octets());
    // SAFETY: `buf` e `len` restano validi per tutta la chiamata; `len` dichiara la capienza di `buf`.
    let rc = unsafe { SendARP(dest, 0, buf.as_mut_ptr(), &mut len) };
    if rc != 0 || len < 6 {
        return None;
    }
    let mac = [buf[0], buf[1], buf[2], buf[3], buf[4], buf[5]];
    (mac != [0u8; 6]).then_some(mac)
}

#[cfg(not(windows))]
pub fn arp_lookup(_ip: Ipv4Addr) -> Option<[u8; 6]> {
    None
}

pub fn fmt_mac(m: [u8; 6]) -> String {
    m.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
}

/// IP del PC sull'interfaccia che raggiunge `target` (nessun pacchetto viene inviato).
pub fn local_ip_towards(target: Ipv4Addr) -> Option<Ipv4Addr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect((target, 3001)).ok()?;
    match s.local_addr().ok()?.ip() {
        IpAddr::V4(v) => Some(v),
        IpAddr::V6(_) => None,
    }
}

/// IP del PC sull'interfaccia di default (nessun pacchetto viene inviato).
fn local_ipv4() -> Option<Ipv4Addr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    match s.local_addr().ok()?.ip() {
        IpAddr::V4(v) => Some(v),
        IpAddr::V6(_) => None,
    }
}

fn header(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_string())
    })
}

/// Restituisce (ip, location) per ogni dispositivo che risponde alla ricerca LG.
fn ssdp_search(window: Duration) -> Vec<(Ipv4Addr, Option<String>)> {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else { return vec![] };
    let _ = sock.set_read_timeout(Some(Duration::from_millis(400)));
    let req = format!(
        "M-SEARCH * HTTP/1.1\r\nHOST: {SSDP_ADDR}\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {LG_ST}\r\n\r\n"
    );
    // UDP può perdere pacchetti: due invii.
    for _ in 0..2 {
        let _ = sock.send_to(req.as_bytes(), SSDP_ADDR);
    }

    let mut found: Vec<(Ipv4Addr, Option<String>)> = vec![];
    let mut buf = [0u8; 2048];
    let start = Instant::now();
    while start.elapsed() < window {
        let Ok((n, src)) = sock.recv_from(&mut buf) else { continue };
        let IpAddr::V4(ip) = src.ip() else { continue };
        let text = String::from_utf8_lossy(&buf[..n]).to_string();
        let lower = text.to_ascii_lowercase();
        if !(lower.contains("webos") || lower.contains("lge")) {
            continue;
        }
        if !found.iter().any(|(i, _)| *i == ip) {
            found.push((ip, header(&text, "LOCATION")));
        }
    }
    found
}

/// GET HTTP minimale (solo per leggere la descrizione UPnP della TV).
fn http_get(addr: SocketAddr, host: &str, path: &str) -> Option<String> {
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(1500)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes()).ok()?;
    let mut out = String::new();
    s.take(64 * 1024).read_to_string(&mut out).ok()?;
    Some(out)
}

fn xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&format!("</{tag}>"))? + start;
    let v = xml[start..end].trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// Legge nome e modello dalla descrizione UPnP indicata da LOCATION.
fn describe(ip: Ipv4Addr, location: &str) -> (Option<String>, Option<String>) {
    let rest = location.strip_prefix("http://").unwrap_or(location);
    let (hostport, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, "/".into()));
    let port = hostport.split_once(':').and_then(|(_, p)| p.parse::<u16>().ok()).unwrap_or(80);
    match http_get(SocketAddr::new(ip.into(), port), hostport, &path) {
        Some(xml) => (xml_tag(&xml, "friendlyName"), xml_tag(&xml, "modelName")),
        None => (None, None),
    }
}

/// Ripiego: host della /24 con la porta 3001 aperta (escluso il PC stesso).
fn scan_port_3001(local: Ipv4Addr) -> Vec<Ipv4Addr> {
    let o = local.octets();
    let hosts: Vec<u8> = (1..=254u8).filter(|h| *h != o[3]).collect();
    let found = Mutex::new(Vec::new());
    let found_ref = &found;
    thread::scope(|s| {
        // 32 thread da 8 host: la scansione completa dura circa 3 secondi.
        for chunk in hosts.chunks(8) {
            s.spawn(move || {
                for h in chunk {
                    let ip = Ipv4Addr::new(o[0], o[1], o[2], *h);
                    let addr = SocketAddr::new(ip.into(), 3001);
                    if TcpStream::connect_timeout(&addr, Duration::from_millis(400)).is_ok() {
                        found_ref.lock().unwrap().push(ip);
                    }
                }
            });
        }
    });
    let mut v = found.into_inner().unwrap();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prova su rete reale: `cargo test live_discovery -- --nocapture --ignored`.
    #[test]
    #[ignore]
    fn live_discovery() {
        println!("IP locale: {:?}", local_ipv4());
        let t = Instant::now();
        for f in discover(Some("192.168.0.83")) {
            println!(
                "trovata: {} (anche {:?}) | mac {:?} | {:?} | {:?} | via {}",
                f.ip, f.also, f.mac, f.name, f.model, f.via
            );
        }
        println!("durata: {:?}", t.elapsed());
    }

    #[test]
    fn parses_headers_and_xml() {
        let r = "HTTP/1.1 200 OK\r\nLocation: http://1.2.3.4:1/d.xml\r\nST: x\r\n\r\n";
        assert_eq!(header(r, "LOCATION").as_deref(), Some("http://1.2.3.4:1/d.xml"));
        let x = "<root><friendlyName> [LG] TV </friendlyName><modelName>OLED</modelName></root>";
        assert_eq!(xml_tag(x, "friendlyName").as_deref(), Some("[LG] TV"));
        assert_eq!(xml_tag(x, "modelName").as_deref(), Some("OLED"));
        assert_eq!(xml_tag(x, "missing"), None);
    }
}

/// Cerca TV LG. `preferred` è l'IP già configurato: se la TV ha più indirizzi
/// (Wi-Fi e cavo) viene proposto quello come principale.
pub fn discover(preferred: Option<&str>) -> Vec<FoundTv> {
    let new = |ip: Ipv4Addr, name, model, via| FoundTv {
        ip: ip.to_string(),
        also: vec![],
        mac: None,
        name,
        model,
        via,
    };

    let mut found: Vec<FoundTv> = ssdp_search(Duration::from_secs(2))
        .into_iter()
        .map(|(ip, loc)| {
            let (name, model) = loc.map(|l| describe(ip, &l)).unwrap_or((None, None));
            new(ip, name, model, "ssdp")
        })
        .collect();

    if found.is_empty() {
        if let Some(local) = local_ipv4() {
            found = scan_port_3001(local)
                .into_iter()
                .map(|ip| new(ip, None, None, "porta 3001"))
                .collect();
        }
    }

    // Una TV con Wi-Fi e cavo risponde da due indirizzi con lo stesso MAC:
    // si raggruppa per MAC, così compare una sola volta.
    let mut groups: Vec<FoundTv> = vec![];
    for mut f in found {
        f.mac = f.ip.parse().ok().and_then(arp_lookup).map(fmt_mac);
        let same = f
            .mac
            .as_ref()
            .and_then(|m| groups.iter().position(|g| g.mac.as_ref() == Some(m)));
        match same {
            Some(i) => {
                let g = &mut groups[i];
                if preferred == Some(f.ip.as_str()) {
                    let old = std::mem::replace(&mut g.ip, f.ip.clone());
                    g.also.push(old);
                } else {
                    g.also.push(f.ip.clone());
                }
                g.name = g.name.take().or(f.name.take());
                g.model = g.model.take().or(f.model.take());
            }
            None => groups.push(f),
        }
    }
    groups
}

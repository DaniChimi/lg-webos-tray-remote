//! Native-side messages (tray menu, errors shown in the panel).
//! The web interface has its own dictionary in `ui/index.html`.

use std::sync::RwLock;

static LANG: RwLock<&'static str> = RwLock::new("en");

/// Sets the language. Any code that does not start with `it` becomes English.
pub fn set(code: &str) {
    let lang = if code.to_ascii_lowercase().starts_with("it") { "it" } else { "en" };
    *LANG.write().unwrap() = lang;
}

pub fn current() -> &'static str {
    *LANG.read().unwrap()
}

/// Windows UI language (for example `it-IT`), empty if it cannot be detected.
#[cfg(windows)]
pub fn system_lang() -> String {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetUserDefaultLocaleName(buf: *mut u16, len: i32) -> i32;
    }
    let mut b = [0u16; 85];
    let n = unsafe { GetUserDefaultLocaleName(b.as_mut_ptr(), b.len() as i32) };
    if n > 1 {
        String::from_utf16_lossy(&b[..(n - 1) as usize])
    } else {
        String::new()
    }
}

#[cfg(not(windows))]
pub fn system_lang() -> String {
    std::env::var("LANG").unwrap_or_default()
}

/// Localized text; if the key does not exist it returns the key itself.
pub fn t(key: &str) -> String {
    let table = if current() == "it" { it(key) } else { en(key) };
    table.or_else(|| en(key)).unwrap_or(key).to_string()
}

/// Like `t`, replacing the `{name}` placeholders.
pub fn tf(key: &str, vars: &[(&str, &str)]) -> String {
    let mut s = t(key);
    for (k, v) in vars {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

fn it(key: &str) -> Option<&'static str> {
    Some(match key {
        "ip_invalid" => "indirizzo IP non valido",
        "mac_undetected" => "MAC non rilevato: la riaccensione da PC non è possibile",
        "tv_no_image" => "la TV non ha restituito l'immagine (contenuto protetto?)",
        "file_missing" => "file non trovato",
        "file_too_big" => "file troppo grande ({mb} MB): il limite è 40 MB",
        "image_unreadable" => "immagine non leggibile: {err}",
        "tv_ip_unset" => "IP della TV non impostato",
        "pc_ip_unknown" => "indirizzo del PC sulla rete non trovato",
        "pointer_invalid" => "tipo di puntatore non valido",
        "mac_format" => "MAC non valido (atteso aa:bb:cc:dd:ee:ff)",
        "mac_invalid" => "MAC non valido",
        "mac_unset" => "MAC non impostato: serve per accendere la TV",
        "wol_failed" => "invio del pacchetto non riuscito",
        "tray_open" => "Apri pannello",
        "tray_off" => "Spegni TV",
        "tray_quit" => "Esci",
        "tray_tooltip" => "Controllo TV LG",
        "dlg_images" => "Immagini",
        "pairing_closed" => "connessione chiusa durante il pairing",
        "pairing_expired" => "pairing scaduto: accetta il prompt sulla TV",
        "connect_timeout" => "TV non raggiungibile (timeout)",
        "not_connected" => "non connesso",
        "conn_closed" => "connessione chiusa",
        "tv_timeout" => "timeout risposta dalla TV",
        "no_socket" => "socketPath assente nella risposta",
        "pointer_failed" => "invio al puntatore non riuscito",
        "http_incomplete" => "risposta HTTP incompleta",
        "body_invalid" => "corpo della risposta non valido",
        "not_https" => "l'indirizzo dell'immagine non è https",
        "unresolvable" => "indirizzo non risolvibile",
        "not_file" => "il percorso scelto non è un file",
        _ => return None,
    })
}

fn en(key: &str) -> Option<&'static str> {
    Some(match key {
        "ip_invalid" => "invalid IP address",
        "mac_undetected" => "MAC address not detected: powering on from the PC is not possible",
        "tv_no_image" => "the TV did not return the image (protected content?)",
        "file_missing" => "file not found",
        "file_too_big" => "file too large ({mb} MB): the limit is 40 MB",
        "image_unreadable" => "unreadable image: {err}",
        "tv_ip_unset" => "TV IP address not set",
        "pc_ip_unknown" => "could not find the PC's address on the network",
        "pointer_invalid" => "invalid pointer type",
        "mac_format" => "invalid MAC address (expected aa:bb:cc:dd:ee:ff)",
        "mac_invalid" => "invalid MAC address",
        "mac_unset" => "MAC address not set: it is needed to power on the TV",
        "wol_failed" => "could not send the packet",
        "tray_open" => "Open panel",
        "tray_off" => "Turn TV off",
        "tray_quit" => "Quit",
        "tray_tooltip" => "LG TV control",
        "dlg_images" => "Images",
        "pairing_closed" => "connection closed during pairing",
        "pairing_expired" => "pairing timed out: accept the prompt on the TV",
        "connect_timeout" => "TV unreachable (timeout)",
        "not_connected" => "not connected",
        "conn_closed" => "connection closed",
        "tv_timeout" => "the TV did not reply in time",
        "no_socket" => "socketPath missing from the reply",
        "pointer_failed" => "could not send to the pointer",
        "http_incomplete" => "incomplete HTTP response",
        "body_invalid" => "invalid response body",
        "not_https" => "the image address is not https",
        "unresolvable" => "address could not be resolved",
        "not_file" => "the selected path is not a file",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_italian_key_exists_in_english() {
        for k in [
            "ip_invalid", "mac_undetected", "tv_no_image", "file_missing", "file_too_big", "image_unreadable",
            "tv_ip_unset", "pc_ip_unknown", "pointer_invalid", "mac_format", "mac_invalid", "mac_unset",
            "wol_failed", "tray_open", "tray_off", "tray_quit", "tray_tooltip", "dlg_images", "pairing_closed",
            "pairing_expired", "connect_timeout", "not_connected", "conn_closed", "tv_timeout", "no_socket",
            "pointer_failed", "http_incomplete", "body_invalid", "not_https", "unresolvable", "not_file",
        ] {
            assert!(it(k).is_some() && en(k).is_some(), "missing key: {k}");
        }
    }

    #[test]
    fn placeholders_are_replaced() {
        set("en");
        assert_eq!(tf("file_too_big", &[("mb", "50")]), "file too large (50 MB): the limit is 40 MB");
    }
}

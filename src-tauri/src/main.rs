#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod discovery;
mod dpapi;
mod share;
mod ssap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, PhysicalPosition, Rect, State, WebviewWindow, WindowEvent,
};

// ---------------------------------------------------------------- configurazione

#[derive(Serialize, Deserialize, Clone)]
struct Config {
    ip: String,
    mac: Option<String>,
    client_key: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config { ip: "192.168.0.83".into(), mac: None, client_key: None }
    }
}

struct AppState {
    ssap: tokio::sync::Mutex<ssap::Ssap>,
    cfg: Mutex<Config>,
    file: PathBuf,
}

/// Formato su disco: la chiave di pairing è cifrata con DPAPI (solo questo utente
/// Windows, su questo PC, può leggerla).
#[derive(Serialize, Deserialize, Default)]
struct DiskConfig {
    ip: Option<String>,
    mac: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_key_enc: Option<String>,
    /// Compare solo nei vecchi file (chiave in chiaro) o se DPAPI non è disponibile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_key: Option<String>,
}

/// Legge la configurazione. Il booleano indica che il file va riscritto
/// (vecchio formato con la chiave in chiaro).
fn load_config(file: &std::path::Path) -> (Config, bool) {
    let parsed = fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str::<DiskConfig>(&t).ok());
    let Some(d) = parsed else { return (Config::default(), false) };

    let mut rewrite = false;
    let key = match (&d.client_key_enc, &d.client_key) {
        (Some(enc), _) => dpapi::open(enc), // non decifrabile: servirà un nuovo pairing
        (None, Some(plain)) => {
            rewrite = true;
            Some(plain.clone())
        }
        (None, None) => None,
    };
    let cfg = Config {
        ip: d.ip.unwrap_or_else(|| Config::default().ip),
        mac: d.mac,
        client_key: key,
    };
    (cfg, rewrite)
}

impl AppState {
    fn persist(&self) -> Result<(), String> {
        let cfg = self.cfg.lock().unwrap().clone();
        if let Some(dir) = self.file.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let sealed = cfg.client_key.as_deref().and_then(dpapi::seal);
        let disk = DiskConfig {
            ip: Some(cfg.ip.clone()),
            mac: cfg.mac.clone(),
            // se la cifratura non è disponibile si conserva il valore, per non perdere il pairing
            client_key: if sealed.is_none() { cfg.client_key.clone() } else { None },
            client_key_enc: sealed,
        };
        let text = serde_json::to_string_pretty(&disk).map_err(|e| e.to_string())?;
        fs::write(&self.file, text).map_err(|e| e.to_string())
    }
}

/// Dati di configurazione visibili all'interfaccia (la chiave di pairing no).
#[derive(Serialize)]
struct PublicConfig {
    ip: String,
    mac: Option<String>,
    paired: bool,
    /// Percorso del file di configurazione sul disco.
    file: String,
}

fn public(cfg: &Config, file: &std::path::Path) -> PublicConfig {
    PublicConfig {
        ip: cfg.ip.clone(),
        mac: cfg.mac.clone(),
        paired: cfg.client_key.is_some(),
        file: file.display().to_string(),
    }
}

// ---------------------------------------------------------------- comandi

#[tauri::command]
fn get_config(state: State<'_, AppState>) -> PublicConfig {
    public(&state.cfg.lock().unwrap(), &state.file)
}

#[tauri::command]
async fn save_config(
    state: State<'_, AppState>,
    ip: String,
    mac: Option<String>,
) -> Result<PublicConfig, String> {
    let ip = ip.trim().to_string();
    if ip.parse::<std::net::Ipv4Addr>().is_err() {
        return Err("indirizzo IP non valido".into());
    }
    let mac = mac.map(|m| m.trim().to_string()).filter(|m| !m.is_empty());
    if let Some(m) = &mac {
        parse_mac(m)?;
    }
    let (key, out) = {
        let mut cfg = state.cfg.lock().unwrap();
        if cfg.ip != ip {
            cfg.client_key = None; // la chiave vale solo per quella TV
        }
        cfg.ip = ip.clone();
        cfg.mac = mac;
        (cfg.client_key.clone(), public(&cfg, &state.file))
    };
    state.persist()?;
    state.ssap.lock().await.set_target(ip, key);
    Ok(out)
}

/// Disaccoppia l'app dalla TV: dimentica chiave di pairing, IP e MAC.
/// La TV non viene toccata: la voce dell'app resta nel suo elenco dispositivi.
#[tauri::command]
async fn unpair(state: State<'_, AppState>) -> Result<PublicConfig, String> {
    let out = {
        let mut cfg = state.cfg.lock().unwrap();
        cfg.client_key = None;
        cfg.mac = None;
        cfg.ip = String::new();
        public(&cfg, &state.file)
    };
    state.persist()?;
    state.ssap.lock().await.set_target(String::new(), None);
    Ok(out)
}

/// Salva su disco la chiave di pairing se la connessione ne ha ottenuta una nuova.
fn sync_key(state: &AppState, s: &mut ssap::Ssap) -> Result<(), String> {
    if s.key_dirty {
        state.cfg.lock().unwrap().client_key = s.client_key.clone();
        s.key_dirty = false;
        state.persist()?;
    }
    Ok(())
}

#[tauri::command]
async fn pair(state: State<'_, AppState>) -> Result<PublicConfig, String> {
    let mut s = state.ssap.lock().await;
    s.disconnect();
    s.connect().await?;
    sync_key(&state, &mut s)?;
    drop(s);
    // La TV è raggiungibile: è il momento buono per memorizzare il MAC.
    remember_mac(&state).await;
    Ok(public(&state.cfg.lock().unwrap(), &state.file))
}

#[tauri::command]
async fn tv_request(
    state: State<'_, AppState>,
    uri: String,
    payload: Option<Value>,
) -> Result<Value, String> {
    let mut s = state.ssap.lock().await;
    let res = s.request(&uri, payload.unwrap_or_else(|| json!({}))).await;
    sync_key(&state, &mut s)?;
    res
}

#[tauri::command]
async fn tv_button(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let mut s = state.ssap.lock().await;
    let res = s.button(&name).await;
    sync_key(&state, &mut s)?;
    res
}

use discovery::{arp_lookup, fmt_mac};

/// Rileva il MAC attuale della TV e, se è nuovo o diverso, lo salva nella configurazione.
/// Restituisce il MAC valido (appena rilevato oppure quello già salvato).
async fn remember_mac(state: &AppState) -> Option<String> {
    let ip: std::net::Ipv4Addr = state.cfg.lock().unwrap().ip.parse().ok()?;
    let found = tauri::async_runtime::spawn_blocking(move || arp_lookup(ip))
        .await
        .ok()
        .flatten()
        .map(fmt_mac);
    if let Some(mac) = found {
        let changed = state.cfg.lock().unwrap().mac.as_deref() != Some(mac.as_str());
        if changed {
            state.cfg.lock().unwrap().mac = Some(mac.clone());
            let _ = state.persist();
        }
        return Some(mac);
    }
    state.cfg.lock().unwrap().mac.clone()
}

#[derive(Serialize)]
struct PowerResult {
    mac: Option<String>,
    /// Presente se la riaccensione da remoto non è garantita.
    warning: Option<String>,
}

/// Aggiorna il MAC e poi spegne la TV: ordine voluto, perché da spenta non è più rilevabile.
async fn do_power_off(state: &AppState) -> Result<PowerResult, String> {
    let mac = remember_mac(state).await;
    let warning = if mac.is_none() {
        Some("MAC non rilevato: la riaccensione da PC non è possibile".to_string())
    } else {
        None
    };
    let mut s = state.ssap.lock().await;
    let res = s.request("ssap://system/turnOff", json!({})).await;
    sync_key(state, &mut s)?;
    res?;
    Ok(PowerResult { mac, warning })
}

#[tauri::command]
async fn power_off(state: State<'_, AppState>) -> Result<PowerResult, String> {
    do_power_off(&state).await
}

/// Cattura lo schermo della TV e salva il JPEG in Immagini\TV Screenshots.
/// Passa dal servizio di cattura della TV (SSAP), senza SSH.
#[tauri::command]
async fn screenshot(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let reply = {
        let mut s = state.ssap.lock().await;
        let r = s
            .request(
                "ssap://tv/executeOneShot",
                json!({ "path": "/tmp/tvtray.jpg", "method": "DISPLAY", "format": "JPEG",
                        "width": 1920, "height": 1080 }),
            )
            .await;
        sync_key(&state, &mut s)?;
        r?
    };
    let uri = reply["imageUri"]
        .as_str()
        .ok_or("la TV non ha restituito l'immagine (contenuto protetto?)")?
        .to_string();
    let bytes = tauri::async_runtime::spawn_blocking(move || capture::https_get(&uri))
        .await
        .map_err(|e| e.to_string())??;

    let dir = app
        .path()
        .picture_dir()
        .map_err(|e| e.to_string())?
        .join("TV Screenshots");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join(format!("tv-{}.jpg", capture::local_stamp()));
    fs::write(&file, &bytes).map_err(|e| e.to_string())?;
    Ok(file.display().to_string())
}

/// Mostra un file in Esplora risorse.
#[tauri::command]
fn reveal_file(path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.is_file() {
        return Err("file non trovato".into());
    }
    std::process::Command::new("explorer")
        .arg(format!("/select,{}", p.display()))
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Sceglie un file e lo fa aprire alla TV: il PC lo serve sulla rete locale per
/// un'ora e la TV lo carica nel suo browser.
/// Riduce l'immagine a 1920x1080 al massimo e la salva come JPEG temporaneo da condividere.
fn prepare_image(src: &std::path::Path) -> Result<std::path::PathBuf, String> {
    use image::{imageops::FilterType, GenericImageView, ImageReader};
    const MAX_W: u32 = 1920;
    const MAX_H: u32 = 1080;
    const MAX_BYTES: u64 = 40 * 1024 * 1024;
    let len = std::fs::metadata(src).map_err(|e| e.to_string())?.len();
    if len > MAX_BYTES {
        return Err(format!("file troppo grande ({} MB): il limite è 40 MB", len / (1024 * 1024)));
    }
    let img = ImageReader::open(src)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .decode()
        .map_err(|e| format!("immagine non leggibile: {e}"))?;
    let (w, h) = img.dimensions();
    let img = if w > MAX_W || h > MAX_H { img.resize(MAX_W, MAX_H, FilterType::Triangle) } else { img };
    let dir = std::env::temp_dir().join("tvtray-share");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stem = src.file_stem().and_then(|n| n.to_str()).unwrap_or("immagine");
    let out = dir.join(format!("{stem}.jpg"));
    let file = std::fs::File::create(&out).map_err(|e| e.to_string())?;
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(file), 88);
    img.to_rgb8().write_with_encoder(enc).map_err(|e| e.to_string())?;
    Ok(out)
}

#[tauri::command]
async fn send_file(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    use tauri_plugin_dialog::DialogExt;

    let tv_ip: std::net::Ipv4Addr = state
        .cfg
        .lock()
        .unwrap()
        .ip
        .parse()
        .map_err(|_| "IP della TV non impostato")?;

    let dialog_app = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        dialog_app
            .dialog()
            .file()
            .add_filter("Immagini", &["jpg", "jpeg", "png"])
            .blocking_pick_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(picked) = picked else { return Ok(String::new()) }; // annullato

    let path = picked.into_path().map_err(|e| e.to_string())?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string();
    // Una foto da 23 megapixel (anche se pesa 1,4 MB) ha bloccato la TV: la riduco prima di
    // condividerla. La TV è 4K al massimo, 1920x1080 basta e avanza per mostrarla.
    let src = path.clone();
    let path = tauri::async_runtime::spawn_blocking(move || prepare_image(&src))
        .await
        .map_err(|e| e.to_string())??;
    let local = discovery::local_ip_towards(tv_ip).ok_or("indirizzo del PC sulla rete non trovato")?;
    let url = share::start(path, local)?;

    let mut s = state.ssap.lock().await;
    // `system.launcher/open` con un indirizzo del PC blocca il browser della TV: si lancia
    // il browser in modo esplicito, come ha fatto il caso verificato con lo spike.
    let r = s
        .request(
            "ssap://system.launcher/launch",
            json!({ "id": "com.webos.app.browser", "params": { "target": url } }),
        )
        .await;
    sync_key(&state, &mut s)?;
    r?;
    Ok(name)
}

#[tauri::command]
fn get_autostart(app: AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    let launcher = app.autolaunch();
    if enabled {
        launcher.enable().map_err(|e| e.to_string())?;
    } else {
        launcher.disable().map_err(|e| e.to_string())?;
    }
    launcher.is_enabled().map_err(|e| e.to_string())
}

/// Puntatore del Magic Remote: `kind` è "move", "click" o "scroll".
#[tauri::command]
async fn tv_pointer(
    state: State<'_, AppState>,
    kind: String,
    dx: Option<i32>,
    dy: Option<i32>,
) -> Result<(), String> {
    // Valori limitati: un movimento anomalo non deve poter inondare la TV.
    let (dx, dy) = (dx.unwrap_or(0).clamp(-200, 200), dy.unwrap_or(0).clamp(-200, 200));
    let mut s = state.ssap.lock().await;
    let res = match kind.as_str() {
        "move" => s.pointer_move(dx, dy).await,
        "click" => s.pointer_click().await,
        "scroll" => s.pointer_scroll(dy).await,
        _ => Err("tipo di puntatore non valido".to_string()),
    };
    sync_key(&state, &mut s)?;
    res
}

fn parse_mac(text: &str) -> Result<[u8; 6], String> {
    let parts: Vec<&str> = text.split(|c| c == ':' || c == '-').collect();
    if parts.len() != 6 {
        return Err("MAC non valido (atteso aa:bb:cc:dd:ee:ff)".into());
    }
    let mut out = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        out[i] = u8::from_str_radix(p, 16).map_err(|_| "MAC non valido".to_string())?;
    }
    Ok(out)
}

/// Cerca TV LG webOS sulla rete locale (SSDP, con ripiego sulla porta 3001).
#[tauri::command]
async fn discover_tvs(state: State<'_, AppState>) -> Result<Vec<discovery::FoundTv>, String> {
    // Se la TV ha più indirizzi (Wi-Fi e cavo) si preferisce quello già configurato.
    let preferred = state.cfg.lock().unwrap().ip.clone();
    tauri::async_runtime::spawn_blocking(move || discovery::discover(Some(&preferred)))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn wake(state: State<'_, AppState>) -> Result<(), String> {
    let mac = state
        .cfg
        .lock()
        .unwrap()
        .mac
        .clone()
        .ok_or("MAC non impostato: serve per accendere la TV")?;
    let mac = parse_mac(&mac)?;
    let mut packet = vec![0xFFu8; 6];
    for _ in 0..16 {
        packet.extend_from_slice(&mac);
    }
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    sock.set_broadcast(true).map_err(|e| e.to_string())?;

    // Broadcast generale e broadcast della sottorete (si assume una /24), porte 9 e 7.
    let mut targets = vec!["255.255.255.255".to_string()];
    if let Ok(ip) = state.cfg.lock().unwrap().ip.parse::<std::net::Ipv4Addr>() {
        let o = ip.octets();
        targets.push(format!("{}.{}.{}.255", o[0], o[1], o[2]));
    }
    let mut sent = false;
    for t in &targets {
        for port in [9u16, 7] {
            sent |= sock.send_to(&packet, (t.as_str(), port)).is_ok();
        }
    }
    if sent { Ok(()) } else { Err("invio del pacchetto non riuscito".into()) }
}

// ---------------------------------------------------------------- pannello vicino alla tray

static LAST_BLUR: Mutex<Option<Instant>> = Mutex::new(None);

fn place(w: &WebviewWindow, rect: Option<Rect>) {
    let scale = w.scale_factor().unwrap_or(1.0);
    let Ok(size) = w.outer_size() else { return };
    let (win_w, win_h) = (size.width as f64, size.height as f64);
    let margin = 8.0 * scale;

    let monitor = w.current_monitor().ok().flatten().or_else(|| w.primary_monitor().ok().flatten());
    let (mx, my, mw, mh) = match &monitor {
        Some(m) => (
            m.position().x as f64,
            m.position().y as f64,
            m.size().width as f64,
            m.size().height as f64,
        ),
        None => (0.0, 0.0, 1920.0, 1080.0),
    };

    let (mut x, mut y) = match rect {
        Some(r) => {
            let p = r.position.to_physical::<f64>(scale);
            let s = r.size.to_physical::<f64>(scale);
            (p.x + s.width / 2.0 - win_w / 2.0, p.y - win_h - margin)
        }
        // Senza icona di riferimento: angolo in basso a destra, sopra la barra.
        None => (mx + mw - win_w - margin, my + mh - win_h - 56.0 * scale),
    };
    x = x.clamp(mx + margin, (mx + mw - win_w - margin).max(mx + margin));
    y = y.clamp(my + margin, (my + mh - win_h - margin).max(my + margin));
    let _ = w.set_position(PhysicalPosition::new(x as i32, y as i32));
}

fn show_panel(app: &AppHandle, rect: Option<Rect>) {
    let Some(w) = app.get_webview_window("main") else { return };
    place(&w, rect);
    let _ = w.show();
    let _ = w.set_focus();
    // `set_focus` della finestra non dà il focus della tastiera alla pagina web:
    // senza questo le caselle di testo del pannello non ricevono i tasti.
    let webview: &tauri::Webview = w.as_ref();
    let _ = webview.set_focus();
}

fn toggle_panel(app: &AppHandle, rect: Rect) {
    let Some(w) = app.get_webview_window("main") else { return };
    if w.is_visible().unwrap_or(false) {
        let _ = w.hide();
        return;
    }
    // Il click sull'icona toglie prima il focus alla finestra (che si nasconde):
    // senza questo controllo il pannello si richiuderebbe e riaprirebbe subito.
    let just_blurred = LAST_BLUR
        .lock()
        .unwrap()
        .map(|t| t.elapsed() < Duration::from_millis(300))
        .unwrap_or(false);
    if !just_blurred {
        show_panel(app, Some(rect));
    }
}

fn config_file(app: &tauri::App) -> PathBuf {
    app.path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("config.json")
}

fn main() {
    tauri::Builder::default()
        // Deve essere il primo plugin: una seconda copia apre il pannello della prima.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_panel(app, None);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            let file = config_file(app);
            let (cfg, rewrite) = load_config(&file);
            // Senza chiave di pairing la TV non è ancora collegata: al primo avvio
            // si apre il pannello, che avvia da solo la ricerca.
            let first_run = cfg.client_key.is_none();
            let mut s = ssap::Ssap::default();
            s.set_target(cfg.ip.clone(), cfg.client_key.clone());
            app.manage(AppState {
                ssap: tokio::sync::Mutex::new(s),
                cfg: Mutex::new(cfg),
                file,
            });
            if rewrite {
                // migrazione: la chiave in chiaro viene sostituita da quella cifrata
                let _ = app.state::<AppState>().persist();
            }

            let open = MenuItem::with_id(app, "open", "Apri pannello", true, None::<&str>)?;
            let off = MenuItem::with_id(app, "off", "Spegni TV", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Esci", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &off, &quit])?;

            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().cloned().ok_or("icona mancante")?)
                .tooltip("Controllo TV LG")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_panel(app, None),
                    "off" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let state = app.state::<AppState>();
                            let _ = do_power_off(&state).await;
                        });
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } = event
                    {
                        toggle_panel(tray.app_handle(), rect);
                    }
                })
                .build(app)?;
            if first_run {
                show_panel(app.handle(), None);
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::Focused(false) if window.label() == "main" => {
                // Quando il focus passa dalla finestra al webview Windows genera
                // Focused(false) seguito, pochi ms dopo, da Focused(true). Si nasconde
                // solo se il focus non è tornato: è un vero click fuori dal pannello.
                let w = window.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    if !w.is_focused().unwrap_or(false) {
                        *LAST_BLUR.lock().unwrap() = Some(Instant::now());
                        let _ = w.hide();
                    }
                });
            }
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            get_config, save_config, pair, tv_request, tv_button, wake, power_off, discover_tvs, unpair,
            tv_pointer, get_autostart, set_autostart, screenshot, reveal_file, send_file
        ])
        .run(tauri::generate_context!())
        .expect("errore nell'avvio dell'app");
}

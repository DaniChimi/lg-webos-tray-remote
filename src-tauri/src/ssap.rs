//! Client SSAP (WebSocket) per TV LG webOS.
//!
//! Una sola connessione persistente, serializzata da un mutex nel chiamante.
//! Se la connessione cade viene riaperta una volta in modo trasparente.

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::{net::TcpStream, time::timeout};
use tokio_tungstenite::{
    connect_async_tls_with_config, tungstenite::Message, Connector, MaybeTlsStream, WebSocketStream,
};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Manifest dei permessi (da lgtv2, licenza MIT). Senza, la TV risponde 401.
const PAIRING: &str = include_str!("pairing.json");

/// I messaggi di errore che arrivano dalla TV iniziano con questo prefisso:
/// non vanno ritentati, a differenza degli errori di trasporto.
const TV_ERR: &str = "TV: ";

fn tls() -> Result<Connector, String> {
    // La TV usa un certificato autofirmato: l'accettazione è limitata alla LAN
    // dell'indirizzo configurato dall'utente.
    let c = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(Connector::NativeTls(c))
}

async fn open(url: &str) -> Result<Ws, String> {
    let fut = connect_async_tls_with_config(url, None, false, Some(tls()?));
    let (ws, _) = timeout(Duration::from_secs(5), fut)
        .await
        .map_err(|_| "TV non raggiungibile (timeout)".to_string())?
        .map_err(|e| e.to_string())?;
    Ok(ws)
}

async fn register(ws: &mut Ws, key: &Option<String>) -> Result<String, String> {
    let mut payload: Value = serde_json::from_str(PAIRING).map_err(|e| e.to_string())?;
    if let Some(k) = key {
        payload["client-key"] = json!(k);
    }
    let msg = json!({ "type": "register", "id": "register_0", "payload": payload });
    ws.send(msg.to_string().into()).await.map_err(|e| e.to_string())?;

    // Al primo pairing l'utente deve accettare il prompt sulla TV.
    let wait = if key.is_some() { 10 } else { 60 };
    let fut = async {
        while let Some(m) = ws.next().await {
            let m = m.map_err(|e| e.to_string())?;
            let Message::Text(t) = m else { continue };
            let v: Value = serde_json::from_str(t.as_ref()).map_err(|e| e.to_string())?;
            if v["id"].as_str() != Some("register_0") {
                continue;
            }
            match v["type"].as_str() {
                Some("registered") => {
                    return Ok(v["payload"]["client-key"].as_str().unwrap_or_default().to_string())
                }
                Some("error") => return Err(format!("{TV_ERR}{}", v["error"])),
                _ => {} // "response": prompt mostrato, in attesa
            }
        }
        Err("connessione chiusa durante il pairing".to_string())
    };
    timeout(Duration::from_secs(wait), fut)
        .await
        .map_err(|_| "pairing scaduto: accetta il prompt sulla TV".to_string())?
}

#[derive(Default)]
pub struct Ssap {
    pub ip: String,
    pub client_key: Option<String>,
    /// Vero se la chiave è cambiata e va salvata su disco.
    pub key_dirty: bool,
    ws: Option<Ws>,
    pointer: Option<Ws>,
    counter: u64,
}

impl Ssap {
    pub fn set_target(&mut self, ip: String, client_key: Option<String>) {
        self.ip = ip;
        self.client_key = client_key;
        self.disconnect();
    }

    pub fn disconnect(&mut self) {
        self.ws = None;
        self.pointer = None;
    }

    pub async fn connect(&mut self) -> Result<(), String> {
        if self.ip.trim().is_empty() {
            return Err("IP della TV non impostato".into());
        }
        let mut ws = open(&format!("wss://{}:3001", self.ip.trim())).await?;
        let key = register(&mut ws, &self.client_key).await?;
        if self.client_key.as_deref() != Some(key.as_str()) {
            self.client_key = Some(key);
            self.key_dirty = true;
        }
        self.ws = Some(ws);
        Ok(())
    }

    pub async fn request(&mut self, uri: &str, payload: Value) -> Result<Value, String> {
        let mut last = String::new();
        for _ in 0..2 {
            if self.ws.is_none() {
                self.connect().await?;
            }
            match self.try_request(uri, &payload).await {
                Ok(v) => return Ok(v),
                Err(e) if e.starts_with(TV_ERR) => return Err(e),
                Err(e) => {
                    last = e;
                    self.disconnect();
                }
            }
        }
        Err(last)
    }

    async fn try_request(&mut self, uri: &str, payload: &Value) -> Result<Value, String> {
        self.counter += 1;
        let id = format!("req_{}", self.counter);
        let ws = self.ws.as_mut().ok_or("non connesso")?;
        let msg = json!({ "type": "request", "id": id, "uri": uri, "payload": payload });
        ws.send(msg.to_string().into()).await.map_err(|e| e.to_string())?;

        let fut = async {
            loop {
                match ws.next().await {
                    Some(Ok(Message::Text(t))) => {
                        let v: Value = serde_json::from_str(t.as_ref()).map_err(|e| e.to_string())?;
                        if v["id"].as_str() == Some(id.as_str()) {
                            if v["type"].as_str() == Some("error") {
                                return Err(format!("{TV_ERR}{}", v["error"]));
                            }
                            return Ok(v["payload"].clone());
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e.to_string()),
                    None => return Err("connessione chiusa".to_string()),
                }
            }
        };
        timeout(Duration::from_secs(8), fut)
            .await
            .map_err(|_| "timeout risposta dalla TV".to_string())?
    }

    /// Simula un tasto del telecomando (UP, DOWN, LEFT, RIGHT, ENTER, BACK, HOME, ...).
    pub async fn button(&mut self, name: &str) -> Result<(), String> {
        self.pointer_send(format!("type:button\nname:{name}\n\n")).await
    }

    /// Movimento relativo del puntatore (come il Magic Remote).
    pub async fn pointer_move(&mut self, dx: i32, dy: i32) -> Result<(), String> {
        self.pointer_send(format!("type:move\ndx:{dx}\ndy:{dy}\n\n")).await
    }

    pub async fn pointer_click(&mut self) -> Result<(), String> {
        self.pointer_send("type:click\n\n".to_string()).await
    }

    pub async fn pointer_scroll(&mut self, dy: i32) -> Result<(), String> {
        self.pointer_send(format!("type:scroll\ndx:0\ndy:{dy}\n\n")).await
    }

    /// Invia un messaggio sul socket di input, aprendolo se serve.
    async fn pointer_send(&mut self, text: String) -> Result<(), String> {
        for _ in 0..2 {
            if self.pointer.is_none() {
                let r = self
                    .request("ssap://com.webos.service.networkinput/getPointerInputSocket", json!({}))
                    .await?;
                let url = r["socketPath"].as_str().ok_or("socketPath assente nella risposta")?;
                self.pointer = Some(open(url).await?);
            }
            let res = self.pointer.as_mut().unwrap().send(text.clone().into()).await;
            match res {
                Ok(()) => return Ok(()),
                Err(_) => self.pointer = None, // riapre il socket e riprova
            }
        }
        Err("invio al puntatore non riuscito".into())
    }
}

//! SSAP (WebSocket) client for LG webOS TVs.
//!
//! A single persistent connection, serialized by a mutex in the caller.
//! If the connection drops it is reopened once, transparently.

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::{net::TcpStream, time::timeout};
use tokio_tungstenite::{
    connect_async_tls_with_config, tungstenite::Message, Connector, MaybeTlsStream, WebSocketStream,
};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Permission manifest (from lgtv2, MIT licence). Without it the TV answers 401.
const PAIRING: &str = include_str!("pairing.json");

/// Error messages that come from the TV start with this prefix:
/// they must not be retried, unlike transport errors.
const TV_ERR: &str = "TV: ";

fn tls() -> Result<Connector, String> {
    // The TV uses a self-signed certificate: acceptance is limited to the LAN
    // of the address configured by the user.
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
        .map_err(|_| crate::i18n::t("connect_timeout"))?
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

    // On the first pairing the user must accept the prompt on the TV.
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
                _ => {} // "response": prompt shown, waiting
            }
        }
        Err(crate::i18n::t("pairing_closed"))
    };
    timeout(Duration::from_secs(wait), fut)
        .await
        .map_err(|_| crate::i18n::t("pairing_expired"))?
}

#[derive(Default)]
pub struct Ssap {
    pub ip: String,
    pub client_key: Option<String>,
    /// True if the key changed and must be saved to disk.
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
            return Err(crate::i18n::t("tv_ip_unset"));
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
        let ws = self.ws.as_mut().ok_or_else(|| crate::i18n::t("not_connected"))?;
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
                    None => return Err(crate::i18n::t("conn_closed")),
                }
            }
        };
        timeout(Duration::from_secs(8), fut)
            .await
            .map_err(|_| crate::i18n::t("tv_timeout"))?
    }

    /// Simulates a remote-control key (UP, DOWN, LEFT, RIGHT, ENTER, BACK, HOME, ...).
    pub async fn button(&mut self, name: &str) -> Result<(), String> {
        self.pointer_send(format!("type:button\nname:{name}\n\n")).await
    }

    /// Relative pointer movement (like the Magic Remote).
    pub async fn pointer_move(&mut self, dx: i32, dy: i32) -> Result<(), String> {
        self.pointer_send(format!("type:move\ndx:{dx}\ndy:{dy}\n\n")).await
    }

    pub async fn pointer_click(&mut self) -> Result<(), String> {
        self.pointer_send("type:click\n\n".to_string()).await
    }

    pub async fn pointer_scroll(&mut self, dy: i32) -> Result<(), String> {
        self.pointer_send(format!("type:scroll\ndx:0\ndy:{dy}\n\n")).await
    }

    /// Sends a message on the input socket, opening it if needed.
    async fn pointer_send(&mut self, text: String) -> Result<(), String> {
        for _ in 0..2 {
            if self.pointer.is_none() {
                let r = self
                    .request("ssap://com.webos.service.networkinput/getPointerInputSocket", json!({}))
                    .await?;
                let url = r["socketPath"].as_str().ok_or_else(|| crate::i18n::t("no_socket"))?;
                self.pointer = Some(open(url).await?);
            }
            let res = self.pointer.as_mut().unwrap().send(text.clone().into()).await;
            match res {
                Ok(()) => return Ok(()),
                Err(_) => self.pointer = None, // reopens the socket and retries
            }
        }
        Err(crate::i18n::t("pointer_failed"))
    }
}

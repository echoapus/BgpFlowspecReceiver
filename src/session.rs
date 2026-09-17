use crate::{app::Shared, wire};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{net::Ipv4Addr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    time::{sleep, timeout, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub local_as: u32,
    pub router_id: Ipv4Addr,
    pub peer_ip: Ipv4Addr,
    pub peer_as: u32,
    pub hold_time: u16,
    pub reconnect_delay: f64,
    pub connect_timeout: f64,
    pub active_retry_delay: f64,
    pub listen_port: u16,
    pub json_output: Option<String>,
}
impl Config {
    pub fn from_value(mut value: Value) -> Result<Self, String> {
        let map = value.as_object_mut().ok_or("Config must be an object")?;
        for (key, default) in [
            ("hold_time", json!(90)),
            ("reconnect_delay", json!(5)),
            ("connect_timeout", json!(5.0)),
            ("active_retry_delay", json!(1.0)),
            ("listen_port", json!(179)),
        ] {
            map.entry(key).or_insert(default);
        }
        for key in [
            "local_as",
            "peer_as",
            "hold_time",
            "reconnect_delay",
            "connect_timeout",
            "active_retry_delay",
            "listen_port",
        ] {
            if let Some(s) = map.get(key).and_then(Value::as_str) {
                let parsed: Value =
                    serde_json::from_str(s).map_err(|_| format!("Invalid {key}"))?;
                if !parsed.is_number() {
                    return Err(format!("Invalid {key}"));
                }
                map.insert(key.into(), parsed);
            }
        }
        if map.get("json_output") == Some(&json!("")) {
            map.insert("json_output".into(), Value::Null);
        }
        let config: Self = serde_json::from_value(value).map_err(|e| e.to_string())?;
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.local_as == 0 || self.peer_as == 0 {
            return Err("AS numbers must be positive".into());
        }
        if self.hold_time != 0 && self.hold_time < 3 {
            return Err("Hold time must be zero or at least 3 seconds".into());
        }
        if self.listen_port == 0 {
            return Err("Listen port must be positive".into());
        }
        for (name, value, allow_zero) in [
            ("reconnect_delay", self.reconnect_delay, true),
            ("connect_timeout", self.connect_timeout, false),
            ("active_retry_delay", self.active_retry_delay, false),
        ] {
            if !value.is_finite() || value < 0.0 || (!allow_zero && value == 0.0) || value > 86400.0
            {
                return Err(format!(
                    "Invalid {name}: expected {}..86400 seconds",
                    if allow_zero { "0" } else { "positive" }
                ));
            }
        }
        Ok(())
    }
}

async fn incoming(listener: &Option<TcpListener>, config: &Config) -> TcpStream {
    let Some(listener) = listener else {
        return std::future::pending().await;
    };
    loop {
        match listener.accept().await {
            Ok((stream, peer)) if peer.ip() == config.peer_ip => return stream,
            Ok(_) => {}
            Err(_) => sleep(Duration::from_millis(100)).await,
        }
    }
}
async fn active(config: &Config) -> TcpStream {
    loop {
        if let Ok(Ok(stream)) = timeout(
            Duration::from_secs_f64(config.connect_timeout),
            TcpStream::connect((config.peer_ip, 179)),
        )
        .await
        {
            return stream;
        }
        sleep(Duration::from_secs_f64(config.active_retry_delay)).await;
    }
}

pub async fn run(app: Shared, config: Config, mut stop: oneshot::Receiver<()>) {
    let listener = match TcpListener::bind((Ipv4Addr::UNSPECIFIED, config.listen_port)).await {
        Ok(l) => {
            app.emit(
                "session",
                "info",
                format!("Passive listener on :{}", config.listen_port),
                json!({}),
            );
            Some(l)
        }
        Err(e) => {
            app.emit(
                "session",
                "warning",
                format!("Passive listener disabled: {e}"),
                json!({}),
            );
            None
        }
    };
    let mut queued = None;
    loop {
        app.set_state("CONNECT");
        let stream = if let Some(stream) = queued.take() {
            stream
        } else {
            tokio::select! {biased;
                _=&mut stop=>break,
                stream=incoming(&listener,&config)=>stream,
                stream=active(&config)=>stream,
            }
        };
        let result = {
            let session = connected(&app, &config, stream);
            tokio::pin!(session);
            loop {
                tokio::select! {biased;
                    _=&mut stop=>break None,
                    result=&mut session=>break Some(result),
                    duplicate=incoming(&listener,&config)=>{
                        drop(duplicate);
                        app.emit("session","warning","Duplicate passive connection dropped",json!({}));
                    }
                }
            }
        };
        {
            let mut data = app.data.lock().unwrap();
            data.clear();
            data.peer_info = json!({});
        }
        app.set_state("IDLE");
        let Some(result) = result else { break };
        if let Err(e) = result {
            app.emit("error", "error", e, json!({}));
        }
        tokio::select! {biased;
            _=&mut stop=>break,
            stream=incoming(&listener,&config)=>queued=Some(stream),
            _=sleep(Duration::from_secs_f64(config.reconnect_delay))=>{},
        }
    }
    {
        let mut data = app.data.lock().unwrap();
        data.running = false;
        data.state = "IDLE";
        data.clear();
        data.peer_info = json!({});
    }
    if let Err(e) = app.flush(true) {
        app.emit("error", "error", e, json!({}));
    }
}

async fn read_message(
    reader: &mut tokio::net::tcp::OwnedReadHalf,
) -> Result<(u8, Vec<u8>), String> {
    let mut header = [0; 19];
    reader
        .read_exact(&mut header)
        .await
        .map_err(|e| e.to_string())?;
    let (kind, len) = wire::header(&header)?;
    let mut body = vec![0; len];
    reader
        .read_exact(&mut body)
        .await
        .map_err(|e| e.to_string())?;
    Ok((kind, body))
}

async fn connected(app: &Shared, config: &Config, stream: TcpStream) -> Result<(), String> {
    stream.set_nodelay(true).map_err(|e| e.to_string())?;
    let (mut reader, mut writer) = stream.into_split();
    app.set_state("OPEN_SENT");
    writer
        .write_all(&wire::build_open(
            config.local_as,
            config.hold_time,
            config.router_id,
        ))
        .await
        .map_err(|e| e.to_string())?;
    let mut hold = config.hold_time;
    let mut received_open = false;
    let mut established = false;
    let mut asn_len = 2;
    let mut deadline =
        Instant::now() + Duration::from_secs(if hold == 0 { 86400 * 365 } else { hold as u64 });
    let mut next_keepalive = Instant::now() + Duration::from_secs(86400 * 365);
    loop {
        // Keep the frame read alive while sending keepalives; read_exact is not cancellation safe.
        let frame = read_message(&mut reader);
        tokio::pin!(frame);
        let (kind, body) = loop {
            tokio::select! {
                result=&mut frame=>break result?,
                _=tokio::time::sleep_until(deadline),if hold>0=>return Err("Hold timer expired".into()),
                _=tokio::time::sleep_until(next_keepalive),if received_open&&hold>0=>{
                    timeout(Duration::from_secs(hold as u64),writer.write_all(&wire::message(4,&[]))).await.map_err(|_|"Keepalive write timed out")?.map_err(|e|e.to_string())?;
                    next_keepalive=Instant::now()+Duration::from_secs((hold/3).max(1) as u64);
                }
            }
        };
        match kind {
            1 if !received_open => {
                let info = wire::open(&body)?;
                if info["peer_as"] != config.peer_as {
                    return Err("Peer ASN mismatch".into());
                }
                hold = hold.min(info["hold_time"].as_u64().unwrap() as u16);
                if info["supports_4byte_asn"] == true {
                    asn_len = 4;
                }
                app.data.lock().unwrap().peer_info = info.clone();
                app.emit(
                    "session",
                    "info",
                    "Received OPEN",
                    json!({"peer_info":info}),
                );
                writer
                    .write_all(&wire::message(4, &[]))
                    .await
                    .map_err(|e| e.to_string())?;
                received_open = true;
                app.set_state("OPEN_CONFIRMED");
                next_keepalive = Instant::now() + Duration::from_secs((hold / 3).max(1) as u64);
            }
            4 if received_open && body.is_empty() => {
                if !established {
                    established = true;
                    app.set_state("ESTABLISHED");
                }
            }
            2 if established => {
                app.apply_update(wire::update(&body, asn_len)?, &config.peer_ip.to_string())
            }
            3 => {
                return Err(format!(
                    "BGP NOTIFICATION {}/{}",
                    body.first().unwrap_or(&0),
                    body.get(1).unwrap_or(&0)
                ))
            }
            _ => return Err(format!("Unexpected BGP message {kind}")),
        }
        if hold > 0 {
            deadline = Instant::now() + Duration::from_secs(hold as u64);
        }
    }
}

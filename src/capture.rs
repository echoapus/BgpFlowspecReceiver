use crate::app::{bad, ApiError, App, Shared, Task};
use axum::{extract::State, Json};
use regex::Regex;
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::oneshot,
};

pub async fn start(State(app): State<Shared>) -> Result<Json<Value>, ApiError> {
    let mut control = app.control.lock().await;
    let peer = {
        let data = app.data.lock().unwrap();
        if !data.running {
            return Err(bad("No active session"));
        }
        data.config.as_ref().unwrap().peer_ip
    };
    App::stop_task(&mut control.capture).await;
    let mut child = Command::new("tcpdump")
        .args([
            "-l",
            "-nn",
            "-X",
            "-i",
            "any",
            &format!("host {peer} and port 179"),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(bad)?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (stop, mut rx) = oneshot::channel();
    app.data.lock().unwrap().capturing = true;
    app.emit(
        "capture",
        "info",
        "Capture started",
        json!({"running":true}),
    );
    let cloned = app.clone();
    control.capture = Some(Task {
        stop,
        join: tokio::spawn(async move {
            let app = cloned;
            let mut lines = BufReader::new(stdout).lines();
            let mut errors = BufReader::new(stderr).lines();
            let regex=Regex::new(r"\d{2}:\d{2}:\d{2}\.\d+.+?\s(In|Out)\s+IP\s([\d.]+)\.(\d+)\s+>\s+([\d.]+)\.(\d+):\s+Flags\s+\[([^\]]+)\].*?length\s+(\d+)").unwrap();
            let mut packet = None;
            let mut bytes = Vec::new();
            let mut stderr_open = true;
            loop {
                tokio::select! {
                    _=&mut rx=>break,
                    line=lines.next_line()=>match line{
                        Ok(Some(line))=>{
                            if let Some(c)=regex.captures(&line){
                                flush(&app,packet.take(),&bytes);bytes.clear();
                                let flags=match &c[6]{"S"=>"SYN","R"=>"RST","F"=>"FIN","P"=>"PSH","."=>"ACK","S."=>"SYN-ACK","F."=>"FIN-ACK","R."=>"RST-ACK","P."=>"PSH-ACK",f=>f};
                                let event=match &c[6]{"S"=>Some("TCP SYN - connection attempt"),"S."=>Some("TCP SYN-ACK - connection accepted"),"R"|"R."=>Some("TCP RST - connection refused/reset"),"F"|"F."=>Some("TCP FIN - connection closing"),_=>None};
                                packet=Some(json!({"src":format!("{}:{}",&c[2],&c[3]),"dst":format!("{}:{}",&c[4],&c[5]),"direction":if c[4]==peer.to_string(){"→"}else{"←"},"flags":flags,"length":c[7].parse::<u32>().unwrap_or(0),"tcp_event":event}));
                            }else if line.trim_start().starts_with("0x"){
                                if let Some((_,hex))=line.split_once(':'){
                                    for word in hex.split_whitespace().take(8){
                                        if ![2,4].contains(&word.len())||!word.bytes().all(|b|b.is_ascii_hexdigit()){break;}
                                        bytes.push(u8::from_str_radix(&word[..2],16).unwrap());
                                        if word.len()==4 {bytes.push(u8::from_str_radix(&word[2..],16).unwrap());}
                                    }
                                }
                            }
                        }
                        Ok(None)=>break,
                        Err(e)=>{app.emit("capture","error",e,json!({}));break;}
                    },
                    line=errors.next_line(),if stderr_open=>match line{
                        Ok(Some(line))=>app.emit("capture","info",line,json!({})),
                        _=>stderr_open=false,
                    }
                }
            }
            flush(&app, packet, &bytes);
            let _ = child.kill().await;
            let _ = child.wait().await;
            app.data.lock().unwrap().capturing = false;
            app.emit(
                "capture",
                "info",
                "Capture stopped",
                json!({"running":false}),
            );
        }),
    });
    Ok(Json(json!({"ok":true})))
}
fn flush(app: &Shared, packet: Option<Value>, bytes: &[u8]) {
    let Some(mut p) = packet else { return };
    let kind = bytes.windows(19).find_map(|w| {
        if w[..16] == [255; 16] {
            match w[18] {
                1 => Some("OPEN"),
                2 => Some("UPDATE"),
                3 => Some("NOTIFICATION"),
                4 => Some("KEEPALIVE"),
                _ => None,
            }
        } else {
            None
        }
    });
    p["bgp_type"] = json!(kind);
    let message = format!(
        "[{}] {} {} {} len={}{}",
        p["flags"].as_str().unwrap(),
        p["src"].as_str().unwrap(),
        p["direction"].as_str().unwrap(),
        p["dst"].as_str().unwrap(),
        p["length"],
        kind.map(|k| format!(" BGP {k}")).unwrap_or_default()
    );
    app.emit("capture", "packet", message, p);
}
pub async fn stop(State(app): State<Shared>) -> Json<Value> {
    App::stop_task(&mut app.control.lock().await.capture).await;
    Json(json!({"ok":true}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_event_contains_bgp_type() {
        let app = App::new(None);
        let packet = json!({"src":"192.0.2.1:179","dst":"192.0.2.2:40000","direction":"in","flags":"ACK","length":19,"tcp_event":null});
        let mut payload = vec![0; 40];
        payload.extend(crate::wire::message(4, &[]));
        flush(&app, Some(packet), &payload);
        let data = app.data.lock().unwrap();
        let event: Value = serde_json::from_str(data.history.back().unwrap()).unwrap();
        assert_eq!(event["type"], "capture");
        assert_eq!(event["bgp_type"], "KEEPALIVE");
        assert_eq!(event["src"], "192.0.2.1:179");
    }
}

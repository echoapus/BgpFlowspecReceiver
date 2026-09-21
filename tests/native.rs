use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use bgpx::{
    app::{router, App},
    session::Config,
    wire,
};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::{timeout, Duration},
};
use tower::ServiceExt;

#[test]
fn parser_compatibility_fixtures() {
    // Captured from the Python reference before removing the legacy backend.
    let cases: Vec<Value> = include_str!("fixtures/parser.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(cases.len(), 324);
    for (index, case) in cases.iter().enumerate() {
        let body: Vec<u8> = serde_json::from_value(case["body"].clone()).unwrap();
        let actual = if case["kind"] == "open" {
            wire::open(&body)
        } else {
            wire::update(&body, case["asn_len"].as_u64().unwrap() as usize)
        }
        .unwrap();
        assert_eq!(actual, case["expected"], "parser fixture {index}");
    }
}

fn config(port: u16) -> Config {
    Config::from_value(json!({"local_as":"65001","peer_as":"65000","router_id":"192.0.2.2","peer_ip":"127.0.0.1","listen_port":port,"hold_time":3,"reconnect_delay":0.01})).unwrap()
}
fn attr(code: u8, data: &[u8]) -> Vec<u8> {
    let mut a = vec![0x80, code, data.len() as u8];
    a.extend(data);
    a
}
fn update(attrs: &[u8], nlri: &[u8]) -> Vec<u8> {
    let mut b = vec![0, 0];
    b.extend_from_slice(&(attrs.len() as u16).to_be_bytes());
    b.extend(attrs);
    b.extend(nlri);
    b
}
async fn request(app: axum::Router, path: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[test]
fn parser_roundtrip_and_malformed_input() {
    let open = wire::build_open(4200000000, 90, "192.0.2.1".parse().unwrap());
    assert_eq!(wire::header(&open[..19]).unwrap(), (1, open.len() - 19));
    assert_eq!(wire::open(&open[19..]).unwrap()["peer_as"], 4200000000u32);
    for end in 0..open.len() - 19 {
        assert!(wire::open(&open[19..19 + end]).is_err());
    }
    let mut attrs = attr(3, &[192, 0, 2, 1]);
    attrs.extend(attr(8, &[255, 255, 255, 1]));
    attrs.extend(attr(99, &[1, 2]));
    let body = update(&attrs, &[24, 203, 0, 113]);
    let parsed = wire::update(&body, 2).unwrap();
    assert_eq!(
        parsed["announce"]["ipv4-unicast"][0],
        json!({"prefix":"203.0.113.0/24","next_hop":"192.0.2.1"})
    );
    assert_eq!(parsed["path_attributes"][1]["value"], json!(["NO_EXPORT"]));
    assert_eq!(parsed["path_attributes"][2]["raw"], "0102");
    assert!(wire::update(&body[..body.len() - 1], 2).is_err());
    assert!(wire::update(&body, 3).is_err());
}

#[test]
fn mixed_flowspec_and_redirect() {
    let nlri = [
        3, 0x81, 6, 5, 0x13, 4, 0, 0x95, 255, 255, 9, 1, 2, 0x83, 16, 11, 0x81, 255, 12, 0x80, 5,
    ];
    let mut mp = vec![0, 1, 133, 4, 192, 0, 2, 1, 0, nlri.len() as u8];
    mp.extend(nlri);
    let mut attrs = attr(14, &mp);
    attrs.extend(attr(16, &[8, 0, 0, 0, 0, 0, 0, 0]));
    let parsed = wire::update(&update(&attrs, &[]), 2).unwrap();
    assert_eq!(
        parsed["announce"]["ipv4-flowspec"][0],
        json!({"ip-proto":["=tcp(6)"],"dst-port":[">=1024","<=65535"],"tcp-flags":["all(syn)","not-all(ack)"],"dscp":["=63"],"fragment":["any(df,first-fragment)"]})
    );
    assert_eq!(parsed["actions"], json!(["redirect-to-ipv4=192.0.2.1"]));
    mp.pop();
    let body = update(&attr(14, &mp), &[]);
    assert!(wire::update(&body, 2).is_err());
}

#[test]
fn config_validation() {
    for value in [
        json!([]),
        json!({"local_as":0}),
        json!({"local_as":1,"peer_as":2,"router_id":"::1","peer_ip":"127.0.0.1"}),
    ] {
        assert!(Config::from_value(value).is_err());
    }
    let mut c = config(9179);
    c.hold_time = 1;
    assert!(c.validate().is_err());
    c.hold_time = 0;
    assert!(c.validate().is_ok());
    c.connect_timeout = f64::INFINITY;
    assert!(c.validate().is_err());
}

#[tokio::test]
async fn rib_http_stats_export_and_persistence() {
    let path = std::env::temp_dir().join(format!("bgpx-native-{}.json", std::process::id()));
    let app = App::new(Some(path.to_string_lossy().into_owned()));
    let body = update(
        &attr(3, &[192, 0, 2, 1]),
        &[24, 203, 0, 113, 24, 198, 51, 100],
    );
    app.apply_update(wire::update(&body, 2).unwrap(), "192.0.2.1");
    let api = router(app.clone());
    let (_, page) = request(api.clone(), "/routes?page_size=1&sort=prefix&order=asc").await;
    assert_eq!(page["count"], 2);
    assert_eq!(page["routes"][0]["prefix"], "198.51.100.0/24");
    assert_eq!(page["stats"]["unicast"], 2);
    app.apply_update(wire::update(&body, 2).unwrap(), "192.0.2.1");
    assert_eq!(app.data.lock().unwrap().stats()["total"], 2);
    let (_, export) = request(api.clone(), "/routes/export?family=unicast").await;
    assert_eq!(export["routes"].as_array().unwrap().len(), 2);
    assert_eq!(
        request(api.clone(), "/routes?family=invalid").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(api.clone(), "/health").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    app.flush(true).unwrap();
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["count"], 2);
    let withdraw = [0, 4, 24, 203, 0, 113, 0, 0];
    app.apply_update(wire::update(&withdraw, 2).unwrap(), "192.0.2.1");
    assert_eq!(app.data.lock().unwrap().stats()["total"], 1);
    app.stop().await;
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["count"], 0);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn search_longest_prefix_match() {
    let app = App::new(None);
    let body = update(
        &attr(3, &[192, 0, 2, 1]),
        &[24, 203, 0, 113, 25, 203, 0, 113, 128],
    );
    app.apply_update(wire::update(&body, 2).unwrap(), "192.0.2.1");
    let api = router(app.clone());

    let (status, res) = request(api.clone(), "/routes/search?ip=203.0.113.130").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(res["match"], true);
    assert_eq!(res["length"], 25);
    assert_eq!(res["route"]["prefix"], "203.0.113.128/25");
    assert_eq!(res["route"]["peer"], "192.0.2.1");
    assert_eq!(res["route"]["afi"], "ipv4-unicast");

    let (_, res) = request(api.clone(), "/routes/search?ip=203.0.113.10").await;
    assert_eq!(res["match"], true);
    assert_eq!(res["length"], 24);
    assert_eq!(res["route"]["prefix"], "203.0.113.0/24");

    let (_, res) = request(api.clone(), "/routes/search?ip=8.8.8.8").await;
    assert_eq!(res["match"], false);

    assert_eq!(
        request(api.clone(), "/routes/search?ip=not-an-ip").await.0,
        StatusCode::BAD_REQUEST
    );
}

async fn read_frame(stream: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut h = [0; 19];
    timeout(Duration::from_secs(3), stream.read_exact(&mut h))
        .await
        .unwrap()
        .unwrap();
    let (kind, len) = wire::header(&h).unwrap();
    let mut body = vec![0; len];
    stream.read_exact(&mut body).await.unwrap();
    (kind, body)
}
async fn wait_state(app: &bgpx::app::Shared, state: &str) {
    timeout(Duration::from_secs(3), async {
        loop {
            if app.data.lock().unwrap().state == state {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn passive_bgp_handshake_update_withdraw_and_stop() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let app = App::new(None);
    app.start(config(port)).await.unwrap();
    let mut peer = timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(s) = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await {
                break s;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(read_frame(&mut peer).await.0, 1);
    peer.write_all(&wire::build_open(65000, 3, "192.0.2.1".parse().unwrap()))
        .await
        .unwrap();
    assert_eq!(read_frame(&mut peer).await.0, 4);
    peer.write_all(&wire::message(4, &[])).await.unwrap();
    wait_state(&app, "ESTABLISHED").await;
    let body = update(&attr(3, &[127, 0, 0, 1]), &[24, 203, 0, 113]);
    // A partial frame must survive the keepalive timer firing mid-read.
    let msg = wire::message(2, &body);
    peer.write_all(&msg[..8]).await.unwrap();
    assert_eq!(read_frame(&mut peer).await.0, 4);
    peer.write_all(&msg[8..]).await.unwrap();
    timeout(Duration::from_secs(2), async {
        loop {
            if app.data.lock().unwrap().stats()["total"] == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        request(router(app.clone()), "/health").await.0,
        StatusCode::OK
    );
    let mut duplicate = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
    let mut b = [0];
    assert_eq!(
        timeout(Duration::from_secs(2), duplicate.read(&mut b))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    peer.write_all(&wire::message(2, &[0, 4, 24, 203, 0, 113, 0, 0]))
        .await
        .unwrap();
    timeout(Duration::from_secs(2), async {
        loop {
            if app.data.lock().unwrap().stats()["total"] == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    timeout(Duration::from_secs(2), app.stop()).await.unwrap();
    assert!(!app.data.lock().unwrap().running);
    let rebound = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
    drop(rebound);
}

#[tokio::test]
async fn sse_snapshot_replay_and_shutdown() {
    let app = App::new(None);
    app.emit("session", "info", "history marker", json!({}));
    let response = router(app.clone())
        .oneshot(
            Request::builder()
                .uri("/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    app.shutdown.send_replace(true);
    let body = timeout(
        Duration::from_secs(2),
        to_bytes(response.into_body(), 100_000),
    )
    .await
    .unwrap()
    .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    let events: Vec<Value> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(events[0]["type"], "snapshot");
    assert_eq!(events[0]["status"]["running"], false);
    assert_eq!(events[1]["message"], "history marker");
    assert_eq!(events[1]["replayed"], true);
}

#[tokio::test]
async fn hold_expiry_and_passive_reconnect() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let app = App::new(None);
    let mut cfg = config(port);
    cfg.reconnect_delay = 30.0;
    app.start(cfg).await.unwrap();
    let connect = || async {
        timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(s) = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await {
                    break s;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    };
    let mut peer = connect().await;
    assert_eq!(read_frame(&mut peer).await.0, 1);
    peer.write_all(&wire::build_open(65000, 3, "192.0.2.1".parse().unwrap()))
        .await
        .unwrap();
    assert_eq!(read_frame(&mut peer).await.0, 4);
    peer.write_all(&wire::message(4, &[])).await.unwrap();
    wait_state(&app, "ESTABLISHED").await;
    timeout(Duration::from_secs(5), async {
        loop {
            if app.data.lock().unwrap().history.iter().any(|e| {
                serde_json::from_str::<Value>(e).unwrap()["message"] == "Hold timer expired"
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(app.data.lock().unwrap().state, "IDLE");
    let mut second = connect().await;
    assert_eq!(read_frame(&mut second).await.0, 1);
    app.stop().await;
}

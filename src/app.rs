use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        Html, IntoResponse, Response,
    },
    routing::{delete, get, post},
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    convert::Infallible,
    io::Write,
    net::IpAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{broadcast, oneshot};

use crate::session::Config;

pub type Shared = Arc<App>;
pub type ApiError = (StatusCode, Json<Value>);
pub fn bad(message: impl ToString) -> ApiError {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":message.to_string()})),
    )
}
pub fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

pub struct Task {
    pub stop: oneshot::Sender<()>,
    pub join: tokio::task::JoinHandle<()>,
}

#[derive(Default)]
pub struct Control {
    pub session: Option<Task>,
    pub capture: Option<Task>,
}

pub struct App {
    // ponytail: one state lock includes persistence I/O; snapshot writes if large RIBs stall updates.
    pub data: Mutex<Data>,
    pub control: tokio::sync::Mutex<Control>,
    pub events: broadcast::Sender<Value>,
    pub shutdown: tokio::sync::watch::Sender<bool>,
}

// Typed RIB row instead of a raw serde_json::Value: on a full table (900k+ rows) a
// Value::Object pays for every field name as a fresh heap string per row, and numbers in
// arrays (e.g. as_path) get boxed to ~24 bytes each instead of a plain 4-byte u32. This still
// serializes to the same JSON shape the web UI already expects.
#[derive(Clone, Serialize)]
pub struct Route {
    pub id: String,
    pub family: &'static str,
    pub afi: Arc<str>,
    pub peer: Arc<str>,
    pub received_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_hop: Option<String>,
    #[serde(skip_serializing_if = "slice_is_empty")]
    pub as_path: Arc<[u32]>,
    #[serde(skip_serializing_if = "slice_is_empty")]
    pub communities: Arc<[String]>,
    #[serde(rename = "match", skip_serializing_if = "Option::is_none")]
    pub match_: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
}
fn slice_is_empty<T>(v: &Arc<[T]>) -> bool {
    v.is_empty()
}

// Full tables reuse the same AS_PATH / communities across huge numbers of prefixes (same
// upstream, same policy tag), and peer/afi are drawn from a handful of distinct values (one
// peer, a few AFI/SAFI combos) repeated across every row. Interning them means routes that
// share content share the allocation too, instead of each of 900k+ rows carrying its own copy.
// ponytail: interned entries are never evicted; the number of distinct values on a real table
// is in the thousands at most, not millions, so this stays small regardless of RIB size.
// Data::clear() resets it between sessions.
#[derive(Default)]
struct Pool {
    as_path: HashSet<Arc<[u32]>>,
    communities: HashSet<Arc<[String]>>,
    strings: HashSet<Arc<str>>,
}
impl Pool {
    fn intern_as_path(&mut self, v: Vec<u32>) -> Arc<[u32]> {
        if let Some(existing) = self.as_path.get(v.as_slice()) {
            return existing.clone();
        }
        let arc: Arc<[u32]> = Arc::from(v);
        self.as_path.insert(arc.clone());
        arc
    }
    fn intern_communities(&mut self, v: Vec<String>) -> Arc<[String]> {
        if let Some(existing) = self.communities.get(v.as_slice()) {
            return existing.clone();
        }
        let arc: Arc<[String]> = Arc::from(v);
        self.communities.insert(arc.clone());
        arc
    }
    fn intern_str(&mut self, v: &str) -> Arc<str> {
        if let Some(existing) = self.strings.get(v) {
            return existing.clone();
        }
        let arc: Arc<str> = Arc::from(v);
        self.strings.insert(arc.clone());
        arc
    }
    fn clear(&mut self) {
        self.as_path.clear();
        self.communities.clear();
        self.strings.clear();
    }
}

pub struct Data {
    pub config: Option<Config>,
    pub running: bool,
    pub capturing: bool,
    pub state: &'static str,
    pub peer_info: Value,
    pub history: VecDeque<Value>,
    pub routes: BTreeMap<u64, Arc<Route>>,
    ids: HashMap<String, u64>,
    sequence: u64,
    counts: HashMap<String, usize>,
    analytics: BTreeMap<String, BTreeMap<String, usize>>,
    pool: Pool,
    pub output: Option<String>,
    dirty: Option<Instant>,
}

impl App {
    pub fn new(output: Option<String>) -> Shared {
        Arc::new(Self {
            data: Mutex::new(Data {
                config: None,
                running: false,
                capturing: false,
                state: "IDLE",
                peer_info: json!({}),
                history: VecDeque::new(),
                routes: BTreeMap::new(),
                ids: HashMap::new(),
                sequence: 0,
                counts: HashMap::new(),
                analytics: [
                    "communities",
                    "origin_as",
                    "next_hops",
                    "prefix_lengths",
                    "actions",
                    "protocols",
                    "ports",
                ]
                .into_iter()
                .map(|s| (s.into(), BTreeMap::new()))
                .collect(),
                pool: Pool::default(),
                output,
                dirty: None,
            }),
            control: tokio::sync::Mutex::new(Control::default()),
            events: broadcast::channel(2000).0,
            shutdown: tokio::sync::watch::channel(false).0,
        })
    }

    pub fn emit(&self, kind: &str, level: &str, message: impl ToString, extra: Value) {
        let mut event = json!({"ts":now(),"type":kind,"level":level,"message":message.to_string()});
        if let Some(extra) = extra.as_object() {
            event.as_object_mut().unwrap().extend(extra.clone());
        }
        match level {
            "error" => tracing::error!("{event}"),
            "warning" => tracing::warn!("{event}"),
            "update" | "packet" => tracing::debug!("{event}"),
            _ => tracing::info!("{event}"),
        }
        let mut data = self.data.lock().unwrap();
        if data.history.len() == 2000 {
            data.history.pop_front();
        }
        data.history.push_back(event.clone());
        let _ = self.events.send(event);
    }

    pub fn set_state(&self, state: &'static str) {
        let running = {
            let mut data = self.data.lock().unwrap();
            data.state = state;
            data.running
        };
        self.emit(
            "session",
            "info",
            format!("State: {state}"),
            json!({"state":state,"running":running}),
        );
    }

    pub fn flush(&self, force: bool) -> std::io::Result<()> {
        // Snapshot Arc<Route> clones and release the lock before serializing to disk, so a
        // large RIB write doesn't hold apply_update() out for the whole file write.
        let (path, routes) = {
            let data = self.data.lock().unwrap();
            if !force
                && !data
                    .dirty
                    .is_some_and(|t| t.elapsed() >= Duration::from_secs(5))
            {
                return Ok(());
            }
            let Some(path) = data.output.clone() else {
                return Ok(());
            };
            (path, data.routes.values().cloned().collect::<Vec<_>>())
        };
        let tmp = format!("{path}.tmp");
        let mut file = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        write!(file, "{{\"count\":{},\"routes\":[", routes.len())?;
        for (i, route) in routes.iter().enumerate() {
            if i > 0 {
                file.write_all(b",")?;
            }
            serde_json::to_writer(&mut file, route)?;
        }
        file.write_all(b"]}")?;
        file.flush()?;
        file.get_ref().sync_all()?;
        std::fs::rename(tmp, &path)?;
        // ponytail: a mutation landing while this write was in flight already re-set dirty
        // via insert()/remove(); clearing it here can race that back to None, but the next
        // mutation re-marks it and the following periodic flush picks up the change.
        self.data.lock().unwrap().dirty = None;
        Ok(())
    }

    pub async fn stop_task(task: &mut Option<Task>) {
        if let Some(task) = task.take() {
            let _ = task.stop.send(());
            let _ = task.join.await;
        }
    }

    pub async fn stop(&self) {
        let mut control = self.control.lock().await;
        Self::stop_task(&mut control.session).await;
        Self::stop_task(&mut control.capture).await;
        {
            let mut data = self.data.lock().unwrap();
            data.running = false;
            data.state = "IDLE";
            data.peer_info = json!({});
            data.clear();
        }
        self.emit(
            "session",
            "info",
            "BGP session stopped",
            json!({"running":false,"state":"IDLE"}),
        );
        if let Err(e) = self.flush(true) {
            self.emit("error", "error", e, json!({}));
        }
    }

    pub async fn start(self: &Shared, config: Config) -> Result<(), String> {
        config.validate()?;
        let mut control = self.control.lock().await;
        Self::stop_task(&mut control.session).await;
        Self::stop_task(&mut control.capture).await;
        self.flush(true).map_err(|e| e.to_string())?;
        {
            let mut data = self.data.lock().unwrap();
            data.clear();
            data.output = config.json_output.clone();
            data.config = Some(config.clone());
            data.running = true;
            data.peer_info = json!({});
        }
        let mut extra = serde_json::to_value(&config).unwrap();
        extra["running"] = json!(true);
        self.emit(
            "session",
            "info",
            format!("Starting BGP session: {}", config.peer_ip),
            extra,
        );
        let (stop, rx) = oneshot::channel();
        let app = self.clone();
        control.session = Some(Task {
            stop,
            join: tokio::spawn(crate::session::run(app, config, rx)),
        });
        Ok(())
    }

    pub fn apply_update(&self, update: Value, peer: &str) {
        let attrs = &update["path_attributes"];
        let mut path: Vec<u32> = Vec::new();
        let mut as4: Vec<u32> = Vec::new();
        let mut communities: Vec<String> = Vec::new();
        for attr in attrs.as_array().into_iter().flatten() {
            if let Some(values) = attr["value"].as_array() {
                match attr["name"].as_str().unwrap_or("") {
                    "AS_PATH" | "AS4_PATH" => {
                        let flat: Vec<u32> = values
                            .iter()
                            .flat_map(|s| s["asns"].as_array().into_iter().flatten())
                            .filter_map(|v| v.as_u64())
                            .map(|n| n as u32)
                            .collect();
                        if attr["name"] == "AS4_PATH" {
                            as4 = flat;
                        } else {
                            path = flat;
                        }
                    }
                    "COMMUNITIES" | "LARGE_COMMUNITIES" => communities.extend(
                        values.iter().filter_map(|v| v.as_str()).map(String::from),
                    ),
                    _ => {}
                }
            }
        }
        if !as4.is_empty() {
            path.truncate(path.len().saturating_sub(as4.len()));
            path.extend(as4);
        }
        let actions: Vec<String> = update["actions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .map(String::from)
            .collect();
        // Every route in this UPDATE shares the same AS_PATH/communities/peer; intern once
        // here instead of per route so identical content across updates shares one allocation.
        let (path, communities, peer_arc) = {
            let mut data = self.data.lock().unwrap();
            (
                data.pool.intern_as_path(path),
                data.pool.intern_communities(communities),
                data.pool.intern_str(peer),
            )
        };
        // Collect per-kind rows and emit once per BGP UPDATE message instead of once per
        // route: a full-table initial sync can carry hundreds of routes per message, and
        // each emit() pays for a JSON build, a history push and a broadcast send.
        let mut batches: HashMap<&'static str, Vec<Value>> = HashMap::new();
        for kind in ["announce", "withdraw"] {
            for (afi, routes) in update[kind].as_object().into_iter().flatten() {
                let family: &'static str = if afi.ends_with("-unicast") {
                    "unicast"
                } else {
                    "flowspec"
                };
                // afi is constant for every route in this group; intern once per group
                // instead of once per route.
                let afi_arc = self.data.lock().unwrap().pool.intern_str(afi);
                for route in routes.as_array().into_iter().flatten() {
                    let identity = if family == "unicast" {
                        json!({"prefix":route["prefix"]})
                    } else {
                        route.clone()
                    };
                    let canonical =
                        serde_json::to_vec(&json!([family, afi, peer, identity])).unwrap();
                    let id = format!("{:x}", Sha1::digest(canonical))[..12].to_owned();
                    let mut entry = Route {
                        id: id.clone(),
                        family,
                        afi: afi_arc.clone(),
                        peer: peer_arc.clone(),
                        received_at: now(),
                        prefix: None,
                        next_hop: None,
                        as_path: path.clone(),
                        communities: communities.clone(),
                        match_: None,
                        actions: Vec::new(),
                    };
                    if family == "unicast" {
                        entry.prefix = route["prefix"].as_str().map(String::from);
                        entry.next_hop = route
                            .get("next_hop")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .map(String::from);
                    } else {
                        entry.match_ = Some(route.clone());
                        entry.actions = actions.clone();
                    }
                    let mut data = self.data.lock().unwrap();
                    let existed = data.remove(&id);
                    if kind == "announce" {
                        data.insert(id.clone(), entry.clone());
                    }
                    drop(data);
                    let route_id = (kind == "announce" || existed).then(|| id.clone());
                    let mut row = serde_json::to_value(&entry).unwrap();
                    row["route_id"] = json!(route_id);
                    batches.entry(kind).or_default().push(row);
                }
            }
        }
        // Raw attrs go out on the live/log event only; the stored RIB entry already carries
        // the decoded as_path/communities/match/actions, and duplicating the raw attrs into
        // every RIB row is what blows up memory on a full table.
        for (kind, routes) in batches {
            let count = routes.len();
            self.emit(
                kind,
                "update",
                format!("{} x{count}", kind.to_uppercase()),
                json!({"count":count,"routes":routes,"path_attributes":attrs.clone()}),
            );
        }
    }
}

impl Data {
    fn metrics(route: &Route) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if route.family == "unicast" {
            for c in route.communities.iter() {
                out.push(("communities".into(), c.clone()));
            }
            if let Some(asn) = route.as_path.last() {
                out.push(("origin_as".into(), asn.to_string()));
            }
            if let Some(nh) = route.next_hop.as_deref().filter(|s| !s.is_empty()) {
                out.push(("next_hops".into(), nh.into()));
            }
            if let Some((_, len)) = route.prefix.as_deref().and_then(|s| s.split_once('/')) {
                out.push(("prefix_lengths".into(), format!("/{len}")));
            }
        } else {
            for a in &route.actions {
                out.push(("actions".into(), a.clone()));
            }
            let m = route.match_.as_ref();
            for (name, key) in [
                ("protocols", "ip-proto"),
                ("ports", "port"),
                ("ports", "src-port"),
                ("ports", "dst-port"),
            ] {
                let values = m.and_then(|m| m.get(key)).and_then(Value::as_array);
                for v in values.into_iter().flatten() {
                    out.push((name.into(), v.as_str().unwrap_or("").into()));
                }
            }
        }
        out
    }
    fn count(&mut self, route: &Route, add: bool) {
        for key in [
            route.family,
            if route.afi.starts_with("ipv6") {
                "ipv6"
            } else {
                "ipv4"
            },
        ] {
            let n = self.counts.entry(key.into()).or_default();
            if add {
                *n += 1;
            } else {
                *n -= 1;
            }
        }
        for (key, value) in Self::metrics(route) {
            let counts = self.analytics.get_mut(&key).unwrap();
            let n = counts.entry(value.clone()).or_default();
            if add {
                *n += 1;
            } else {
                *n -= 1;
            }
            if *n == 0 {
                counts.remove(&value);
            }
        }
    }
    fn insert(&mut self, id: String, route: Route) {
        self.count(&route, true);
        self.sequence += 1;
        self.ids.insert(id, self.sequence);
        self.routes.insert(self.sequence, Arc::new(route));
        self.dirty = Some(Instant::now());
    }
    fn remove(&mut self, id: &str) -> bool {
        if let Some(seq) = self.ids.remove(id) {
            let route = self.routes.remove(&seq).unwrap();
            self.count(&route, false);
            self.dirty = Some(Instant::now());
            true
        } else {
            false
        }
    }
    pub fn clear(&mut self) {
        if !self.routes.is_empty() {
            self.dirty = Some(Instant::now());
        }
        self.routes.clear();
        self.ids.clear();
        self.counts.clear();
        self.pool.clear();
        for c in self.analytics.values_mut() {
            c.clear();
        }
    }
    pub fn stats(&self) -> Value {
        let analytics: BTreeMap<_, _> = self
            .analytics
            .iter()
            .map(|(name, counts)| {
                let mut rows: Vec<_> = counts.iter().collect();
                rows.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
                rows.truncate(5);
                (name, rows)
            })
            .collect();
        json!({"total":self.routes.len(),"unicast":self.counts.get("unicast").unwrap_or(&0),"flowspec":self.counts.get("flowspec").unwrap_or(&0),"ipv4":self.counts.get("ipv4").unwrap_or(&0),"ipv6":self.counts.get("ipv6").unwrap_or(&0),"analytics":analytics})
    }
    fn snapshot(&self) -> Value {
        let mut status = self
            .config
            .as_ref()
            .map(|c| serde_json::to_value(c).unwrap())
            .unwrap_or(json!({}));
        status["running"] = json!(self.running);
        status["state"] = json!(self.state);
        status["peer_info"] = self.peer_info.clone();
        status["capture_running"] = json!(self.capturing);
        json!({"ts":now(),"type":"snapshot","level":"info","message":"snapshot","status":status,"stats":self.stats()})
    }
}

#[derive(Deserialize)]
struct Page {
    family: Option<String>,
    page: Option<i64>,
    page_size: Option<i64>,
    sort: Option<String>,
    order: Option<String>,
}
fn family(query: &Page) -> Result<&str, ApiError> {
    let f = query.family.as_deref().unwrap_or("total");
    if ["total", "unicast", "flowspec"].contains(&f) {
        Ok(f)
    } else {
        Err(bad("invalid family"))
    }
}
async fn routes(State(app): State<Shared>, Query(q): Query<Page>) -> Result<Json<Value>, ApiError> {
    let family = family(&q)?;
    let page = q.page.unwrap_or(1).max(1) as usize;
    let size = q.page_size.unwrap_or(50).clamp(1, 500) as usize;
    let data = app.data.lock().unwrap();
    let mut rows: Vec<_> = data
        .routes
        .values()
        .filter(|r| family == "total" || r.family == family)
        .collect();
    let sort = q
        .sort
        .as_deref()
        .filter(|s| {
            [
                "id",
                "family",
                "afi",
                "prefix",
                "next_hop",
                "peer",
                "received_at",
            ]
            .contains(s)
        })
        .unwrap_or("received_at");
    if sort != "received_at" {
        rows.sort_by_cached_key(|r| {
            match sort {
                "id" => r.id.as_str(),
                "family" => r.family,
                "afi" => r.afi.as_ref(),
                "prefix" => r.prefix.as_deref().unwrap_or(""),
                "next_hop" => r.next_hop.as_deref().unwrap_or(""),
                "peer" => r.peer.as_ref(),
                _ => "",
            }
            .to_lowercase()
        });
    }
    if q.order.as_deref() != Some("asc") {
        rows.reverse();
    }
    let count = rows.len();
    let start = page.saturating_sub(1).saturating_mul(size);
    Ok(Json(
        json!({"page":page,"page_size":size,"count":count,"routes":rows.into_iter().skip(start).take(size).collect::<Vec<_>>(),"stats":data.stats()}),
    ))
}
async fn export(State(app): State<Shared>, Query(q): Query<Page>) -> Result<Response, ApiError> {
    let family = family(&q)?.to_owned();
    // Snapshot shared references so serialization does not hold the RIB lock across awaits.
    let rows: Vec<_> = app
        .data
        .lock()
        .unwrap()
        .routes
        .values()
        .filter(|r| family == "total" || r.family == family)
        .cloned()
        .collect();
    let stream = async_stream::stream! {
        yield Ok::<_,Infallible>("{\"routes\":[".to_owned());
        for (i,row) in rows.into_iter().enumerate(){
            yield Ok(format!("{}{}",if i==0{""}else{","},serde_json::to_string(&row).unwrap()));
        }
        yield Ok("]}".to_owned());
    };
    Ok((
        [
            ("content-type", "application/json"),
            (
                "content-disposition",
                "attachment; filename=\"bgpx-routes.json\"",
            ),
        ],
        axum::body::Body::from_stream(stream),
    )
        .into_response())
}

// Longest-prefix match, `show ip bgp <ip>` style: the mask length this unicast
// prefix shares with `ip`, or None if `ip` isn't inside it.
fn prefix_match_len(prefix: &str, ip: IpAddr) -> Option<u8> {
    let (net, len) = prefix.split_once('/')?;
    let len: u8 = len.parse().ok()?;
    match (net.parse::<IpAddr>().ok()?, ip) {
        (IpAddr::V4(net), IpAddr::V4(ip)) if len <= 32 => {
            let mask = if len == 0 { 0 } else { u32::MAX << (32 - len) };
            (u32::from(net) & mask == u32::from(ip) & mask).then_some(len)
        }
        (IpAddr::V6(net), IpAddr::V6(ip)) if len <= 128 => {
            let mask = if len == 0 { 0 } else { u128::MAX << (128 - len) };
            (u128::from(net) & mask == u128::from(ip) & mask).then_some(len)
        }
        _ => None,
    }
}

#[derive(Deserialize)]
struct SearchQuery {
    ip: String,
}
async fn search(
    State(app): State<Shared>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<Value>, ApiError> {
    let ip: IpAddr = q.ip.trim().parse().map_err(|_| bad("invalid ip"))?;
    // Snapshot Arc<Route> clones and release the lock before scanning, same as export():
    // a full-table scan shouldn't hold apply_update() out for its whole duration.
    let routes: Vec<Arc<Route>> = app.data.lock().unwrap().routes.values().cloned().collect();
    let best = routes
        .iter()
        .filter(|r| r.family == "unicast")
        .filter_map(|r| Some((prefix_match_len(r.prefix.as_deref()?, ip)?, r)))
        .max_by_key(|(len, _)| *len);
    Ok(Json(match best {
        Some((len, route)) => json!({"match":true,"length":len,"route":route}),
        None => json!({"match":false}),
    }))
}
async fn events(State(app): State<Shared>) -> Response {
    let mut shutdown = app.shutdown.subscribe();
    let (mut rx, initial) = {
        let data = app.data.lock().unwrap();
        let rx = app.events.subscribe();
        let mut initial = vec![data.snapshot()];
        initial.extend(data.history.iter().cloned().map(|mut e| {
            e["replayed"] = json!(true);
            e
        }));
        (rx, initial)
    };
    let stream = async_stream::stream! {
        for event in initial {yield Ok::<_,Infallible>(Event::default().data(event.to_string()));}
        loop {let received=tokio::select!{_=shutdown.changed()=>break,event=rx.recv()=>event};match received {
            Ok(event)=>yield Ok(Event::default().data(event.to_string())),
            Err(broadcast::error::RecvError::Lagged(_))=>{
                let snapshot=app.data.lock().unwrap().snapshot();yield Ok(Event::default().data(snapshot.to_string()));
            }
            Err(_)=>break,
        }}
    };
    (
        [("x-accel-buffering", "no")],
        Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(20))),
    )
        .into_response()
}
async fn start(
    State(app): State<Shared>,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(body) = body.map_err(bad)?;
    let config = Config::from_value(body).map_err(bad)?;
    app.start(config).await.map_err(bad)?;
    Ok(Json(json!({"ok":true})))
}
async fn stop(State(app): State<Shared>) -> Json<Value> {
    app.stop().await;
    Json(json!({"ok":true}))
}
async fn health(State(app): State<Shared>) -> Response {
    let data = app.data.lock().unwrap();
    let ok = data.running && data.state == "ESTABLISHED";
    (
        if ok {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(json!({"status":if ok{"ok"}else{"degraded"},"bgp_state":data.state})),
    )
        .into_response()
}
pub fn router(app: Shared) -> Router {
    Router::new()
        .route("/", get(|| async { Html(include_str!("../web/ui.html")) }))
        .route("/session/start", post(start))
        .route("/session/stop", post(stop))
        .route("/routes", get(routes))
        .route("/routes/export", get(export))
        .route("/routes/search", get(search))
        .route("/events", get(events))
        .route("/health", get(health))
        .route(
            "/log",
            delete(|State(app): State<Shared>| async move {
                app.data.lock().unwrap().history.clear();
                Json(json!({"ok":true}))
            }),
        )
        .route("/capture/start", post(crate::capture::start))
        .route("/capture/stop", post(crate::capture::stop))
        .with_state(app)
}

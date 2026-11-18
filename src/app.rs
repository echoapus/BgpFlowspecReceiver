use axum::{
    extract::{Path, Query, State},
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
use tokio::sync::{broadcast, mpsc, oneshot, OwnedSemaphorePermit, Semaphore};

const HISTORY_BYTES: usize = 8 * 1024 * 1024;
const HISTORY_EVENTS: usize = 2000;
const EXPORT_TIMEOUT: Duration = Duration::from_secs(60);

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
    // Route snapshots release this lock before persistence I/O.
    pub data: Mutex<Data>,
    pub control: tokio::sync::Mutex<Control>,
    pub events: broadcast::Sender<Arc<str>>,
    exports: Arc<Semaphore>,
    persistence: Mutex<()>,
    pub shutdown: tokio::sync::watch::Sender<bool>,
}

// Typed RIB row instead of a raw serde_json::Value: on a full table (900k+ rows) a
// Value::Object pays for every field name as a fresh heap string per row, and numbers in
// arrays (e.g. as_path) get boxed to ~24 bytes each instead of a plain 4-byte u32. This still
// serializes to the same JSON shape the web UI already expects.
// The RIB stores compact `Entry` slots (below); `Route` is a borrowed view built only when a
// row is serialized.
#[derive(Serialize)]
pub struct Flow {
    #[serde(rename = "match")]
    pub match_: Value,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
}

// id is the first 48 bits of a SHA-1 and received_at is epoch microseconds; both are formatted
// to the same strings as before only when serialized.
fn ser_id<S: serde::Serializer>(id: &u64, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(&format_args!("{id:012x}"))
}
fn ser_time<S: serde::Serializer>(us: &i64, s: S) -> Result<S::Ok, S::Error> {
    let t = chrono::DateTime::from_timestamp_micros(*us).unwrap_or_default();
    s.collect_str(&t.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
}

#[derive(Serialize)]
pub struct Route<'a> {
    #[serde(serialize_with = "ser_id")]
    pub id: u64,
    pub family: &'static str,
    pub afi: &'a str,
    pub peer: &'a str,
    #[serde(serialize_with = "ser_time")]
    pub received_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<Prefix>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_hop: Option<&'a str>,
    #[serde(skip_serializing_if = "slice_is_empty")]
    pub as_path: &'a [u32],
    #[serde(skip_serializing_if = "slice_is_empty")]
    pub communities: &'a [String],
    #[serde(flatten)]
    pub flow: Option<&'a Flow>,
}
fn slice_is_empty<T>(v: &&[T]) -> bool {
    v.is_empty()
}

// Everything an UPDATE shares across its prefixes (and that repeats across a full table) lives
// in one interned, refcounted record, so a slot only carries an 8-byte pointer to it.
#[derive(PartialEq, Eq, Hash)]
pub struct Attrs {
    afi: Box<str>,
    peer: Box<str>,
    next_hop: Option<Box<str>>,
    as_path: Box<[u32]>,
    communities: Box<[String]>,
}
impl Attrs {
    fn family(&self) -> &'static str {
        if self.afi.ends_with("-unicast") {
            "unicast"
        } else {
            "flowspec"
        }
    }
}

// Packed unicast prefix (16 address bytes, IPv4 uses the first 4) instead of a heap String.
#[derive(Clone, Copy)]
pub struct Prefix {
    addr: [u8; 16],
    len: u8,
    v6: bool,
}
impl Prefix {
    fn parse(s: &str) -> Option<Self> {
        let (net, len) = s.split_once('/')?;
        let len: u8 = len.parse().ok()?;
        let mut addr = [0u8; 16];
        let v6 = match net.parse::<IpAddr>().ok()? {
            IpAddr::V4(a) if len <= 32 => {
                addr[..4].copy_from_slice(&a.octets());
                false
            }
            IpAddr::V6(a) if len <= 128 => {
                addr = a.octets();
                true
            }
            _ => return None,
        };
        Some(Self { addr, len, v6 })
    }
    // Longest-prefix match, `show ip bgp <ip>` style: the mask length if `ip` is inside.
    fn match_len(&self, ip: IpAddr) -> Option<u8> {
        match (self.v6, ip) {
            (false, IpAddr::V4(ip)) => {
                let net = u32::from_be_bytes([self.addr[0], self.addr[1], self.addr[2], self.addr[3]]);
                let mask = if self.len == 0 { 0 } else { u32::MAX << (32 - self.len) };
                (net & mask == u32::from(ip) & mask).then_some(self.len)
            }
            (true, IpAddr::V6(ip)) => {
                let mask = if self.len == 0 { 0 } else { u128::MAX << (128 - self.len) };
                (u128::from_be_bytes(self.addr) & mask == u128::from(ip) & mask).then_some(self.len)
            }
            _ => None,
        }
    }
}
impl std::fmt::Display for Prefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.v6 {
            write!(f, "{}/{}", std::net::Ipv6Addr::from(self.addr), self.len)
        } else {
            let a = self.addr;
            write!(f, "{}/{}", std::net::Ipv4Addr::new(a[0], a[1], a[2], a[3]), self.len)
        }
    }
}
impl Serialize for Prefix {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

#[derive(Clone)]
pub struct Uni {
    id: u64,
    at: i64,
    attrs: Arc<Attrs>,
    prefix: Prefix,
}
#[derive(Clone)]
pub struct FlowEntry {
    id: u64,
    at: i64,
    attrs: Arc<Attrs>,
    flow: Arc<Flow>,
}
#[derive(Clone)]
pub enum Entry {
    Uni(Uni),
    Flow(FlowEntry),
}
impl Entry {
    fn id(&self) -> u64 {
        match self {
            Entry::Uni(u) => u.id,
            Entry::Flow(f) => f.id,
        }
    }
    fn attrs(&self) -> &Arc<Attrs> {
        match self {
            Entry::Uni(u) => &u.attrs,
            Entry::Flow(f) => &f.attrs,
        }
    }
    pub fn view(&self) -> Route<'_> {
        let (id, at, a) = match self {
            Entry::Uni(u) => (u.id, u.at, &*u.attrs),
            Entry::Flow(f) => (f.id, f.at, &*f.attrs),
        };
        Route {
            id,
            family: a.family(),
            afi: &a.afi,
            peer: &a.peer,
            received_at: at,
            prefix: match self {
                Entry::Uni(u) => Some(u.prefix),
                _ => None,
            },
            next_hop: a.next_hop.as_deref(),
            as_path: &a.as_path,
            communities: &a.communities,
            flow: match self {
                Entry::Flow(f) => Some(&*f.flow),
                _ => None,
            },
        }
    }
}

// Full tables reuse the same AS_PATH / communities across huge numbers of prefixes (same
// upstream, same policy tag), and peer/afi are drawn from a handful of distinct values (one
// peer, a few AFI/SAFI combos) repeated across every row. Interning them means routes that
// share content share the allocation too, instead of each of 900k+ rows carrying its own copy.
// Unused entries are collected by the maintenance worker; session teardown drops the pool.
#[derive(Default)]
struct Pool {
    attrs: HashSet<Arc<Attrs>>,
}
impl Pool {
    fn intern(&mut self, a: Attrs) -> Arc<Attrs> {
        if let Some(existing) = self.attrs.get(&a) {
            return existing.clone();
        }
        let arc = Arc::new(a);
        self.attrs.insert(arc.clone());
        arc
    }
    fn collect(&mut self) {
        self.attrs.retain(|v| Arc::strong_count(v) > 1);
        self.attrs.shrink_to_fit();
    }
}

pub struct Data {
    pub config: Option<Config>,
    pub running: bool,
    pub capturing: bool,
    pub state: &'static str,
    pub peer_info: Value,
    pub history: VecDeque<Arc<str>>,
    history_bytes: usize,
    // Append-only slab in insertion order (a re-announce moves a route to the end). Withdrawn
    // slots become None and are squeezed out by compact(), which bumps `epoch` so chunked
    // readers (flush) know their cursor is stale. ids maps route id -> slab index.
    slab: Vec<Option<Entry>>,
    holes: usize,
    epoch: u64,
    ids: HashMap<u64, u32>,
    counts: HashMap<String, usize>,
    analytics: BTreeMap<String, BTreeMap<String, usize>>,
    pool: Pool,
    pub output: Option<String>,
    dirty: Option<Instant>,
    revision: u64,
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
                history_bytes: 0,
                slab: Vec::new(),
                holes: 0,
                epoch: 0,
                ids: HashMap::new(),
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
                revision: 0,
            }),
            control: tokio::sync::Mutex::new(Control::default()),
            // Short live buffer; lagging clients recover through a snapshot.
            events: broadcast::channel(64).0,
            exports: Arc::new(Semaphore::new(2)),
            persistence: Mutex::new(()),
            shutdown: tokio::sync::watch::channel(false).0,
        })
    }

    pub fn emit(&self, kind: &str, level: &str, message: impl ToString, extra: Value) {
        let mut event = json!({"ts":now(),"type":kind,"level":level,"message":message.to_string()});
        if let Value::Object(extra) = extra {
            event.as_object_mut().unwrap().extend(extra);
        }
        match level {
            "error" => tracing::error!("{event}"),
            "warning" => tracing::warn!("{event}"),
            "update" | "packet" => tracing::debug!("{event}"),
            _ => tracing::info!("{event}"),
        }
        let event: Arc<str> = Arc::from(event.to_string());
        let mut data = self.data.lock().unwrap();
        if event.len() <= HISTORY_BYTES {
            while data.history.len() >= HISTORY_EVENTS
                || data.history_bytes + event.len() > HISTORY_BYTES
            {
                let oldest = data.history.pop_front().unwrap();
                data.history_bytes -= oldest.len();
            }
            data.history_bytes += event.len();
            data.history.push_back(event.clone());
        }
        let _ = self.events.send(event);
    }

    pub fn reclaim_memory(&self) {
        let mut data = self.data.lock().unwrap();
        data.compact();
        data.pool.collect();
        data.ids.shrink_to_fit();
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
        // Serialize the slab in small chunks, re-taking the lock for each, so a large RIB
        // write doesn't hold apply_update() out for the whole file write. If compaction runs
        // mid-write (epoch changes) the cursor is stale: abandon and retry on the next tick.
        let _persistence = self.persistence.lock().unwrap();
        let (path, epoch, revision) = {
            let data = self.data.lock().unwrap();
            if !force
                && data
                    .dirty
                    .is_none_or(|t| t.elapsed() < Duration::from_secs(5))
            {
                return Ok(());
            }
            let Some(path) = data.output.clone() else {
                return Ok(());
            };
            (path, data.epoch, data.revision)
        };
        let tmp = format!("{path}.tmp");
        let mut file = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        file.write_all(b"{\"routes\":[")?;
        let (mut cursor, mut n) = (0usize, 0usize);
        loop {
            let mut buf = Vec::new();
            let done = {
                let data = self.data.lock().unwrap();
                if data.epoch != epoch {
                    drop(file);
                    let _ = std::fs::remove_file(&tmp);
                    return Ok(());
                }
                let end = (cursor + 4096).min(data.slab.len());
                for e in data.slab[cursor..end].iter().flatten() {
                    if n > 0 {
                        buf.push(b',');
                    }
                    serde_json::to_writer(&mut buf, &e.view())?;
                    n += 1;
                }
                cursor = end;
                end == data.slab.len()
            };
            file.write_all(&buf)?;
            if done {
                break;
            }
        }
        write!(file, "],\"count\":{n}}}")?;
        file.flush()?;
        file.get_ref().sync_all()?;
        std::fs::rename(tmp, &path)?;
        let mut data = self.data.lock().unwrap();
        if data.revision == revision {
            data.dirty = None;
        }
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
            data.clear_history();
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
                    "COMMUNITIES" | "LARGE_COMMUNITIES" => communities
                        .extend(values.iter().filter_map(|v| v.as_str()).map(String::from)),
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
                // Routes in one group share attributes except possibly next_hop; intern one
                // Attrs per distinct next_hop instead of once per route.
                let mut cache: HashMap<Option<&str>, Arc<Attrs>> = HashMap::new();
                for route in routes.as_array().into_iter().flatten() {
                    let identity = if family == "unicast" {
                        json!({"prefix":route["prefix"]})
                    } else {
                        route.clone()
                    };
                    let canonical =
                        serde_json::to_vec(&json!([family, afi, peer, identity])).unwrap();
                    let d = Sha1::digest(canonical);
                    let id = u64::from_be_bytes([0, 0, d[0], d[1], d[2], d[3], d[4], d[5]]);
                    let next_hop = if family == "unicast" {
                        route
                            .get("next_hop")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                    } else {
                        None
                    };
                    let attrs_arc = match cache.get(&next_hop) {
                        Some(a) => a.clone(),
                        None => {
                            let a = self.data.lock().unwrap().pool.intern(Attrs {
                                afi: afi.as_str().into(),
                                peer: peer.into(),
                                next_hop: next_hop.map(Into::into),
                                as_path: path.as_slice().into(),
                                communities: communities.as_slice().into(),
                            });
                            cache.insert(next_hop, a.clone());
                            a
                        }
                    };
                    let at = Utc::now().timestamp_micros();
                    let entry = if family == "unicast" {
                        // shortcut: a malformed prefix drops the route; wire decode always emits valid ones.
                        let Some(prefix) = route["prefix"].as_str().and_then(Prefix::parse) else {
                            continue;
                        };
                        Entry::Uni(Uni {
                            id,
                            at,
                            attrs: attrs_arc,
                            prefix,
                        })
                    } else {
                        Entry::Flow(FlowEntry {
                            id,
                            at,
                            attrs: attrs_arc,
                            flow: Arc::new(Flow {
                                match_: route.clone(),
                                actions: actions.clone(),
                            }),
                        })
                    };
                    let mut row = serde_json::to_value(entry.view()).unwrap();
                    let mut data = self.data.lock().unwrap();
                    let existed = data.remove(id);
                    if kind == "announce" {
                        data.insert(id, entry);
                    }
                    drop(data);
                    let route_id = (kind == "announce" || existed).then(|| format!("{id:012x}"));
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
    fn mark_dirty(&mut self) {
        self.dirty.get_or_insert_with(Instant::now);
        self.revision += 1;
    }

    fn clear_history(&mut self) {
        self.history = VecDeque::new();
        self.history_bytes = 0;
    }

    fn metrics(route: &Route<'_>) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if route.family == "unicast" {
            for c in route.communities.iter() {
                out.push(("communities".into(), c.clone()));
            }
            if let Some(asn) = route.as_path.last() {
                out.push(("origin_as".into(), asn.to_string()));
            }
            if let Some(nh) = route.next_hop {
                out.push(("next_hops".into(), nh.into()));
            }
            if let Some(p) = route.prefix {
                out.push(("prefix_lengths".into(), format!("/{}", p.len)));
            }
        } else {
            let flow = route.flow;
            for a in flow.map(|f| f.actions.as_slice()).unwrap_or_default() {
                out.push(("actions".into(), a.clone()));
            }
            let m = flow.map(|f| &f.match_);
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
    fn count(&mut self, route: &Route<'_>, add: bool) {
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
    fn insert(&mut self, id: u64, entry: Entry) {
        self.count(&entry.view(), true);
        self.ids.insert(id, self.slab.len() as u32);
        self.slab.push(Some(entry));
        self.mark_dirty();
    }
    fn remove(&mut self, id: u64) -> bool {
        if let Some(i) = self.ids.remove(&id) {
            let entry = self.slab[i as usize].take().unwrap();
            self.count(&entry.view(), false);
            self.holes += 1;
            self.mark_dirty();
            true
        } else {
            false
        }
    }
    pub fn route_count(&self) -> usize {
        self.ids.len()
    }
    // All routes as JSON rows in insertion order (for tests and tooling).
    pub fn rows(&self) -> Vec<Value> {
        self.slab
            .iter()
            .flatten()
            .map(|e| serde_json::to_value(e.view()).unwrap())
            .collect()
    }
    // In-place, order-preserving squeeze of withdrawn slots; skipped until >25% are holes.
    fn compact(&mut self) {
        if self.holes * 4 <= self.slab.len() {
            return;
        }
        let mut w = 0;
        for r in 0..self.slab.len() {
            if self.slab[r].is_some() {
                self.slab.swap(w, r);
                let id = self.slab[w].as_ref().unwrap().id();
                self.ids.insert(id, w as u32);
                w += 1;
            }
        }
        self.slab.truncate(w);
        self.slab.shrink_to_fit();
        self.holes = 0;
        self.epoch += 1;
    }
    pub fn clear(&mut self) {
        if !self.ids.is_empty() {
            self.mark_dirty();
        }
        self.slab = Vec::new();
        self.holes = 0;
        self.epoch += 1;
        self.ids = HashMap::new();
        self.counts = HashMap::new();
        self.pool = Pool::default();
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
        json!({"total":self.ids.len(),"unicast":self.counts.get("unicast").unwrap_or(&0),"flowspec":self.counts.get("flowspec").unwrap_or(&0),"ipv4":self.counts.get("ipv4").unwrap_or(&0),"ipv6":self.counts.get("ipv6").unwrap_or(&0),"analytics":analytics})
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

#[derive(Default, Deserialize)]
struct Page {
    family: Option<String>,
    page: Option<i64>,
    page_size: Option<i64>,
    sort: Option<String>,
    order: Option<String>,
    filter: Option<String>,
}
fn family(query: &Page) -> Result<&str, ApiError> {
    let f = query.family.as_deref().unwrap_or("total");
    if ["total", "unicast", "flowspec"].contains(&f) {
        Ok(f)
    } else {
        Err(bad("invalid family"))
    }
}
fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

fn route_matches_filter(r: &Route<'_>, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    // `get` avoids panicking when the needle starts with a multi-byte UTF-8 character.
    let asn_q = match (needle.get(..2), needle.get(2..)) {
        (Some(p), Some(rest)) if !rest.is_empty() && p.eq_ignore_ascii_case("as") => rest,
        _ => needle,
    };
    if contains_ignore_ascii_case(r.family, needle)
        || contains_ignore_ascii_case(&r.afi, needle)
        || contains_ignore_ascii_case(&r.peer, needle)
        || r.prefix
            .is_some_and(|p| contains_ignore_ascii_case(&p.to_string(), needle))
        || r.next_hop
            .is_some_and(|nh| contains_ignore_ascii_case(nh, needle))
        || r.communities
            .iter()
            .any(|c| contains_ignore_ascii_case(c, needle))
        || r.as_path.iter().any(|asn| {
            let s = asn.to_string();
            s == asn_q || s.contains(needle)
        })
        || r.flow.is_some_and(|f| {
            f.actions
                .iter()
                .any(|a| contains_ignore_ascii_case(a, needle))
        })
    {
        return true;
    }
    if let Some(Value::Object(map)) = r.flow.map(|f| &f.match_) {
        for (k, v) in map {
            if contains_ignore_ascii_case(k, needle)
                || contains_ignore_ascii_case(&v.to_string(), needle)
            {
                return true;
            }
        }
    }
    false
}
async fn routes(State(app): State<Shared>, Query(q): Query<Page>) -> Result<Json<Value>, ApiError> {
    let family = family(&q)?;
    let page = q.page.unwrap_or(1).max(1) as usize;
    let size = q.page_size.unwrap_or(50).clamp(1, 500) as usize;
    let filter = q.filter.as_deref().unwrap_or("").trim();
    // Rows are borrowed from the slab, so filtering/sorting/paging holds the RIB lock (it only
    // builds a Vec of references; there is no per-row copy of the table).
    let data = app.data.lock().unwrap();
    let mut rows: Vec<&Entry> = data
        .slab
        .iter()
        .flatten()
        .filter(|e| family == "total" || e.attrs().family() == family)
        .collect();
    if !filter.is_empty() {
        rows.retain(|e| route_matches_filter(&e.view(), filter));
    }
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
        rows.sort_by_cached_key(|e| {
            let r = e.view();
            match sort {
                "id" => format!("{:012x}", r.id),
                "family" => r.family.to_lowercase(),
                "afi" => r.afi.to_lowercase(),
                "prefix" => r.prefix.map(|p| p.to_string()).unwrap_or_default().to_lowercase(),
                "next_hop" => r.next_hop.unwrap_or("").to_lowercase(),
                "peer" => r.peer.to_lowercase(),
                _ => String::new(),
            }
        });
    }
    if q.order.as_deref() != Some("asc") {
        rows.reverse();
    }
    let count = rows.len();
    let start = page.saturating_sub(1).saturating_mul(size);
    let page_rows: Vec<Value> = rows
        .into_iter()
        .skip(start)
        .take(size)
        .map(|e| serde_json::to_value(e.view()).unwrap())
        .collect();
    Ok(Json(
        json!({"page":page,"page_size":size,"count":count,"routes":page_rows,"stats":data.stats()}),
    ))
}
async fn export(State(app): State<Shared>, Query(q): Query<Page>) -> Result<Response, ApiError> {
    let family = family(&q)?.to_owned();
    let filter = q.filter.as_deref().unwrap_or("").trim().to_owned();
    let permit = app.exports.clone().try_acquire_owned().map_err(|_| {
        (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error":"Two exports are already running"})),
        )
    })?;
    // Copy only the matching slots (refcount bumps, no deep copy) so serialization does not
    // hold the RIB lock.
    let rows: Vec<Entry> = app
        .data
        .lock()
        .unwrap()
        .slab
        .iter()
        .flatten()
        .filter(|e| {
            (family == "total" || e.attrs().family() == family)
                && (filter.is_empty() || route_matches_filter(&e.view(), &filter))
        })
        .cloned()
        .collect();
    Ok((
        [
            ("content-type", "application/json"),
            (
                "content-disposition",
                "attachment; filename=\"bgpx-routes.json\"",
            ),
        ],
        export_body(rows, permit, EXPORT_TIMEOUT),
    )
        .into_response())
}

// The producer owns the snapshot so its deadline releases routes even if the
// HTTP client stops polling the body entirely. One queued row bounds backpressure.
fn export_body(
    rows: Vec<Entry>,
    permit: OwnedSemaphorePermit,
    duration: Duration,
) -> axum::body::Body {
    let (tx, mut rx) = mpsc::channel(1);
    let (done, finished) = oneshot::channel();
    tokio::spawn(async move {
        let _permit = permit;
        let write = async move {
            tx.send(Ok::<_, std::io::Error>("{\"routes\":[".to_owned()))
                .await
                .ok()?;
            for (i, row) in rows.into_iter().enumerate() {
                let json = serde_json::to_string(&row.view()).unwrap();
                tx.send(Ok(format!("{}{json}", if i == 0 { "" } else { "," })))
                    .await
                    .ok()?;
            }
            tx.send(Ok("]}".to_owned())).await.ok()
        };
        let result = tokio::time::timeout(duration, write).await;
        let _ = done.send(result.is_ok());
    });
    axum::body::Body::from_stream(async_stream::stream! {
        while let Some(chunk) = rx.recv().await { yield chunk; }
        if finished.await != Ok(true) {
            yield Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "Export interrupted or timed out"));
        }
    })
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
    // Integer-only scan over the slab; short enough to do under the lock.
    let data = app.data.lock().unwrap();
    let best = data
        .slab
        .iter()
        .flatten()
        .filter_map(|e| match e {
            Entry::Uni(u) => Some((u.prefix.match_len(ip)?, e)),
            _ => None,
        })
        .max_by_key(|(len, _)| *len);
    Ok(Json(match best {
        Some((len, e)) => json!({"match":true,"length":len,"route":e.view()}),
        None => json!({"match":false}),
    }))
}
async fn events(State(app): State<Shared>) -> Response {
    let mut shutdown = app.shutdown.subscribe();
    let (mut rx, snapshot, initial) = {
        let data = app.data.lock().unwrap();
        (
            app.events.subscribe(),
            data.snapshot().to_string(),
            data.history.clone(),
        )
    };
    let stream = async_stream::stream! {
        yield Ok::<_,Infallible>(Event::default().data(snapshot));
        for event in initial {
            // Events are serialized objects; append the replay flag without cloning a JSON tree.
            let replay = format!("{},\"replayed\":true}}", &event[..event.len() - 1]);
            yield Ok(Event::default().data(replay));
        }
        loop {let received=tokio::select!{_=shutdown.changed()=>break,event=rx.recv()=>event};match received {
            Ok(event)=>yield Ok(Event::default().data(event.as_ref())),
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
fn locale_search_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = vec![
        std::path::PathBuf::from("web/locales"),
        std::path::PathBuf::from("locales"),
    ];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            dirs.push(exe_dir.join("locales"));
            if let Some(parent) = exe_dir.parent() {
                dirs.push(parent.join("locales"));
                dirs.push(parent.join("web/locales"));
            }
        }
    }
    dirs
}

fn embedded_locale(code: &str) -> Option<&'static str> {
    match code {
        "en" => Some(include_str!("../web/locales/en.json")),
        "ko" => Some(include_str!("../web/locales/ko.json")),
        "zh-TW" => Some(include_str!("../web/locales/zh-TW.json")),
        _ => None,
    }
}

fn load_all_locales() -> BTreeMap<String, Value> {
    let mut locales = BTreeMap::new();
    for code in ["en", "ko", "zh-TW"] {
        if let Some(raw) = embedded_locale(code) {
            if let Ok(val @ Value::Object(_)) = serde_json::from_str::<Value>(raw) {
                locales.insert(code.to_string(), val);
            }
        }
    }

    for dir in locale_search_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Some(code) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if let Ok(raw) = std::fs::read_to_string(&path) {
                if let Ok(val @ Value::Object(_)) = serde_json::from_str::<Value>(&raw) {
                    locales.insert(code.to_string(), val);
                }
            }
        }
    }
    locales
}

fn load_single_locale(code: &str) -> Option<Value> {
    if code.is_empty()
        || !code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    let filename = format!("{code}.json");
    for dir in locale_search_dirs().into_iter().rev() {
        let path = dir.join(&filename);
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Ok(val @ Value::Object(_)) = serde_json::from_str::<Value>(&raw) {
                return Some(val);
            }
        }
    }
    let raw = embedded_locale(code)?;
    match serde_json::from_str::<Value>(raw) {
        Ok(val @ Value::Object(_)) => Some(val),
        _ => None,
    }
}

async fn list_locales() -> Json<Value> {
    let locales = load_all_locales();
    let list: Vec<Value> = locales
        .iter()
        .map(|(code, dict)| {
            let name = dict
                .get("_name")
                .and_then(|v| v.as_str())
                .unwrap_or(code.as_str());
            json!({ "code": code, "name": name })
        })
        .collect();
    Json(Value::Array(list))
}

async fn get_locale(Path(code): Path<String>) -> Result<Json<Value>, ApiError> {
    match load_single_locale(&code) {
        Some(dict) => Ok(Json(dict)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("Locale '{code}' not found") })),
        )),
    }
}

async fn get_js(Path(file): Path<String>) -> Response {
    let content = match file.as_str() {
        "state.js" => Some(include_str!("../web/js/state.js")),
        "analytics.js" => Some(include_str!("../web/js/analytics.js")),
        "routes.js" => Some(include_str!("../web/js/routes.js")),
        "log.js" => Some(include_str!("../web/js/log.js")),
        "api.js" => Some(include_str!("../web/js/api.js")),
        "main.js" => Some(include_str!("../web/js/main.js")),
        _ => None,
    };
    match content {
        Some(body) => (
            [("content-type", "application/javascript; charset=utf-8")],
            body,
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("Script '{file}' not found") })),
        )
            .into_response(),
    }
}

pub fn router(app: Shared) -> Router {
    Router::new()
        .route("/", get(|| async { Html(include_str!("../web/ui.html")) }))
        .route(
            "/css/style.css",
            get(|| async {
                (
                    [("content-type", "text/css; charset=utf-8")],
                    include_str!("../web/css/style.css"),
                )
            }),
        )
        .route(
            "/i18n.js",
            get(|| async {
                (
                    [("content-type", "application/javascript; charset=utf-8")],
                    include_str!("../web/i18n.js"),
                )
            }),
        )
        .route("/js/{file}", get(get_js))
        .route("/locales", get(list_locales))
        .route("/locales/{code}", get(get_locale))
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
                app.data.lock().unwrap().clear_history();
                Json(json!({"ok":true}))
            }),
        )
        .route("/capture/start", post(crate::capture::start))
        .route("/capture/stop", post(crate::capture::stop))
        .with_state(app)
}

#[cfg(test)]
mod memory_tests {
    use super::*;
    use axum::body::to_bytes;

    fn announce(app: &Shared, asn: u32) {
        app.apply_update(
            json!({
                "announce":{"ipv4-unicast":[{"prefix":"192.0.2.0/24"}]},
                "path_attributes":[
                    {"name":"AS_PATH","value":[{"asns":[asn]}]},
                    {"name":"COMMUNITIES","value":[format!("65000:{asn}")]}
                ]
            }),
            "192.0.2.1",
        );
    }

    #[test]
    fn continuous_updates_do_not_postpone_persistence() {
        let path = std::env::temp_dir().join(format!("bgpx-churn-{}.json", std::process::id()));
        let app = App::new(Some(path.to_string_lossy().into_owned()));
        announce(&app, 1);
        let dirty = Instant::now() - Duration::from_secs(6);
        app.data.lock().unwrap().dirty = Some(dirty);
        announce(&app, 2);
        assert_eq!(app.data.lock().unwrap().dirty, Some(dirty));
        app.flush(false).unwrap();
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["routes"][0]["as_path"], json!([2]));
        assert!(app.data.lock().unwrap().dirty.is_none());
        app.data.lock().unwrap().clear();
        app.flush(true).unwrap();
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["count"], 0);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn churn_reclaims_unused_attributes_and_teardown_releases_capacity() {
        let app = App::new(None);
        announce(&app, 1);
        let old = app
            .data
            .lock()
            .unwrap()
            .slab
            .iter()
            .flatten()
            .next()
            .unwrap()
            .clone();
        let old_attrs = Arc::downgrade(old.attrs());
        for asn in 2..1000 {
            announce(&app, asn);
        }
        app.reclaim_memory();
        // An export snapshot still needs the old attributes.
        assert!(old_attrs.upgrade().is_some());
        drop(old);
        app.reclaim_memory();
        assert!(old_attrs.upgrade().is_none());
        {
            let data = app.data.lock().unwrap();
            assert_eq!(data.ids.len(), 1);
            assert_eq!(data.pool.attrs.len(), 1);
            // 999 re-announces of one prefix leave holes that compaction squeezed out.
            assert_eq!(data.slab.len(), 1);
        }
        app.apply_update(
            json!({"withdraw":{"ipv4-unicast":[{"prefix":"192.0.2.0/24"}]}}),
            "192.0.2.1",
        );
        app.reclaim_memory();
        {
            let data = app.data.lock().unwrap();
            assert!(data.pool.attrs.is_empty());
        }
        announce(&app, 1000);
        let mut data = app.data.lock().unwrap();
        assert!(data.ids.capacity() > 0);
        assert!(data.pool.attrs.capacity() > 0);
        data.clear();
        assert_eq!(data.ids.capacity(), 0);
        assert_eq!(data.pool.attrs.capacity(), 0);
        assert_eq!(data.slab.capacity(), 0);
    }

    #[tokio::test]
    async fn history_is_shared_bounded_and_reset() {
        let app = App::new(None);
        let mut rx = app.events.subscribe();
        app.emit("session", "info", "shared", json!({}));
        let event = rx.recv().await.unwrap();
        assert!(Arc::ptr_eq(
            &event,
            app.data.lock().unwrap().history.back().unwrap()
        ));
        for _ in 0..HISTORY_EVENTS + 1 {
            app.emit("session", "info", "small", json!({}));
        }
        assert_eq!(app.data.lock().unwrap().history.len(), HISTORY_EVENTS);
        for _ in 0..10 {
            app.emit(
                "announce",
                "update",
                "large",
                json!({"payload":"x".repeat(1024 * 1024)}),
            );
        }
        {
            let data = app.data.lock().unwrap();
            assert!(data.history.len() < 10);
            assert!(data.history_bytes <= HISTORY_BYTES);
            assert_eq!(
                data.history_bytes,
                data.history.iter().map(|s| s.len()).sum::<usize>()
            );
        }
        // An oversized event remains available live but cannot exceed the replay budget.
        app.emit(
            "announce",
            "update",
            "oversized",
            json!({"payload":"x".repeat(HISTORY_BYTES)}),
        );
        assert!(app.data.lock().unwrap().history_bytes <= HISTORY_BYTES);
        app.data.lock().unwrap().clear_history();
        app.emit("session", "info", "after clear", json!({}));
        app.stop().await;
        let data = app.data.lock().unwrap();
        assert_eq!(data.history.len(), 1); // only the stop event
        assert_eq!(data.history_bytes, data.history[0].len());
    }

    #[tokio::test]
    async fn stalled_export_releases_snapshot_and_permit_without_body_polling() {
        let app = App::new(None);
        announce(&app, 1);
        let rows: Vec<Entry> = app
            .data
            .lock()
            .unwrap()
            .slab
            .iter()
            .flatten()
            .cloned()
            .collect();
        let old = Arc::downgrade(rows[0].attrs());
        let permit = app.exports.clone().try_acquire_owned().unwrap();
        let body = export_body(rows, permit, Duration::from_millis(10));
        app.data.lock().unwrap().clear();
        tokio::time::timeout(Duration::from_secs(2), async {
            while old.upgrade().is_some() || app.exports.available_permits() != 2 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(to_bytes(body, 100_000).await.is_err());
    }

    #[tokio::test]
    async fn export_limit_and_disconnect_release() {
        let app = App::new(None);
        announce(&app, 1);
        let page = Page::default;
        let first = export(State(app.clone()), Query(page())).await.unwrap();
        let second = export(State(app.clone()), Query(page())).await.unwrap();
        assert_eq!(
            export(State(app.clone()), Query(page()))
                .await
                .unwrap_err()
                .0,
            StatusCode::TOO_MANY_REQUESTS
        );
        drop(first);
        drop(second);
        tokio::time::timeout(Duration::from_secs(2), async {
            while app.exports.available_permits() != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let response = export(State(app), Query(page())).await.unwrap();
        let bytes = to_bytes(response.into_body(), 100_000).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap()["routes"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
}

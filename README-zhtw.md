# bgpx

內建 Live Web UI 的 BGP Unicast 與 FlowSpec receiver — 原生 Rust 版本，版本 26.9.21。
可連接 peer router，在記憶體中維護 RIB，並透過 Server-Sent Events (SSE) 將所有內容即時串流至瀏覽器。

```bash
cargo install --path . --locked
bgpx --local-as 65001 --router-id 192.0.2.2 --peer-ip 192.0.2.1 --peer-as 65000
# 開啟 http://localhost:8080
```

`bgpx` 執行檔內建了 Web UI，並直接以 Rust 執行 BGP、RIB、HTTP 和 SSE。執行時不需要 Python 與 PyO3。`tcpdump` 為選用，只有在需要 packet capture 時才需要安裝。舊版的 Python/PyO3 後端已被移除，但其 parser 輸出已保留作為 Rust 的 regression test fixtures。

> **BGP session 僅支援 IPv4。** `--peer-ip` 與 `--router-id` 必須是 IPv4 addresses。  
> 透過該 session 接收的路由則可以是 IPv4 或是 IPv6、Unicast 或 FlowSpec。

---

## 功能特點 (Features)

- **雙模式連線 (Dual-mode connection)** — 同時發起 active connect 與被動監聽 accept；先成功者優先。
- **IPv4 + IPv6 FlowSpec** (RFC 8955/8956) — 支援所有 NLRI 類型：prefix、port、protocol、TCP flags、DSCP、fragment、flow-label。
- **IPv4 + IPv6 Unicast** — 支援 prefix、next-hop、AS path、標準 / well-known / large communities。
- **所有標準 FlowSpec actions** — rate-limit (bps/pps)、discard、redirect-to-VRF、redirect-to-IP、DSCP mark、traffic-action。
- **4-byte ASN** — `AS_TRANS`, `CAP_4BYTE_ASN`, `AS4_PATH` (RFC 6793)。
- **Hold-timer 強制執行** — timeout 時會重設 session。
- **JSON RIB persistence** — 透過 debounced atomic write 將路由表儲存至檔案中 (`--json-output`)。
- **Web UI** — 支援排序的路由表、帶有 filter chips 的 live log、Analytics、packet capture 檢視器，以及針對 Unicast RIB 的 `show ip bgp <ip>` LPM (Longest Prefix Match) 搜尋。

---

## 快速開始 (Quick start)

```bash
# 僅啟動 Web UI — 可直接在瀏覽器中設定 BGP session
bgpx

# 自動啟動 session
bgpx --local-as 65001 --router-id 10.0.0.1 \
     --peer-ip 10.0.0.2 --peer-as 65000

# 包含 JSON 輸出與 DEBUG log
bgpx --local-as 65001 --router-id 10.0.0.1 \
     --peer-ip 10.0.0.2 --peer-as 65000 \
     --json-output /tmp/routes.json --log-level DEBUG
```

### Docker

```bash
docker build -t bgpx .
# 若需啟用 packet capture，請加上：--cap-add=NET_RAW --cap-add=NET_ADMIN
# 注意：在標準 Linux 主機上 bind port 179 可能需要 root 權限。
# 您可以改用 `-p 9179:9179` 並將 `--listen-port 9179` 參數傳給 bgpx。
docker run --rm -p 179:179 -p 8080:8080 bgpx
```

---

## CLI Flags

| 參數 | 預設值 | 描述 |
|---|---|---|
| `--local-as` | — | Local AS number |
| `--router-id` | — | Local BGP router-id (IPv4) |
| `--peer-ip` | — | BGP peer IP (IPv4) |
| `--peer-as` | — | BGP peer AS number |
| `--hold-time` | `90` | Hold time 秒數 (`0` = 停用，否則至少為 `3`) |
| `--reconnect-delay` | `5` | 斷線後 reconnect 的延遲秒數 |
| `--connect-timeout` | `5.0` | TCP connect timeout |
| `--active-retry-delay` | `1.0` | 兩次 active connect 之間的延遲時間 |
| `--listen-port` | `179` | 被動監聽的 BGP port |
| `--json-output` | — | 每次批次變更後，將 RIB 寫入此檔案 |
| `--host` | `0.0.0.0` | Web UI 監聽 IP |
| `--port` | `8080` | Web UI 監聽 port |
| `--log-level` | `INFO` | `DEBUG` / `INFO` / `WARNING` / `ERROR` |

> **綁定 Port 179** 需要 root 權限或執行：
> ```bash
> sudo setcap cap_net_bind_service+ep "$(readlink -f "$(command -v bgpx)")"
> ```

---

## 網頁介面 (Web UI)

打開 `http://localhost:8080`。Session 的設定會被存放在瀏覽器的 `localStorage` 中。

| 面板 | 顯示內容 |
|---|---|
| Sidebar | 設定並 start/stop BGP session |
| **Total / Unicast / FlowSpec** tabs | 支援分頁與排序的路由表 |
| **Analytics** tab | Family/AFI counts、top communities、origin AS、next-hops、prefix lengths、FlowSpec actions/protocols/ports |
| **Live Log** tab | SSE event stream — 可用 SESSION / ANNOUNCE / WITHDRAW / ERROR / PCAP 進行過濾；點擊可展開檢視 JSON |
| **◉ Capture** | 開始/停止 BGP 流量的 `tcpdump` (環境變數 `$PATH` 必須要有 `tcpdump`) |
| **⬇ Export** | 將目前的路由表下載為 JSON |
| **🔍 Search** 搜尋列 | 針對 Unicast RIB 提供類似 `show ip bgp <ip>` 的 LPM (Longest Prefix Match) 搜尋 (`GET /routes/search?ip=`) |

記憶體限制：Event replay 功能最多保留 2,000 個 events 或 8 MiB 的 JSON（以先達到者為準）；超過限制的大型 event 將只支援即時傳送。手動 stop session 會清除伺服器端的 replay history。即時串流會 buffer 64 個 events；如果 Client 落後太多，將會收到一份全新的 snapshot。Export 允許最多兩個 concurrent snapshots，並在 60 秒後過期（額外的 requests 會回傳 HTTP 429）。未使用的 interned 路由屬性及閒置的 index 會在每 30 秒執行一次的 background worker 中回收；session 結束後也會釋放相關記憶體。

上方狀態指示器：
- **SSE 狀態燈** — 綠燈 = live，黃燈閃爍 = reconnecting
- **狀態標籤** — `IDLE` → `CONNECT` → `OPEN_SENT` → `OPEN_CONFIRMED` → `ESTABLISHED`

> `announce`/`withdraw` 的 SSE events 會將同一個 BGP UPDATE message 打包成一個 batch — 每條 message 會觸發一個包含 `{"count", "routes": [...], "path_attributes"}` 的 event，而不是每一筆路由一個獨立 event。這能避免在初次同步 full-table 時，產生過多訊息塞爆 broadcast channel 與 Live Log。

---

## HTTP API 與 Monitoring

除了 Web UI 外，`bgpx` 也提供以下 HTTP endpoints：

- `GET /health` — 當 BGP session 狀態為 `ESTABLISHED` 時回傳 `200 OK`，否則回傳 `503 Service Unavailable`。非常適合 Load Balancer 或 Kubernetes 的 readiness probe。
- `GET /routes/search?ip=<ip>` — 針對 Unicast RIB 進行 LPM 查詢。

---

## JSON RIB 格式

```json
{
  "count": 2,
  "routes": [
    {
      "id": "98d8d25ae319",
      "family": "unicast",
      "afi": "ipv4-unicast",
      "peer": "10.0.0.2",
      "received_at": "2026-06-25T12:00:00+00:00",
      "prefix": "192.0.2.0/24",
      "next_hop": "10.0.0.2",
      "as_path": [65000, 64496],
      "communities": ["65000:100", "NO_EXPORT"],
      "path_attributes": []
    },
    {
      "id": "a3f1b2c4d5e6",
      "family": "flowspec",
      "afi": "ipv4-flowspec",
      "peer": "10.0.0.2",
      "received_at": "2026-06-04T12:00:00+00:00",
      "match": {
        "dst-prefix": "203.0.113.0/24",
        "ip-proto": ["=tcp(6)"],
        "dst-port": ["=80", "=443"]
      },
      "actions": ["discard"],
      "path_attributes": []
    }
  ]
}
```

`traffic-rate-bytes` 會根據 RFC 8955 解碼為 bytes/second，在 Web UI 上顯示時會自動轉換為 bps — 例如路由器發送的 `0.1 Mbps` 會顯示為 `rate-limit=100000bps`。

---

## RFC 支援範圍

### Supported & Partially Supported RFCs

| RFC | 範圍 | 狀態 | 備註 / 限制 |
|---|---|---|---|
| RFC 4271 | BGP-4 / IPv4 Unicast | 🟡 Partial | 僅 receiver；FSM、timers、OPEN/UPDATE/KEEPALIVE、IPv4 unicast NLRI。**未實作**：發送 outbound UPDATE、發生 parsing error 時產生標準的 NOTIFICATION、BGP over IPv6 傳輸、針對多個 peers 的 best-path selection。 |
| RFC 1997 | BGP Communities | ✅ Full | 標準 communities (`ASN:val`) 與 well-known communities (`NO_EXPORT`, `NO_ADVERTISE`, `NO_EXPORT_SUBCONFED`, `NOPEER`)。 |
| RFC 4360 | Extended Communities | ✅ Full | 2-byte/4-byte/IPv4-specific extended communities 以及 FlowSpec actions (rate-limit, redirect to VRF 等)。 |
| RFC 4760 | MP-BGP | 🟡 Partial | Capability 1, 針對 AFI 1/2 與 SAFI 1/133 的 `MP_REACH_NLRI` / `MP_UNREACH_NLRI`。**未實作**：其他 SAFI (例如 SAFI 128 L3VPN, SAFI 4 MPLS, SAFI 2 Multicast, SAFI 134 FlowSpec VPN)。 |
| RFC 5492 | Capabilities Advertisement | ✅ Full | OPEN 訊息中 Optional Parameter Type 2。 |
| RFC 5701 | IPv6 Specific Ext. Communities | ✅ Full | Type 25 extended communities (例如 IPv6 FlowSpec redirect actions)。 |
| RFC 6793 | 4-Byte ASN | ✅ Full | Capability 65, `AS_TRANS`, 4-octet `AS_PATH`, `AS4_PATH` 合併, `AS4_AGGREGATOR`。 |
| RFC 7606 | Revised UPDATE Error Handling | 🟡 Partial | 在不 drop session 的情況下 discard 錯誤屬性 / 保留 malformed attributes 的 raw hex。**未實作**：完整的 treat-as-withdraw state machine。 |
| RFC 8092 | BGP Large Communities | ✅ Full | 12-octet large communities (`admin:data1:data2`)。 |
| RFC 8955 | IPv4 FlowSpec | 🟡 Partial | 所有 component types (1–12) 與 actions (rate-limit bps/pps, discard, redirect to VRF/IP, DSCP mark, sample/terminal)。**未實作**：嚴格的 component 遞增排序檢查 (Section 5.1)、跟 unicast RIB 驗證的 FlowSpec route validation (Section 6)、Web UI 中明確的 AND/OR operator 分組。 |
| RFC 8956 | IPv6 FlowSpec | 🟡 Partial | Component types 1–13 (包含 flow-label) 與 IPv6 redirect actions。**未實作**：Prefix offset byte (Section 3.2；目前解碼為 `[length][prefix]`，不支援 offset $\ne 0$)。 |
| RFC 9184 | FlowSpec Redirect to IP | ✅ Full | Redirect-to-IP / copy-to-IP extended communities (`0x010C` / `0x000C`)。 |

### Out-of-Scope / Not Implemented RFCs

| RFC | 名稱 | 狀態 | 原因 / 營運影響 |
|---|---|---|---|
| RFC 2918 | Route Refresh Capability | ❌ Not implemented | 不支援 ROUTE-REFRESH message (Type 5)；需要重新建立 session 才能 refresh RIB。 |
| RFC 4724 | Graceful Restart Mechanism | ❌ Not implemented | 不會廣播 Capability 64；session 終止時會立即 purge 路由。 |
| RFC 7911 | Advertisement of Multiple Paths (ADD-PATH) | ❌ Not implemented | 不支援 NLRI 中的 Path Identifier prefix；預設單一 prefix 只有一條 path。 |
| RFC 8654 | Extended Message Support (64K BGP messages) | ❌ Not implemented | 根據 RFC 4271，嚴格限制最大 message size 為 4,096 bytes。 |
| RFC 5065 | Autonomous System Confederations | ❌ Not implemented | 會 parse 並顯示 confederation segment types，但省略了 confederation peer / loop detection 邏輯。 |
| RFC 2385 / 5925 | TCP MD5 Signature / TCP-AO | ❌ Not implemented | 在 socket 層級不支援 TCP authentication options。 |
| RFC 7999 | BLACKHOLE Community | ❌ Not implemented | `65535:666` 會被 parse 為一般的 community，而不是特定的 named action。 |
| RFC 8212 | EBGP Route Propagation without Policies | ❌ Not implemented | 未實作 inbound route policy / filter engine。 |

---

## 架構 (Architecture)

*BGP message 在 `wire.rs` 進行原生 parsing，存放在記憶體中的 RIB (`app.rs`)，並立即透過 Server-Sent Events (SSE) broadcast 到 Web frontend。*

```
src/
├── main.rs         CLI、HTTP listener、shutdown 與 persistence worker
├── session.rs      Tokio BGP 連線、FSM、timers 及 UPDATE dispatch
├── app.rs          RIB、analytics、HTTP routes 與 SSE history
├── capture.rs      tcpdump lifecycle 與 packet events
├── wire.rs         原生 BGP parsing 與 message 構建
└── wire_helpers.rs 既有的 Rust FlowSpec 解碼 helper
web/ui.html         內建的單一檔案 Vanilla JS Web UI
```

---

## 開發 (Development)

```bash
cargo build --locked --release
cargo test --locked
./test.sh               # fmt, Clippy, 原生 test, shell/JS 語法檢查
# cargo test 包含 324 個 parser compatibility fixtures
# Python 3 為選用，僅用於獨立的 HTTP/SSE smoke test。
```

---

## 安裝 (Installation)

您可以使用內附的 script 輕鬆將 `bgpx` 安裝成 systemd service：

```bash
sudo ./deploy.sh --service --cap-net-bind-service
```

若需完整的主機 deploy 教學、自訂路徑，或使用 `uninstall.sh` 徹底移除，請參考 [INSTALL.md](INSTALL.md)。

**從 Python 版本升級：** 舊版的 Python 後端與 PyO3 dependency 已經完全移除。如果您要升級較舊的 deployment，只需重新執行 `./deploy.sh --service`，系統便會將其替換為原生的 Rust binary。所有現有的 config 皆會無縫保留。

---

## License

PolyForm Noncommercial License 1.0.0 — 允許個人、研究、教育、政府及 public-benefit 用途。商業用途需取得書面許可。

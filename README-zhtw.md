# bgpx

帶有即時網頁介面的 BGP Unicast 與 FlowSpec 接收器 — 原生 Rust 版本，版本 26.9.21。
可連接對等路由器 (peer router)，維護記憶體中的路由訊息庫 (RIB)，並透過 Server-Sent Events (SSE) 將所有內容即時串流至瀏覽器。

```bash
cargo install --path . --locked
bgpx --local-as 65001 --router-id 192.0.2.2 --peer-ip 192.0.2.1 --peer-as 65000
# 開啟 http://localhost:8080
```

`bgpx` 執行檔內建了網頁介面，並直接以 Rust 執行 BGP、RIB、HTTP 和 SSE 功能。執行時不再需要 Python 與 PyO3 環境。`tcpdump` 為選用項目，僅在需要進行封包擷取時才需要安裝。舊版的 Python/PyO3 後端已被移除，但其解析器輸出已保留作為 Rust 的迴歸測試基準。

> **BGP 會話僅支援 IPv4。** `--peer-ip` 與 `--router-id` 必須是 IPv4 地址。  
> 透過該會話接收的路由則可為 IPv4 或 IPv6、Unicast 或 FlowSpec。

---

## 功能特點

- **雙模式連線 (Dual-mode connection)** — 同時發起主動連線與被動接收連線；先成功者優先。
- **IPv4 + IPv6 FlowSpec** (RFC 8955/8956) — 支援所有 NLRI 類型：前綴 (prefix)、埠號 (port)、協定 (protocol)、TCP 標記 (flags)、DSCP、分片 (fragment)、流標籤 (flow-label)。
- **IPv4 + IPv6 Unicast** — 支援前綴、下一跳 (next-hop)、AS path、標準 / 知名 (well-known) / 大型 (large) 社群屬性。
- **所有標準 FlowSpec 動作** — 速率限制 (bps/pps)、丟棄 (discard)、重新導向至 VRF (redirect-to-VRF)、重新導向至 IP (redirect-to-IP)、DSCP 標記、流量動作 (traffic-action)。
- **4-byte ASN** — `AS_TRANS`, `CAP_4BYTE_ASN`, `AS4_PATH` (RFC 6793)。
- **Hold-timer 強制執行** — 逾時將重設連線。
- **JSON RIB 持久化** — 透過防彈跳 (debounced) 的原子寫入機制，將路由表儲存至檔案中 (`--json-output`)。
- **網頁介面** — 支援排序的路由表、帶有過濾標籤的即時日誌、數據分析、封包擷取檢視器，以及針對 Unicast RIB 的 `show ip bgp <ip>` 最長前綴匹配 (LPM) 搜尋。

---

## 快速開始

```bash
# 僅啟動網頁介面 — 可於瀏覽器中設定 BGP 會話
bgpx

# 自動啟動會話
bgpx --local-as 65001 --router-id 10.0.0.1 \
     --peer-ip 10.0.0.2 --peer-as 65000

# 包含 JSON 輸出與除錯日誌
bgpx --local-as 65001 --router-id 10.0.0.1 \
     --peer-ip 10.0.0.2 --peer-as 65000 \
     --json-output /tmp/routes.json --log-level DEBUG
```

### Docker

```bash
docker build -t bgpx .
# 若需啟用封包擷取功能，請加上：--cap-add=NET_RAW --cap-add=NET_ADMIN
# 注意：在標準的 Linux 主機上綁定 179 埠可能需要 root 權限。
# 您可以改用 `-p 9179:9179` 並將 `--listen-port 9179` 參數傳遞給 bgpx。
docker run --rm -p 179:179 -p 8080:8080 bgpx
```

---

## 命令列參數

| 參數 | 預設值 | 描述 |
|---|---|---|
| `--local-as` | — | 本地 AS 號碼 |
| `--router-id` | — | 本地 BGP router-id (IPv4) |
| `--peer-ip` | — | BGP 對等端 IP (IPv4) |
| `--peer-as` | — | BGP 對等端 AS 號碼 |
| `--hold-time` | `90` | Hold time 秒數 (`0` = 停用，否則至少為 `3`) |
| `--reconnect-delay` | `5` | 斷線後重新連線的延遲秒數 |
| `--connect-timeout` | `5.0` | TCP 連線超時秒數 |
| `--active-retry-delay` | `1.0` | 主動嘗試連線之間的延遲 |
| `--listen-port` | `179` | 被動 BGP 監聽埠號 |
| `--json-output` | — | 每次批次變更後，將 RIB 寫入至此檔案 |
| `--host` | `0.0.0.0` | 網頁介面監聽地址 |
| `--port` | `8080` | 網頁介面監聽埠號 |
| `--log-level` | `INFO` | `DEBUG` / `INFO` / `WARNING` / `ERROR` |

> **綁定 Port 179** 需要 root 權限或執行：
> ```bash
> sudo setcap cap_net_bind_service+ep "$(readlink -f "$(command -v bgpx)")"
> ```

---

## 網頁介面 (Web UI)

請開啟 `http://localhost:8080`。會話設定會儲存於 `localStorage` 中。

| 面板 | 顯示內容 |
|---|---|
| 側邊欄 (Sidebar) | 設定並啟動/停止 BGP 會話 |
| **Total / Unicast / FlowSpec** 分頁 | 支援分頁與排序的路由表 |
| **Analytics** (分析) 分頁 | Family/AFI 統計、熱門社群屬性、來源 AS、下一跳、前綴長度、FlowSpec 動作/協定/埠號 |
| **Live Log** (即時日誌) 分頁 | SSE 事件串流 — 可依 SESSION / ANNOUNCE / WITHDRAW / ERROR / PCAP 進行過濾；點擊可展開檢視 JSON |
| **◉ Capture** (擷取) | 開始/停止擷取 BGP 流量的 `tcpdump` (需要系統 `$PATH` 環境變數中包含 `tcpdump`) |
| **⬇ Export** (匯出) | 將目前的路由表視圖下載為 JSON 檔案 |
| **🔍 Search** 搜尋列 | 針對 Unicast RIB 提供類似 `show ip bgp <ip>` 的最長前綴匹配查詢 (`GET /routes/search?ip=`) |

記憶體限制：事件重播功能最多保留 2,000 個事件或 8 MiB 的序列化 JSON（以先達到者為準）；較大的單一事件僅提供即時傳送。手動停止會話將清除伺服器端的重播歷史紀錄。即時傳輸會緩衝 64 個事件；若客戶端落後過多，將會收到一份全新的快照。匯出功能允許同時進行兩個快照，並在 60 秒後過期（額外的請求會回傳 HTTP 429）。未使用的內部路由屬性及閒置的索引空間會在每 30 秒執行一次的維護程式中被回收；會話結束後會釋放相關的資料容器。

頂部狀態指示器：
- **SSE 狀態燈** — 綠色 = 連線中，黃色閃爍 = 重新連線中
- **狀態徽章** — `IDLE` → `CONNECT` → `OPEN_SENT` → `OPEN_CONFIRMED` → `ESTABLISHED`

> `announce`/`withdraw` 的 SSE 事件會依照每個 BGP UPDATE 訊息進行批次處理 — 每條訊息的事件會包含 `{"count", "routes": [...], "path_attributes"}`，而非每個路由產生一個獨立事件。這可避免初次同步完整路由表時，過多的訊息淹沒廣播頻道與即時日誌。

---

## HTTP API 與監控

除了網頁介面外，`bgpx` 也提供以下 HTTP 端點：

- `GET /health` — 當 BGP 會話狀態為 `ESTABLISHED` 時回傳 `200 OK`，否則回傳 `503 Service Unavailable`。非常適合用於負載平衡器或 Kubernetes 的就緒探針 (readiness probes)。
- `GET /routes/search?ip=<ip>` — 針對 Unicast RIB 進行最長前綴匹配查詢。

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

`traffic-rate-bytes` 會依照 RFC 8955 規範解碼為 bytes/second，並於顯示時轉換為網路的 bits/second — 例如路由器發送的 `0.1 Mbps` 會顯示為 `rate-limit=100000bps`。

---

## RFC 支援範圍

### 支援與部分支援的 RFC

| RFC | 範圍 | 狀態 | 備註 / 限制 |
|---|---|---|---|
| RFC 4271 | BGP-4 / IPv4 Unicast | 🟡 部分支援 | 僅接收端；FSM、計時器、OPEN/UPDATE/KEEPALIVE、IPv4 unicast NLRI。**未實作**：發送出站 UPDATE、發生解析錯誤時產生標準 NOTIFICATION、IPv6 傳輸 BGP、多個對等端的最佳路徑選擇演算法。 |
| RFC 1997 | BGP Communities | ✅ 完整支援 | 標準社群屬性 (`ASN:val`) 與知名社群屬性 (`NO_EXPORT`, `NO_ADVERTISE`, `NO_EXPORT_SUBCONFED`, `NOPEER`)。 |
| RFC 4360 | Extended Communities | ✅ 完整支援 | 2-byte/4-byte/IPv4-specific 擴展社群屬性及 FlowSpec 動作 (rate-limit, redirect to VRF 等)。 |
| RFC 4760 | MP-BGP | 🟡 部分支援 | Capability 1, 針對 AFI 1/2 與 SAFI 1/133 的 `MP_REACH_NLRI` / `MP_UNREACH_NLRI`。**未實作**：其他 SAFI (例如 SAFI 128 L3VPN, SAFI 4 MPLS, SAFI 2 Multicast, SAFI 134 FlowSpec VPN)。 |
| RFC 5492 | Capabilities Advertisement | ✅ 完整支援 | OPEN 訊息中 Optional Parameter Type 2。 |
| RFC 5701 | IPv6 Specific Ext. Communities | ✅ 完整支援 | Type 25 擴展社群屬性 (例如 IPv6 FlowSpec 重新導向動作)。 |
| RFC 6793 | 4-Byte ASN | ✅ 完整支援 | Capability 65, `AS_TRANS`, 4-octet `AS_PATH`, `AS4_PATH` 合併, `AS4_AGGREGATOR`。 |
| RFC 7606 | Revised UPDATE Error Handling | 🟡 部分支援 | 在不中斷連線的情況下丟棄屬性 / 保留格式錯誤屬性的原始十六進位資料。**未實作**：完整的 treat-as-withdraw 狀態機。 |
| RFC 8092 | BGP Large Communities | ✅ 完整支援 | 12-octet 大型社群屬性 (`admin:data1:data2`)。 |
| RFC 8955 | IPv4 FlowSpec | 🟡 部分支援 | 所有組件類型 (1–12) 與動作 (rate-limit bps/pps, discard, redirect to VRF/IP, DSCP mark, sample/terminal)。**未實作**：嚴格的遞增組件順序檢查 (章節 5.1)、與 unicast RIB 核對的 FlowSpec 路由驗證 (章節 6)、使用者介面中明確的 AND/OR 運算子分組。 |
| RFC 8956 | IPv6 FlowSpec | 🟡 部分支援 | 組件類型 1–13 (包含 flow-label) 與 IPv6 重新導向動作。**未實作**：前綴偏移量位元組 (章節 3.2；目前解碼為 `[length][prefix]`，不支援 offset $\ne 0$)。 |
| RFC 9184 | FlowSpec Redirect to IP | ✅ 完整支援 | Redirect-to-IP / copy-to-IP 擴展社群屬性 (`0x010C` / `0x000C`)。 |

### 不在範圍內 / 未實作的 RFC

| RFC | 名稱 | 狀態 | 原因 / 營運影響 |
|---|---|---|---|
| RFC 2918 | Route Refresh Capability | ❌ 未實作 | 不支援 ROUTE-REFRESH 訊息 (Type 5)；需要重新建立連線才能刷新 RIB。 |
| RFC 4724 | Graceful Restart Mechanism | ❌ 未實作 | 不發送 Capability 64 廣播；連線終止時將立即清除路由。 |
| RFC 7911 | Advertisement of Multiple Paths (ADD-PATH) | ❌ 未實作 | 不支援 NLRI 中的 Path Identifier 前綴；預設單一前綴只有單一路徑。 |
| RFC 8654 | Extended Message Support (64K BGP messages) | ❌ 未實作 | 依據 RFC 4271，嚴格限制最大訊息大小為 4,096 bytes。 |
| RFC 5065 | Autonomous System Confederations | ❌ 未實作 | 會解析並顯示聯邦區段類型，但省略了聯邦對等連線/迴圈檢測邏輯。 |
| RFC 2385 / 5925 | TCP MD5 Signature / TCP-AO | ❌ 未實作 | 在 socket 層級不支援 TCP 認證選項。 |
| RFC 7999 | BLACKHOLE Community | ❌ 未實作 | 社群屬性 `65535:666` 會被解析為通用社群，而非特定命名的動作。 |
| RFC 8212 | EBGP Route Propagation without Policies | ❌ 未實作 | 未實作入站路由策略/過濾引擎。 |

---

## 架構

*BGP 訊息在 `wire.rs` 中進行原生解析，儲存於記憶體中的 RIB (`app.rs`)，並立即透過 Server-Sent Events (SSE) 廣播至前端。*

```
src/
├── main.rs         CLI、HTTP 監聽器、關機與持久化作業處理
├── session.rs      Tokio BGP 連線、FSM、計時器及 UPDATE 訊息分發
├── app.rs          RIB、資料分析、HTTP 路由與 SSE 歷史紀錄
├── capture.rs      tcpdump 生命週期與封包事件
├── wire.rs         原生 BGP 解析與訊息建構
└── wire_helpers.rs 既有的 Rust FlowSpec 解碼輔助函式
web/ui.html         內建單一檔案 Vanilla JS 網頁介面
```

---

## 開發

```bash
cargo build --locked --release
cargo test --locked
./test.sh               # fmt, Clippy, 原生測試, shell/JS 語法檢查
# cargo test 包含 324 個解析器相容性夾具 (fixtures)
# Python 3 為選用，僅用於獨立的 HTTP/SSE 冒煙測試。
```

---

## 安裝

您可以使用內附的腳本輕鬆將 `bgpx` 安裝為 systemd 服務：

```bash
sudo ./deploy.sh --service --cap-net-bind-service
```

若需完整的主機部署指南、自訂路徑，或使用 `uninstall.sh` 進行完整解除安裝，請參閱 [INSTALL.md](INSTALL.md)。

**從 Python 版本升級：** 舊版的 Python 後端與 PyO3 依賴項已完全移除。如果您要升級較舊的部署，只需重新執行 `./deploy.sh --service`，系統便會將其替換為原生的 Rust 執行檔。所有現有的設定皆會無縫保留。

---

## 授權

PolyForm Noncommercial License 1.0.0 — 允許個人、研究、教育、政府及公共利益用途。商業用途需取得書面許可。

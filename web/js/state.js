// ── Centralized Reactive State Store (AppState) ────────────────────────────
const AppState = {
  // Session & connection state
  sseState: 'reconnecting',
  bgpState: 'IDLE',
  sessionRunning: false,
  capturing: false,
  peerIp: '',
  peerAs: '',
  peerInfo: {},

  // RIB statistics & event counters
  ribStats: { total: 0, unicast: 0, flowspec: 0, ipv4: 0, ipv6: 0, analytics: {} },
  announceCount: 0,
  withdrawCount: 0,

  // Routes table & pagination state
  routeRows: [],
  routeTotal: 0,
  routesLoaded: false,
  currentView: 'total',
  currentPage: 1,
  pageSize: 50,
  sortCol: 'received_at',
  sortAsc: false,
  routeFilter: '',
  flashRouteId: null,
  expandedRouteIds: new Set(),

  // Search state
  searchResult: null,

  // Live Log state
  logCount: 0,
  logFilter: 'all',
  logSearch: '',
  logPaused: false,
  autoScroll: true,

  // Layout state
  sidebarOpen: true,
  activeTab: 'total',

  // Pub/Sub listeners: Map<topic, Set<fn>>
  _listeners: {},

  on(topic, fn) {
    if (!this._listeners[topic]) this._listeners[topic] = new Set();
    this._listeners[topic].add(fn);
    return () => this._listeners[topic].delete(fn);
  },

  emit(topic, payload) {
    const set = this._listeners[topic];
    if (set) set.forEach(fn => fn(payload, this));
  },

  update(patch, topics = []) {
    Object.assign(this, patch);
    topics.forEach(topic => this.emit(topic, this));
  },

  resetRib() {
    this.expandedRouteIds.clear();
    this.update({
      routeRows: [],
      routeTotal: 0,
      ribStats: { total: 0, unicast: 0, flowspec: 0, ipv4: 0, ipv6: 0, analytics: {} },
      currentPage: 1,
      routeFilter: '',
      flashRouteId: null,
    }, ['rib', 'routes']);
  },
};

// ── DOM & Formatting Helpers ───────────────────────────────────────────────
function $(id) {
  return document.getElementById(id);
}

function setText(id, text) {
  const el = $(id);
  if (el) el.textContent = text;
}

function createEl(tag, attrs = {}, children = []) {
  const el = document.createElement(tag);
  Object.entries(attrs).forEach(([k, v]) => {
    if (v === undefined || v === null) return;
    if (k === 'className') el.className = v;
    else if (k === 'textContent') el.textContent = v;
    else if (k === 'onclick') el.addEventListener('click', v);
    else if (k === 'dataset') Object.entries(v).forEach(([dk, dv]) => { el.dataset[dk] = dv; });
    else el.setAttribute(k, v);
  });
  children.forEach(child => {
    if (typeof child === 'string') el.appendChild(document.createTextNode(child));
    else if (child instanceof Node) el.appendChild(child);
  });
  return el;
}

function absLocalTime(iso) {
  if (!iso) return '—';
  const d = new Date(iso);
  if (isNaN(d.getTime())) return '—';
  return new Date(d - d.getTimezoneOffset() * 60000).toISOString().slice(0, 19).replace('T', ' ');
}

function formatServerTime(iso) {
  return iso ? iso.replace(/T(\d{2}:\d{2}:\d{2})(?:\.\d+)?([+-]\d{2}:\d{2}|Z)/, ' $1 $2').replace(' Z', ' UTC') : '—';
}

function escHtml(s) {
  return String(s)
    .replace(/&/g,'&amp;').replace(/</g,'&lt;')
    .replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}

function toast(msg, type = 'ok') {
  const wrap = $('toasts');
  if (!wrap) return;
  const el = createEl('div', { className: `toast ${type}`, textContent: msg });
  wrap.appendChild(el);
  setTimeout(() => el.remove(), 4000);
}

function copyText(btn, text) {
  navigator.clipboard.writeText(text).then(() => {
    btn.textContent = t('log.copied');
    setTimeout(() => { btn.textContent = t('log.copy'); }, 1500);
  });
}

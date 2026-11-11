// ── SSE Stream & Session/Capture API (Driven by AppState) ──────────────────
let evtSource = null;

function connectSSE() {
  if (evtSource) evtSource.close();
  AppState.update({ sseState: 'reconnecting' }, ['sse']);
  evtSource = new EventSource('/events');

  evtSource.onopen = () => AppState.update({ sseState: 'live' }, ['sse']);

  evtSource.onmessage = e => {
    try {
      const event = JSON.parse(e.data);

      if (event.type === 'snapshot') {
        handleSnapshot(event);
        return;
      }

      // Log tab: always append (replayed events fill history)
      appendLogEntry(event);

      // State updates only from live (non-replayed) events
      if (!event.replayed) handleLiveEvent(event);
    } catch (_) {}
  };

  evtSource.onerror = () => {
    AppState.update({ sseState: 'reconnecting' }, ['sse']);
    evtSource.close();
    setTimeout(connectSSE, 3000);
  };
}

function renderSseDot() {
  const state = AppState.sseState;
  const dot = $('sse-dot');
  if (!dot) return;
  dot.className = 'sse-dot ' + state;
  dot.title = state === 'live' ? t('sse.live') : t('sse.reconnecting');
}

// ── Snapshot (initial state on SSE connect) ────────────────────────────────
function handleSnapshot(event) {
  const s = event.status || {};
  const running = Boolean(s.running);

  AppState.update({
    currentPage: 1,
    announceCount: 0,
    withdrawCount: 0,
    ribStats: event.stats || AppState.ribStats,
    sessionRunning: running,
    capturing: Boolean(s.capture_running),
    bgpState: s.state || 'IDLE',
    peerIp: s.peer_ip || '',
    peerAs: s.peer_as || '',
    peerInfo: s.peer_info || {},
  }, ['session', 'capture', 'rib', 'counters']);

  if (running) {
    populateForm(s);
  } else {
    restoreForm();
  }
  loadRoutes();
}

// ── Live event handler ─────────────────────────────────────────────────────
function handleLiveEvent(event) {
  if (event.type === 'announce' && event.count) {
    queueRouteReload(event.routes.find(r => r.route_id)?.route_id);
  } else if (event.type === 'withdraw' && event.count) {
    queueRouteReload();
  } else if (event.type === 'session') {
    const patch = {};
    if (event.state !== undefined) {
      patch.bgpState = event.state;
      if (event.state === 'IDLE') {
        patch.sessionRunning = false;
        patch.capturing = false;
        AppState.resetRib();
      }
    }
    if (event.running !== undefined) {
      patch.sessionRunning = Boolean(event.running);
      if (event.peer_ip !== undefined) patch.peerIp = event.peer_ip;
      if (event.peer_as !== undefined) patch.peerAs = event.peer_as;
      if (event.running) {
        populateForm(event);
      } else {
        patch.peerIp = '';
        patch.peerAs = '';
        patch.peerInfo = {};
        patch.capturing = false;
        AppState.resetRib();
      }
    }
    if (event.peer_info) {
      patch.peerInfo = event.peer_info;
    }
    AppState.update(patch, ['session', 'capture']);
  } else if (event.type === 'capture') {
    if (event.running !== undefined) {
      AppState.update({ capturing: Boolean(event.running) }, ['capture']);
    }
  }
}

// ── State badge & Peer info rendering ──────────────────────────────────────
function renderSessionStatus() {
  const state = AppState.bgpState || 'IDLE';
  const badge = $('state-badge');
  const label = $('state-label');
  if (badge && label) {
    badge.className = 'state-badge';
    label.textContent = t('state.' + state);
    if (state === 'ESTABLISHED')
      badge.classList.add('established');
    else if (['CONNECT', 'OPEN_SENT', 'OPEN_CONFIRMED'].includes(state))
      badge.classList.add('connecting');
    else if (state === 'ERROR')
      badge.classList.add('error');
  }

  const peerLabel = $('peer-label');
  if (peerLabel) {
    if (AppState.peerIp) {
      peerLabel.replaceChildren(
        createEl('span', { textContent: String(AppState.peerIp) }),
        ' AS ',
        createEl('span', { textContent: String(AppState.peerAs || '') }),
      );
    } else if (!AppState.sessionRunning) {
      peerLabel.textContent = '';
    }
  }

  renderPeerInfo();
  updateFormState();
}

function renderPeerInfo() {
  const info = AppState.peerInfo || {};
  const section = $('peer-info-section');
  const grid    = $('peer-info-grid');
  if (!section || !grid) return;
  if (!Object.keys(info).length) {
    section.style.display = 'none';
    return;
  }
  const labels = {
    version: t('peer_info.version'),
    peer_as: t('peer_info.peer_as'),
    hold_time: t('peer_info.hold_time'),
    router_id: t('peer_info.router_id'),
  };
  const cells = Object.entries(info).flatMap(([k, v]) => [
    createEl('span', { className: 'k', textContent: labels[k] || k }),
    createEl('span', { className: 'v', textContent: String(v) }),
  ]);
  grid.replaceChildren(...cells);
  section.style.display = '';
}

// ── Session control ────────────────────────────────────────────────────────
async function startSession() {
  const config = collectConfig();
  if (!config) return;
  saveForm(config);
  setFormDisabled(true);
  $('start-btn').disabled = true;

  try {
    const r = await fetch('/session/start', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(config),
    });
    const data = await r.json();
    if (!r.ok) throw new Error(data.error || r.statusText);
    toast(t('toast.session_starting'), 'ok');
    AppState.update({
      sessionRunning: true,
      peerIp: config.peer_ip,
      peerAs: config.peer_as,
    }, ['session']);
  } catch (e) {
    toast(String(e), 'error');
    setFormDisabled(false);
    $('start-btn').disabled = false;
  }
}

async function stopSession() {
  $('stop-btn').disabled = true;
  try {
    await fetch('/session/stop', { method: 'POST' });
    toast(t('toast.session_stopped'), 'ok');
    AppState.update({
      sessionRunning: false,
      capturing: false,
      peerIp: '',
      peerAs: '',
      peerInfo: {},
    }, ['session', 'capture']);
    AppState.resetRib();
  } catch (e) {
    toast(String(e), 'error');
    $('stop-btn').disabled = false;
  }
}

// ── Packet capture ─────────────────────────────────────────────────────────
function renderCaptureButton() {
  const btn = $('capture-btn');
  if (!btn) return;
  btn.disabled = !AppState.sessionRunning;
  if (AppState.capturing) {
    btn.textContent = t('btn.stop_capture');
    btn.classList.add('capturing');
  } else {
    btn.textContent = t('btn.capture');
    btn.classList.remove('capturing');
  }
}

async function toggleCapture() {
  if (!AppState.sessionRunning) {
    toast(t('toast.no_active_session'), 'error');
    return;
  }
  const btn = $('capture-btn');
  btn.disabled = true;
  try {
    if (!AppState.capturing) {
      const r    = await fetch('/capture/start', { method: 'POST' });
      const data = await r.json();
      if (!r.ok) { toast(data.error || t('toast.capture_failed'), 'error'); return; }
      switchTab('log');
    } else {
      await fetch('/capture/stop', { method: 'POST' });
    }
  } catch (e) {
    toast(String(e), 'error');
  } finally {
    btn.disabled = !AppState.sessionRunning;
  }
}

// Subscribe to state changes
AppState.on('sse', renderSseDot);
AppState.on('session', renderSessionStatus);
AppState.on('capture', renderCaptureButton);

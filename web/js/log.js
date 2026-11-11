// ── Live Log Panel, Pause/Resume & Keyword Filtering ───────────────────────
function setLogFilter(f) {
  AppState.update({ logFilter: f }, ['logFilter']);
}

function setLogSearch(query) {
  AppState.update({ logSearch: (query || '').trim().toLowerCase() }, ['logFilter']);
}

function toggleLogPause() {
  AppState.update({ logPaused: !AppState.logPaused }, ['logPause']);
}

function renderLogPauseButton() {
  const btn = $('log-pause-btn');
  if (!btn) return;
  const paused = AppState.logPaused;
  btn.textContent = paused ? t('log.resume') : t('log.pause');
  btn.classList.toggle('paused', paused);
}

function renderLogFilter() {
  const f = AppState.logFilter;
  document.querySelectorAll('.filter-chip[data-filter]').forEach(c =>
    c.classList.toggle('active', c.dataset.filter === f)
  );
  document.querySelectorAll('#log-wrap .log-entry').forEach(el => {
    el.classList.toggle(
      'hidden',
      !matchesFilter(el.dataset.type, el.dataset.level, el.dataset.search || ''),
    );
  });
}

function matchesFilter(type, level, searchText = '') {
  const f = AppState.logFilter;
  let typeOk = true;
  if (f === 'error') typeOk = type === 'error' || level === 'warning';
  else if (f === 'capture') typeOk = type === 'capture';
  else if (f !== 'all') typeOk = type === f;
  if (!typeOk) return false;

  if (AppState.logSearch && !searchText.includes(AppState.logSearch)) {
    return false;
  }
  return true;
}

function appendLogEntry(event) {
  const wrap  = $('log-wrap');
  const empty = $('log-empty');
  if (empty) empty.style.display = 'none';

  if (event.type === 'announce') {
    AppState.update({ announceCount: AppState.announceCount + (event.count || 0) }, ['counters']);
  } else if (event.type === 'withdraw') {
    AppState.update({ withdrawCount: AppState.withdrawCount + (event.count || 0) }, ['counters']);
  }

  if (AppState.logPaused && !event.replayed) return;
  if ((event.type === 'announce' || event.type === 'withdraw')
      && !$('tab-log').classList.contains('active')) return;

  const msgNode = createLogMsgEl(event);
  const searchable = `${event.type || ''} ${event.level || ''} ${msgNode.textContent || ''}`.toLowerCase();

  const entry = createEl('div', {
    className: 'log-entry' + (matchesFilter(event.type, event.level, searchable) ? '' : ' hidden'),
    dataset: {
      type: event.type || '',
      level: event.level || '',
      search: searchable,
    },
    onclick: () => entry.classList.toggle('expanded'),
  }, [
    createEl('span', { className: 'log-ts', textContent: absLocalTime(event.ts) }),
    createLogBadgeEl(event),
    msgNode,
  ]);

  const detailData = { ...event };
  ['ts', 'message', 'type', 'level', 'replayed'].forEach(k => delete detailData[k]);
  const detailJson = Object.keys(detailData).length
    ? JSON.stringify(detailData, null, 2) : t('log.no_extra_data');

  const copyBtn = createEl('button', {
    className: 'copy-btn',
    textContent: t('log.copy'),
    onclick: e => { e.stopPropagation(); copyText(copyBtn, detailJson); },
  });
  const detail = createEl('div', { className: 'log-detail' }, [
    copyBtn,
    createEl('span', { textContent: detailJson }),
  ]);

  wrap.appendChild(entry);
  wrap.appendChild(detail);

  // Keep DOM bounded
  const entries = wrap.querySelectorAll('.log-entry');
  if (entries.length > 500) {
    const excess = entries.length - 500;
    for (let i = 0; i < excess; i++) {
      const next = entries[i].nextSibling;
      entries[i].remove();
      if (next && next.classList.contains('log-detail')) next.remove();
    }
  }

  AppState.logCount++;
  setText('log-count', AppState.logCount);
  if (AppState.autoScroll && !AppState.logPaused && !event.replayed) {
    wrap.scrollTop = wrap.scrollHeight;
  }
}

function createLogBadgeEl(event) {
  let cls = 'badge-session', lbl = (event.type || 'unknown').toUpperCase();
  if (event.type === 'announce')      { cls = 'badge-announce'; lbl = t('log.badge.announce'); }
  else if (event.type === 'withdraw') { cls = 'badge-withdraw'; lbl = t('log.badge.withdraw'); }
  else if (event.type === 'error')    { cls = 'badge-error';    lbl = t('log.badge.error'); }
  else if (event.level === 'warning') { cls = 'badge-warning';  lbl = t('log.badge.warn'); }
  else if (event.type === 'capture')  { cls = 'badge-capture';  lbl = t('log.badge.pcap'); }
  else if (event.level === 'packet')  { cls = 'badge-packet';   lbl = t('log.badge.pkt'); }
  return createEl('span', { className: `log-badge ${cls}`, textContent: lbl });
}

function createLogMsgEl(event) {
  const msgEl = createEl('span', { className: 'log-msg' }, [event.message || '']);
  if ((event.type === 'announce' || event.type === 'withdraw') && event.routes?.length) {
    const shown = event.routes.slice(0, 3).map(r => r.family === 'unicast'
      ? `prefix=${r.prefix || ''}`
      : Object.entries(r.match || {}).map(([k, v]) => `${k}=${Array.isArray(v) ? v.join(' ') : v}`).join(' '));
    if (event.routes.length > shown.length) {
      shown.push(t('log.more_routes', { count: event.routes.length - shown.length }));
    }
    const actions = event.routes[0].actions;
    if (actions && actions.length) shown.push('→ ' + actions.join(', '));
    const extraSpan = createEl('span', { textContent: '  ' + shown.join('  ') });
    extraSpan.style.color = 'var(--text2)';
    msgEl.appendChild(extraSpan);
  }
  return msgEl;
}

async function clearLog() {
  await fetch('/log', { method: 'DELETE' });
  const wrap = $('log-wrap');
  if (wrap) {
    wrap.replaceChildren(
      createEl('div', {
        id: 'log-empty',
        className: 'log-empty',
        dataset: { i18n: 'log.cleared' },
        textContent: t('log.cleared'),
      }),
    );
  }
  AppState.update({
    logCount: 0,
    announceCount: 0,
    withdrawCount: 0,
  }, ['counters']);
  setText('log-count', 0);
}

AppState.on('logFilter', renderLogFilter);
AppState.on('logPause', renderLogPauseButton);

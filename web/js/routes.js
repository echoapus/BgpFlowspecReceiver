// ── Routes Table, Filtering, Pagination & LPM Search (Subscribed to AppState) ─
// ponytail: refresh at most once per second during an update burst
let reloadTimer = null;
let pendingFlashId = null;
let routeRequest = 0;
let filterDebounceTimer = null;

function queueRouteReload(flashId) {
  if (flashId) pendingFlashId = flashId;
  if (reloadTimer) return;
  reloadTimer = setTimeout(() => {
    reloadTimer = null;
    loadRoutes(pendingFlashId);
    pendingFlashId = null;
  }, 1000);
}

function onRouteFilterInput(val) {
  const clearBtn = $('route-filter-clear');
  if (clearBtn) clearBtn.style.display = val.trim() ? '' : 'none';
  if (filterDebounceTimer) clearTimeout(filterDebounceTimer);
  filterDebounceTimer = setTimeout(() => {
    AppState.update({ routeFilter: val.trim(), currentPage: 1 });
    loadRoutes();
  }, 200);
}

function applyRouteFilter(value, targetView) {
  const input = $('route-filter-input');
  if (input) input.value = value;
  const clearBtn = $('route-filter-clear');
  if (clearBtn) clearBtn.style.display = value ? '' : 'none';
  AppState.update({ routeFilter: value, currentPage: 1 });
  switchTab(targetView || AppState.currentView || 'total');
}

function clearRouteFilter() {
  const input = $('route-filter-input');
  if (input) input.value = '';
  const clearBtn = $('route-filter-clear');
  if (clearBtn) clearBtn.style.display = 'none';
  AppState.update({ routeFilter: '', currentPage: 1 });
  loadRoutes();
}

async function loadRoutes(flashId) {
  const request = ++routeRequest;
  const params = new URLSearchParams({
    family: AppState.currentView,
    page: AppState.currentPage,
    page_size: AppState.pageSize,
    sort: AppState.sortCol,
    order: AppState.sortAsc ? 'asc' : 'desc',
  });
  if (AppState.routeFilter) {
    params.set('filter', AppState.routeFilter);
  }
  try {
    const response = await fetch('/routes?' + params);
    const data = await response.json();
    if (!response.ok) throw new Error(data.error || response.statusText);
    if (request !== routeRequest) return;
    AppState.update({
      routeRows: data.routes || [],
      routeTotal: data.count || 0,
      routesLoaded: true,
      ribStats: data.stats || AppState.ribStats,
      flashRouteId: flashId || null,
    }, ['rib', 'routes']);
  } catch (e) {
    setText('page-info', t('routes.load_failed'));
  }
}

function renderRoutes(flashId) {
  const tbody  = $('routes-tbody');
  const routes = AppState.routeRows;
  const activeFlash = flashId !== undefined ? flashId : AppState.flashRouteId;
  const filterInput = $('route-filter-input');
  if (filterInput && document.activeElement !== filterInput && filterInput.value !== AppState.routeFilter) {
    filterInput.value = AppState.routeFilter;
    const clearBtn = $('route-filter-clear');
    if (clearBtn) clearBtn.style.display = AppState.routeFilter ? '' : 'none';
  }
  renderRouteHeader();

  if (!routes.length) {
    const emptyKey = AppState.currentView === 'unicast'
      ? 'routes.empty_unicast'
      : AppState.currentView === 'flowspec'
        ? 'routes.empty_flowspec'
        : (!AppState.sessionRunning && !AppState.routeFilter)
          ? 'routes.empty_initial'
          : 'routes.empty_total';
    const emptyRow = createEl('tr', { className: 'empty' }, [
      createEl('td', { colspan: String(routeColumnCount()), textContent: t(emptyKey) }),
    ]);
    tbody.replaceChildren(emptyRow);
    setText('page-info', t('routes.no_routes'));
    $('btn-prev-page').disabled = true;
    $('btn-next-page').disabled = true;
    return;
  }

  const totalRoutes = AppState.routeTotal;
  const totalPages = Math.ceil(totalRoutes / AppState.pageSize);
  if (AppState.currentPage > totalPages) AppState.currentPage = totalPages || 1;
  if (AppState.currentPage < 1) AppState.currentPage = 1;

  const startIdx = (AppState.currentPage - 1) * AppState.pageSize;
  const endIdx = Math.min(startIdx + AppState.pageSize, totalRoutes);

  const fragment = document.createDocumentFragment();
  routes.forEach(r => {
    fragment.appendChild(createRouteRowEl(r, activeFlash));
    const isExpanded = AppState.expandedRouteIds.has(r.id);
    const detailRow = createEl('tr', {
      className: isExpanded ? 'route-detail-row open' : 'route-detail-row',
      id: `detail-${r.id}`,
    }, [
      createEl('td', { className: 'route-detail-cell', colspan: String(routeColumnCount()) }, [
        createRouteDetailPanelEl(r),
      ]),
    ]);
    fragment.appendChild(detailRow);
  });
  tbody.replaceChildren(fragment);

  // ponytail: update pagination controls state
  setText('page-info', t('pagination.showing', {
    start: totalRoutes ? startIdx + 1 : 0,
    end: endIdx,
    total: totalRoutes,
  }));
  $('btn-prev-page').disabled = AppState.currentPage <= 1;
  $('btn-next-page').disabled = AppState.currentPage >= totalPages;
}

function createRouteDetailPanelEl(r) {
  const rows = [
    ['ID', r.id],
    [t('table.type'), `${r.family} (${r.afi})`],
    [t('table.peer'), r.peer],
    [t('table.received'), formatServerTime(r.received_at)],
  ];
  if (r.family === 'unicast') {
    rows.push(
      [t('table.prefix'), r.prefix || '—'],
      [t('table.next_hop'), r.next_hop || '—'],
      [t('table.as_path'), (r.as_path || []).join(' ') || '—'],
      [t('table.community'), (r.communities || []).join(' ') || '—'],
    );
  } else {
    Object.entries(r.match || {}).forEach(([k, v]) => {
      rows.push([`match.${k}`, Array.isArray(v) ? v.join(' ') : String(v)]);
    });
    rows.push([t('table.actions'), (r.actions || []).join(', ') || '—']);
  }

  const grid = createEl('div', { className: 'detail-grid' },
    rows.flatMap(([k, v]) => [
      createEl('span', { className: 'k', textContent: String(k) }),
      createEl('span', { className: 'v', textContent: String(v) }),
    ])
  );

  const rawJson = JSON.stringify(r, null, 2);
  const rawPre = createEl('pre', { className: 'detail-raw-pre', textContent: rawJson });
  const toggleRawBtn = createEl('button', {
    className: 'detail-raw-btn',
    textContent: `{ } ${t('routes.detail.raw_json')}`,
    onclick: e => {
      e.stopPropagation();
      rawPre.classList.toggle('open');
    },
  });
  const copyJsonBtn = createEl('button', {
    className: 'detail-raw-btn',
    textContent: t('log.copy'),
    onclick: e => {
      e.stopPropagation();
      copyText(copyJsonBtn, rawJson);
    },
  });

  const actionsRow = createEl('div', { className: 'detail-actions-row' }, [
    toggleRawBtn,
    copyJsonBtn,
  ]);

  return createEl('div', { className: 'detail-panel' }, [grid, actionsRow, rawPre]);
}

function clearRoutes() {
  AppState.resetRib();
}

// ponytail: pagination navigation handlers
function prevPage() {
  if (AppState.currentPage > 1) {
    AppState.currentPage--;
    loadRoutes();
  }
}

function nextPage() {
  const totalPages = Math.ceil(AppState.routeTotal / AppState.pageSize);
  if (AppState.currentPage < totalPages) {
    AppState.currentPage++;
    loadRoutes();
  }
}

function toggleRouteDetail(id) {
  if (AppState.expandedRouteIds.has(id)) {
    AppState.expandedRouteIds.delete(id);
  } else {
    AppState.expandedRouteIds.add(id);
  }
  const row = $('detail-' + id);
  if (row) row.classList.toggle('open', AppState.expandedRouteIds.has(id));
}

function sortRoutes(col) {
  if (AppState.sortCol === col) AppState.sortAsc = !AppState.sortAsc;
  else { AppState.sortCol = col; AppState.sortAsc = true; }
  AppState.currentPage = 1;
  loadRoutes();
}

function createHeaderCell(label, col) {
  if (!col) return createEl('th', { textContent: label });
  const arrow = AppState.sortCol === col ? (AppState.sortAsc ? ' ▲' : ' ▼') : '';
  return createEl('th', { onclick: () => sortRoutes(col) }, [
    label,
    createEl('span', { className: 'sort-arrow', textContent: arrow }),
  ]);
}

function renderRouteHeader() {
  const head = $('routes-head');
  if (!head) return;
  let cells;
  if (AppState.currentView === 'unicast') {
    cells = [
      createHeaderCell(t('table.afi'), 'afi'),
      createHeaderCell(t('table.prefix'), 'prefix'),
      createHeaderCell(t('table.community')),
      createHeaderCell(t('table.as_path')),
      createHeaderCell(t('table.next_hop'), 'next_hop'),
      createHeaderCell(t('table.peer'), 'peer'),
      createHeaderCell(t('table.received'), 'received_at'),
    ];
  } else if (AppState.currentView === 'flowspec') {
    cells = [
      createHeaderCell(t('table.afi'), 'afi'),
      createHeaderCell(t('table.match_criteria')),
      createHeaderCell(t('table.actions')),
      createHeaderCell(t('table.peer'), 'peer'),
      createHeaderCell(t('table.received'), 'received_at'),
    ];
  } else {
    cells = [
      createHeaderCell(t('table.type'), 'family'),
      createHeaderCell(t('table.afi'), 'afi'),
      createHeaderCell(t('table.route')),
      createHeaderCell(t('table.policy_path')),
      createHeaderCell(t('table.peer'), 'peer'),
      createHeaderCell(t('table.received'), 'received_at'),
    ];
  }
  head.replaceChildren(createEl('tr', {}, cells));
}

function routeColumnCount() {
  return AppState.currentView === 'unicast' ? 7 : AppState.currentView === 'flowspec' ? 5 : 6;
}

function createRouteRowEl(r, flashId) {
  const tr = createEl('tr', {
    className: r.id === flashId ? 'row-flash' : '',
    dataset: { id: r.id },
    onclick: () => toggleRouteDetail(r.id),
  });
  const receivedTd = createEl('td', {
    className: 'ts-cell',
    title: formatServerTime(r.received_at),
    textContent: absLocalTime(r.received_at),
  });

  if (AppState.currentView === 'unicast') {
    tr.append(
      createEl('td', {}, [createAfiBadgeEl(r.afi)]),
      createEl('td', { textContent: r.prefix || '' }),
      createEl('td', {}, createCommunityBadgeEls(r.communities || [])),
      createEl('td', { textContent: (r.as_path || []).join(' ') || '—' }),
      createEl('td', { textContent: r.next_hop || '—' }),
      createEl('td', { className: 'id-cell', textContent: r.peer || '' }),
      receivedTd,
    );
    return tr;
  }

  if (AppState.currentView === 'flowspec') {
    tr.append(
      createEl('td', {}, [createAfiBadgeEl(r.afi)]),
      createEl('td', { className: 'match-cell' }, createMatchContentEls(r.match)),
      createEl('td', {}, createActionBadgeEls(r.actions || [])),
      createEl('td', { className: 'id-cell', textContent: r.peer || '' }),
      receivedTd,
    );
    return tr;
  }

  const routeCell = r.family === 'unicast'
    ? createEl('td', { className: 'match-cell', textContent: r.prefix || '' })
    : createEl('td', { className: 'match-cell' }, createMatchContentEls(r.match));
  const policyCell = r.family === 'unicast'
    ? createEl('td', { textContent: (r.as_path || []).join(' ') || '—' })
    : createEl('td', {}, createActionBadgeEls(r.actions || []));

  tr.append(
    createEl('td', { textContent: r.family || '' }),
    createEl('td', {}, [createAfiBadgeEl(r.afi)]),
    routeCell,
    policyCell,
    createEl('td', { className: 'id-cell', textContent: r.peer || '' }),
    receivedTd,
  );
  return tr;
}

function createAfiBadgeEl(afi) {
  const isV6 = afi && afi.includes('ipv6');
  return createEl('span', {
    className: `afi-badge ${isV6 ? 'ipv6' : 'ipv4'}`,
    textContent: isV6 ? 'IPv6' : 'IPv4',
  });
}

function createEmptyDashEl() {
  const span = createEl('span', { textContent: '—' });
  span.style.color = 'var(--text3)';
  return span;
}

function createMatchContentEls(match) {
  const entries = Object.entries(match || {});
  if (!entries.length) return [createEmptyDashEl()];

  const items = entries.map(([k, v]) => {
    const val = Array.isArray(v) ? v.join(' ') : String(v);
    return createEl('span', { className: 'match-item' }, [
      createEl('span', { className: 'match-key', textContent: `${k}:` }),
      ' ',
      createEl('span', { className: 'match-val', textContent: val }),
    ]);
  });

  const LIMIT = 3;
  const inner = createEl('div', { className: 'match-inner' }, items);
  if (items.length <= LIMIT) {
    inner.style.maxHeight = 'none';
    return [inner];
  }
  const count = items.length - LIMIT;
  const moreBtn = createEl('span', {
    className: 'match-more',
    dataset: { count: String(count) },
    textContent: t('match.more', { count }),
    onclick: e => {
      e.stopPropagation();
      const exp = inner.classList.toggle('expanded');
      moreBtn.textContent = exp ? t('match.less') : t('match.more', { count });
    },
  });
  return [inner, moreBtn];
}

function createActionBadgeEls(actions) {
  if (!actions || !actions.length) return [createEmptyDashEl()];
  return actions.map(a => {
    let cls = 'action-other';
    if (a === 'discard' || a === 'discard-packets') cls = 'action-discard';
    else if (a.startsWith('rate-limit'))            cls = 'action-rate-limit';
    else if (a.startsWith('redirect') || a.startsWith('rt-redirect') || a.startsWith('copy-to')) cls = 'action-redirect';
    else if (a.startsWith('mark'))                  cls = 'action-mark';
    else if (a.startsWith('unknown-flowspec'))       cls = 'action-unknown-ec';
    return createEl('span', {
      className: `action-badge clickable-badge ${cls}`,
      title: t('analytics.click_to_filter', { value: a }),
      textContent: formatRateLimit(a),
      onclick: e => {
        e.stopPropagation();
        applyRouteFilter(a, AppState.currentView);
      },
    });
  });
}

function createCommunityBadgeEls(communities) {
  if (!communities || !communities.length) return [createEmptyDashEl()];
  return communities.map(c =>
    createEl('span', {
      className: 'action-badge clickable-badge action-other',
      title: t('analytics.click_to_filter', { value: c }),
      textContent: c,
      onclick: e => {
        e.stopPropagation();
        applyRouteFilter(c, AppState.currentView);
      },
    })
  );
}

function formatRateLimit(a) {
  const m = a.match(/^rate-limit=(\d+)(bps|pps)$/);
  if (!m) return a;
  const n = parseInt(m[1], 10);
  if (n >= 1e9) return `rate-limit=${(n/1e9).toFixed(2).replace(/\.?0+$/, '')}G${m[2]}`;
  if (n >= 1e6) return `rate-limit=${(n/1e6).toFixed(2).replace(/\.?0+$/, '')}M${m[2]}`;
  if (n >= 1e3) return `rate-limit=${(n/1e3).toFixed(2).replace(/\.?0+$/, '')}K${m[2]}`;
  return a;
}

function exportRoutes() {
  const params = new URLSearchParams({ family: AppState.currentView });
  if (AppState.routeFilter) params.set('filter', AppState.routeFilter);
  window.location.href = '/routes/export?' + params;
}

// ── Longest-prefix match search (`show ip bgp <ip>`) ───────────────────────
async function searchRoute() {
  const ip = $('search-ip').value.trim();
  if (!ip) { closeSearchResult(); return; }
  try {
    const r = await fetch('/routes/search?ip=' + encodeURIComponent(ip));
    const data = await r.json();
    if (!r.ok) { toast(data.error || t('search.failed'), 'error'); return; }
    AppState.update({ searchResult: { ip, data } }, ['search']);
  } catch (_) {
    toast(t('search.failed'), 'error');
  }
}

function closeSearchResult() {
  AppState.update({ searchResult: null }, ['search']);
}

function renderSearchResult() {
  const panel = $('search-result');
  if (!panel) return;
  const res = AppState.searchResult;
  if (!res) {
    panel.style.display = 'none';
    return;
  }
  const { ip, data } = res;
  const titleText = data.match
    ? t('search.longest_match', { ip })
    : t('search.no_match', { ip });
  const header = createEl('h4', {}, [
    titleText,
    createEl('span', { className: 'close-btn', textContent: '×', onclick: closeSearchResult }),
  ]);

  if (!data.match) {
    const emptyMsg = createEl('div', { textContent: t('search.no_route_covers') });
    emptyMsg.style.color = 'var(--text2)';
    emptyMsg.style.fontSize = '12px';
    panel.replaceChildren(header, emptyMsg);
    panel.style.display = 'block';
    return;
  }

  const r = data.route;
  const rows = [
    [t('search.field.prefix'), `${r.prefix} (/${data.length})`],
    [t('search.field.next_hop'), r.next_hop || '—'],
    [t('search.field.as_path'), (r.as_path || []).join(' ') || '—'],
    [t('search.field.communities'), (r.communities || []).join(' ') || '—'],
    [t('search.field.peer'), r.peer],
    [t('search.field.received_at'), formatServerTime(r.received_at)],
  ];
  const table = createEl('table', {}, rows.map(([k, v]) =>
    createEl('tr', {}, [
      createEl('td', { textContent: k }),
      createEl('td', { textContent: String(v) }),
    ])
  ));
  panel.replaceChildren(header, table);
  panel.style.display = 'block';
}

// Subscribe to state changes
AppState.on('routes', () => renderRoutes());
AppState.on('search', renderSearchResult);

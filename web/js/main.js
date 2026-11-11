// ── UI Entry Point, Theme, Sidebar, Tabs, Shortcuts & Form Persistence ─────
function toggleTheme() {
  const isLight = document.documentElement.classList.toggle('light-theme');
  localStorage.setItem('theme', isLight ? 'light' : 'dark');
  updateThemeButton(isLight);
}

function updateThemeButton(isLight) {
  const btn = $('theme-toggle-btn');
  if (btn) {
    btn.textContent = isLight ? t('theme.dark') : t('theme.light');
  }
}

(function() {
  const savedTheme = localStorage.getItem('theme');
  const prefersLight = window.matchMedia && window.matchMedia('(prefers-color-scheme: light)').matches;
  const isLight = savedTheme === 'light' || (!savedTheme && prefersLight);
  if (isLight) {
    document.documentElement.classList.add('light-theme');
  }
  document.addEventListener('DOMContentLoaded', () => {
    updateThemeButton(isLight);
  });
})();

// ── Language switcher integration ──────────────────────────────────────────
function initLangSelector() {
  const sel = $('lang-select');
  if (!sel) return;
  const options = getAvailableLocales().map(l =>
    createEl('option', { value: l.code, textContent: `🌐 ${l.name}` })
  );
  sel.replaceChildren(...options);
  sel.value = getLang();
}

window.onLanguageChanged = function() {
  initLangSelector();
  const isLight = document.documentElement.classList.contains('light-theme');
  updateThemeButton(isLight);
  AppState.emit('sse');
  AppState.emit('session');
  AppState.emit('capture');
  AppState.emit('rib');
  AppState.emit('search');
  AppState.emit('logPause');
  if (AppState.routesLoaded) {
    AppState.emit('routes');
  } else {
    renderRouteHeader();
  }
  document.querySelectorAll('#log-wrap .copy-btn').forEach(btn => {
    btn.textContent = t('log.copy');
  });
};

// ── Sidebar & Tab controls ─────────────────────────────────────────────────
function toggleSidebar() {
  AppState.update({ sidebarOpen: !AppState.sidebarOpen }, ['layout']);
}

function renderLayout() {
  $('sidebar').classList.toggle('collapsed', !AppState.sidebarOpen);
  setText('sidebar-toggle', AppState.sidebarOpen ? '‹' : '›');
}

function switchTab(name) {
  document.querySelectorAll('.tab-btn').forEach(b => b.classList.remove('active'));
  document.querySelectorAll('.tab-content').forEach(c => c.classList.remove('active'));
  $('tab-' + name + '-btn').classList.add('active');
  if (['total', 'unicast', 'flowspec'].includes(name)) {
    AppState.update({ activeTab: name, currentView: name, currentPage: 1 });
    $('tab-routes').classList.add('active');
    loadRoutes();
  } else {
    AppState.update({ activeTab: name });
    $('tab-' + name).classList.add('active');
    if (name === 'analytics') renderAnalytics();
  }
  $('clear-btn').style.display = name === 'log' ? '' : 'none';
}

AppState.on('layout', renderLayout);

// ── Form validation & localStorage config persistence ──────────────────────
function updateFormState() {
  const running = AppState.sessionRunning;
  setFormDisabled(running);
  $('start-btn').disabled = running;
  $('stop-btn').disabled  = !running;
  renderCaptureButton();
}

function collectConfig() {
  const fields = {
    local_as:           $('f-local-as').value.trim(),
    router_id:          $('f-router-id').value.trim(),
    peer_ip:            $('f-peer-ip').value.trim(),
    peer_as:            $('f-peer-as').value.trim(),
    hold_time:          $('f-hold-time').value.trim(),
    reconnect_delay:    $('f-reconnect').value.trim(),
    connect_timeout:    $('f-connect-timeout').value.trim(),
    active_retry_delay: $('f-active-retry').value.trim(),
    json_output:        $('f-json-output').value.trim(),
    listen_port:        $('f-listen-port').value.trim(),
  };

  let ok = true;
  ['local-as', 'router-id', 'peer-ip', 'peer-as'].forEach(id => {
    const el = $('f-' + id);
    if (!el.value.trim()) { el.classList.add('invalid'); ok = false; }
    else el.classList.remove('invalid');
  });
  if (!ok) { toast(t('toast.fill_required'), 'error'); return null; }
  return fields;
}

function setFormDisabled(disabled) {
  ['f-local-as', 'f-router-id', 'f-peer-ip', 'f-peer-as',
   'f-hold-time', 'f-reconnect', 'f-connect-timeout', 'f-active-retry',
   'f-json-output', 'f-listen-port'].forEach(id => {
    $(id).disabled = disabled;
  });
}

function saveForm(config) {
  try { localStorage.setItem('bgpx-config', JSON.stringify(config)); } catch (_) {}
}

function restoreForm() {
  try {
    const saved = JSON.parse(localStorage.getItem('bgpx-config') || 'null');
    populateForm(saved);
  } catch (_) {}
}

function populateForm(config) {
  if (!config) return;
  const map = {
    'f-local-as': 'local_as', 'f-router-id': 'router_id',
    'f-peer-ip':  'peer_ip',  'f-peer-as':   'peer_as',
    'f-hold-time':'hold_time','f-reconnect':  'reconnect_delay',
    'f-connect-timeout':'connect_timeout','f-active-retry':'active_retry_delay',
    'f-json-output':'json_output','f-listen-port':'listen_port',
  };
  Object.entries(map).forEach(([elId, key]) => {
    if (config[key] != null) $(elId).value = config[key];
  });
}

// ── Keyboard shortcuts & Responsive sidebar ────────────────────────────────
function initKeyboardShortcuts() {
  document.addEventListener('keydown', e => {
    const tag = (e.target && e.target.tagName) || '';
    const isInput = tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT';

    if (e.key === '/' && !isInput && !e.ctrlKey && !e.metaKey) {
      e.preventDefault();
      if (AppState.activeTab === 'log') {
        $('log-search-input')?.focus();
      } else {
        $('route-filter-input')?.focus();
      }
    } else if (e.key === 'Escape') {
      if (AppState.searchResult) {
        closeSearchResult();
      } else if (isInput) {
        e.target.blur();
      } else if (AppState.routeFilter) {
        clearRouteFilter();
      }
    }
  });
}

function initResponsiveSidebar() {
  if (window.innerWidth <= 768 && AppState.sidebarOpen) {
    AppState.update({ sidebarOpen: false }, ['layout']);
  }
}

// ── Init ───────────────────────────────────────────────────────────────────
window.addEventListener('DOMContentLoaded', async () => {
  initResponsiveSidebar();
  initKeyboardShortcuts();
  await initI18n();
  connectSSE();
  restoreForm();

  $('log-wrap').addEventListener('scroll', e => {
    const el = e.target;
    AppState.autoScroll = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
  });

  // Remove invalid class on input
  document.querySelectorAll('.sidebar input').forEach(input => {
    input.addEventListener('input', () => input.classList.remove('invalid'));
  });
});

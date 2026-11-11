// ── RIB Summary Stats & Analytics View (Subscribed to AppState) ────────────
function updateRibStats() {
  const stats = AppState.ribStats || {};
  setStat('s-total', stats.total || 0);
  setStat('s-unicast', stats.unicast || 0);
  setStat('s-flowspec', stats.flowspec || 0);
  setText('total-count', stats.total || 0);
  setText('unicast-count', stats.unicast || 0);
  setText('flowspec-count', stats.flowspec || 0);
  renderAnalytics();
}

function setStat(id, val) {
  const el = $(id);
  if (!el) return;
  el.textContent = val;
  el.className = 'stat-val' + (val > 0 ? ' nonzero' : '');
}

function renderEventCounters() {
  const eventsEl = $('a-events');
  if (!eventsEl) return;
  eventsEl.replaceChildren(
    document.createTextNode(t('analytics.announce_count', { count: AppState.announceCount })),
    createEl('br'),
    document.createTextNode(t('analytics.withdraw_count', { count: AppState.withdrawCount })),
  );
}

function renderAnalytics() {
  const stats = AppState.ribStats || {};
  const a = stats.analytics || {};
  setText('a-total', stats.total || 0);
  setText('a-unicast', stats.unicast || 0);
  setText('a-flowspec', stats.flowspec || 0);

  const afiEl = $('a-afi');
  if (afiEl) {
    afiEl.replaceChildren(
      document.createTextNode(`IPv4 ${stats.ipv4 || 0}`),
      createEl('br'),
      document.createTextNode(`IPv6 ${stats.ipv6 || 0}`),
    );
  }
  renderEventCounters();

  renderCountBarsInto('a-communities', a.communities, '', 'unicast');
  renderCountBarsInto('a-origin-as', a.origin_as, 'AS', 'unicast');
  renderCountBarsInto('a-next-hops', a.next_hops, '', 'unicast');
  renderCountBarsInto('a-prefix-lengths', a.prefix_lengths, '', 'unicast');
  renderCountBarsInto('a-actions', a.actions, '', 'flowspec');
  renderCountBarsInto('a-protocols', a.protocols, '', 'flowspec');
  renderCountBarsInto('a-ports', a.ports, '', 'flowspec');
}

function renderCountBarsInto(containerId, rows = [], prefix = '', targetView = 'total') {
  const container = $(containerId);
  if (!container) return;
  if (!rows || !rows.length) {
    container.textContent = '—';
    return;
  }
  const max = rows[0]?.[1] || 1;
  const nodes = rows.map(([name, count]) => {
    const labelText = prefix + name;
    const fill = createEl('span', { className: 'analytics-bar-fill' });
    fill.style.width = `${(count / max) * 100}%`;
    return createEl('div', {
      className: 'analytics-bar clickable',
      title: t('analytics.click_to_filter', { value: labelText }),
      onclick: () => applyRouteFilter(labelText, targetView),
    }, [
      createEl('span', { textContent: labelText }),
      createEl('span', { className: 'analytics-bar-track' }, [fill]),
      createEl('span', { className: 'analytics-bar-count', textContent: String(count) }),
    ]);
  });
  container.replaceChildren(...nodes);
}

// Subscribe to state changes
AppState.on('rib', updateRibStats);
AppState.on('counters', renderEventCounters);

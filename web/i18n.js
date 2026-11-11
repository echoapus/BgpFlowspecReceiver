// ── bgpx UI Internationalization (i18n) Loader ──────────────────────────────
// Language packs live in individual JSON files under `web/locales/<lang>.json`.
// The backend automatically scans `web/locales/*.json` (and `locales/*.json`)
// and exposes them via `GET /locales` and `GET /locales/{code}`.

const BGPX_LOCALES = {};
let availableLocales = [
  { code: 'en', name: 'English' },
  { code: 'ko', name: '한국어' },
  { code: 'zh-TW', name: '繁體中文' },
];
let currentLang = 'en';

function detectPreferredLang(locales) {
  const codes = locales.map(l => l.code);
  try {
    const saved = localStorage.getItem('bgpx-lang');
    if (saved && codes.includes(saved)) return saved;
  } catch (_) {}

  const nav = (navigator.language || '').toLowerCase();
  const exact = codes.find(c => c.toLowerCase() === nav);
  if (exact) return exact;

  const prefix = nav.split('-')[0];
  if (prefix === 'zh' && codes.includes('zh-TW')) return 'zh-TW';
  const partial = codes.find(c => c.toLowerCase().startsWith(prefix));
  if (partial) return partial;

  return codes.includes('en') ? 'en' : (codes[0] || 'en');
}

async function fetchLocaleDict(code) {
  const r = await fetch('/locales/' + encodeURIComponent(code));
  if (!r.ok) throw new Error(`Failed to load locale: ${code}`);
  const dict = await r.json();
  BGPX_LOCALES[code] = dict;
  return dict;
}

async function initI18n() {
  try {
    const r = await fetch('/locales');
    if (r.ok) {
      const list = await r.json();
      if (Array.isArray(list) && list.length) {
        availableLocales = list;
      }
    }
  } catch (_) {}

  currentLang = detectPreferredLang(availableLocales);

  const tasks = [fetchLocaleDict(currentLang).catch(() => {})];
  if (currentLang !== 'en') {
    tasks.push(fetchLocaleDict('en').catch(() => {}));
  }
  await Promise.all(tasks);

  applyStaticI18n();
  if (typeof window.onLanguageChanged === 'function') {
    window.onLanguageChanged(currentLang);
  }
}

function t(key, vars) {
  const dict = BGPX_LOCALES[currentLang] || {};
  const fallback = BGPX_LOCALES['en'] || {};
  let str = dict[key] !== undefined ? dict[key] : (fallback[key] !== undefined ? fallback[key] : key);
  if (vars) {
    Object.entries(vars).forEach(([k, v]) => {
      str = str.replaceAll(`{${k}}`, String(v));
    });
  }
  return str;
}

function getLang() {
  return currentLang;
}

function getAvailableLocales() {
  return availableLocales;
}

function applyStaticI18n() {
  document.documentElement.lang = currentLang;
  if (BGPX_LOCALES[currentLang] || BGPX_LOCALES['en']) {
    document.title = t('app.title');
  }

  document.querySelectorAll('[data-i18n]').forEach(el => {
    const key = el.getAttribute('data-i18n');
    const val = t(key);
    if (val !== key) el.textContent = val;
  });
  document.querySelectorAll('[data-i18n-html]').forEach(el => {
    const key = el.getAttribute('data-i18n-html');
    const val = t(key);
    if (val !== key) el.innerHTML = val;
  });
  document.querySelectorAll('[data-i18n-placeholder]').forEach(el => {
    const key = el.getAttribute('data-i18n-placeholder');
    const val = t(key);
    if (val !== key) el.setAttribute('placeholder', val);
  });
  document.querySelectorAll('[data-i18n-title]').forEach(el => {
    const key = el.getAttribute('data-i18n-title');
    const val = t(key);
    if (val !== key) el.setAttribute('title', val);
  });
}

async function setLang(lang) {
  try {
    await fetchLocaleDict(lang);
  } catch (_) {
    if (!BGPX_LOCALES[lang]) return;
  }
  currentLang = lang;
  try {
    localStorage.setItem('bgpx-lang', lang);
  } catch (_) {}
  applyStaticI18n();
  if (typeof window.onLanguageChanged === 'function') {
    window.onLanguageChanged(lang);
  }
}

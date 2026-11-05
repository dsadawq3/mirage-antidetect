/* ============================================================================
   Mirage UI — application logic (app.js)
   ----------------------------------------------------------------------------
   Plain ES2020, classic script, no build step, no framework. Talks to the local
   REST API from docs/API.md (http://127.0.0.1:35500 by default) and falls back
   to mock.js when mock mode is on. All network errors surface as toasts.
   ========================================================================= */
(function () {
  'use strict';

  /* ── Constants ──────────────────────────────────────────────────────── */

  var LS = { api: 'mirage.api', token: 'mirage.token', mock: 'mirage.mock' };
  var DEFAULT_API = 'http://127.0.0.1:35500';

  var ROUTES = {
    profiles: { title: 'Профили', sub: '' },
    cookies: { title: 'Cookies', sub: '' },
    import: { title: 'Импорт', sub: '' },
    settings: { title: 'Настройки', sub: '' }
  };

  var OS_META = {
    windows: { label: 'Windows', icon: 'windows', versions: ['10', '11'], availOffset: 40, platform: 'Win32' },
    macos: { label: 'macOS', icon: 'macos', versions: ['10.15', '11', '12', '13', '14'], availOffset: 25, platform: 'MacIntel' },
    linux: { label: 'Linux', icon: 'linux', versions: ['5.15', '6.1', '6.5'], availOffset: 24, platform: 'Linux x86_64' }
  };

  var SCREENS = [
    { w: 1920, h: 1080, dpr: 1, os: null },
    { w: 1366, h: 768, dpr: 1, os: null },
    { w: 2560, h: 1440, dpr: 1, os: null },
    { w: 1440, h: 900, dpr: 1, os: null },
    { w: 1600, h: 900, dpr: 1, os: null },
    { w: 1920, h: 1200, dpr: 1, os: null },
    { w: 1536, h: 864, dpr: 1.25, os: 'windows' },
    { w: 2560, h: 1440, dpr: 1.5, os: 'windows' },
    { w: 1440, h: 900, dpr: 2, os: 'macos' },
    { w: 1512, h: 982, dpr: 2, os: 'macos' },
    { w: 1728, h: 1117, dpr: 2, os: 'macos' },
    { w: 2560, h: 1600, dpr: 2, os: 'macos' }
  ];

  var TIMEZONES = [
    'America/New_York', 'America/Chicago', 'America/Denver', 'America/Los_Angeles',
    'America/Phoenix', 'America/Toronto', 'America/Sao_Paulo', 'America/Mexico_City',
    'Europe/London', 'Europe/Paris', 'Europe/Berlin', 'Europe/Madrid', 'Europe/Rome',
    'Europe/Amsterdam', 'Europe/Warsaw', 'Europe/Stockholm', 'Europe/Moscow', 'Europe/Istanbul',
    'Asia/Kolkata', 'Asia/Tokyo', 'Asia/Seoul', 'Asia/Shanghai', 'Australia/Sydney'
  ];

  var WEBRTC_MODES = [{ v: 'real', label: 'real' }, { v: 'relay', label: 'relay' }, { v: 'off', label: 'off' }];

  /* ── Tiny DOM helper ────────────────────────────────────────────────── */

  function $(sel, root) { return (root || document).querySelector(sel); }

  function h(tag, attrs) {
    var n = document.createElement(tag);
    if (attrs) {
      for (var k in attrs) {
        if (!Object.prototype.hasOwnProperty.call(attrs, k)) continue;
        var v = attrs[k];
        if (v === null || v === undefined || v === false) continue;
        if (k === 'class') n.className = v;
        else if (k === 'text') n.textContent = v;
        else if (k === 'html') n.innerHTML = v;
        else if (k === 'value') n.value = v;
        else if (k === 'checked') n.checked = !!v;
        else if (k === 'disabled') n.disabled = !!v;
        else if (k === 'dataset') { for (var d in v) n.dataset[d] = v[d]; }
        else if (k.slice(0, 2) === 'on' && typeof v === 'function') n.addEventListener(k.slice(2).toLowerCase(), v);
        else n.setAttribute(k, v);
      }
    }
    for (var i = 2; i < arguments.length; i++) appendKid(n, arguments[i]);
    return n;
  }

  function appendKid(n, kid) {
    if (kid === null || kid === undefined || kid === false) return;
    if (Array.isArray(kid)) { for (var i = 0; i < kid.length; i++) appendKid(n, kid[i]); return; }
    if (kid instanceof Node) { n.appendChild(kid); return; }
    n.appendChild(document.createTextNode(String(kid)));
  }

  function clear(n) { while (n.firstChild) n.removeChild(n.firstChild); }

  /* ── Inline SVG icons (no external requests, no emoji) ──────────────── */

  var ICONS = {
    layers: '<path d="M12 2 2 7l10 5 10-5-10-5Z"/><path d="m2 17 10 5 10-5"/><path d="m2 12 10 5 10-5"/>',
    layout: '<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M3 9h18"/><path d="M9 21V9"/>',
    cookie: '<path d="M12 2a10 10 0 1 0 10 10 4 4 0 0 1-5-5 4 4 0 0 1-5-5Z"/><circle cx="9" cy="9" r=".6"/><circle cx="14" cy="14" r=".6"/><circle cx="15" cy="9" r=".6"/>',
    download: '<path d="M12 3v12"/><path d="m7 10 5 5 5-5"/><path d="M5 21h14"/>',
    upload: '<path d="M12 21V9"/><path d="m7 14 5-5 5 5"/><path d="M5 3h14"/>',
    cog: '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1Z"/>',
    search: '<circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/>',
    plus: '<path d="M12 5v14"/><path d="M5 12h14"/>',
    play: '<path d="M6 4l14 8-14 8Z" fill="currentColor" stroke="none"/>',
    stop: '<rect x="6" y="6" width="12" height="12" rx="2" fill="currentColor" stroke="none"/>',
    edit: '<path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z"/>',
    copy: '<rect x="9" y="9" width="12" height="12" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>',
    trash: '<path d="M3 6h18"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/><path d="M10 11v6"/><path d="M14 11v6"/>',
    globe: '<circle cx="12" cy="12" r="9"/><path d="M3 12h18"/><path d="M12 3a15 15 0 0 1 0 18 15 15 0 0 1 0-18Z"/>',
    check: '<path d="M20 6 9 17l-5-5"/>',
    x: '<path d="M18 6 6 18"/><path d="M6 6l12 12"/>',
    warning: '<path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z"/><path d="M12 9v4"/><path d="M12 17h.01"/>',
    info: '<circle cx="12" cy="12" r="9"/><path d="M12 16v-4"/><path d="M12 8h.01"/>',
    refresh: '<path d="M21 12a9 9 0 1 1-3-6.7L21 8"/><path d="M21 3v5h-5"/>',
    server: '<rect x="3" y="4" width="18" height="7" rx="2"/><rect x="3" y="13" width="18" height="7" rx="2"/><path d="M7 8h.01"/><path d="M7 17h.01"/>',
    lock: '<rect x="4" y="10" width="16" height="11" rx="2"/><path d="M8 10V7a4 4 0 0 1 8 0v3"/>',
    key: '<circle cx="7.5" cy="15.5" r="4.5"/><path d="m10.7 12.3 8.3-8.3"/><path d="M16 6l3 3"/><path d="M19 3l2 2"/>',
    external: '<path d="M14 4h6v6"/><path d="M20 4 10 14"/><path d="M20 14v5a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h5"/>',
    windows: '<path fill="currentColor" stroke="none" d="M3.5 3.8 11 2.8v8.2H3.5Zm8.5-1.2L20.5 1.5v9.5H12ZM3.5 12h7.5v8.2L3.5 20.2Zm8.5 0h8.5v9.5L12 20.2Z"/>',
    macos: '<path fill="currentColor" stroke="none" d="M16.6 12.7c0-2.1 1.7-3.1 1.8-3.2-1-1.4-2.5-1.6-3-1.6-1.3-.1-2.6.8-3.2.8-.7 0-1.7-.8-2.8-.8-1.4 0-2.8.8-3.5 2.1-1.5 2.6-.4 6.4 1 8.5.7 1 1.5 2.2 2.6 2.1 1-.04 1.4-.7 2.7-.7s1.6.7 2.7.66c1.1-.02 1.9-1 2.6-2.1.5-.8.7-1.2 1.1-2-2-.9-2-3.4-2-3.4Zm-2-6.3c.6-.7 1-1.7.9-2.7-.9.04-1.9.6-2.5 1.3-.6.6-1.1 1.7-.9 2.6 1 .1 1.9-.5 2.5-1.2Z"/>',
    linux: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="m7 9 3 3-3 3"/><path d="M13 15h4"/>',
    filter: '<path d="M3 5h18l-7 8v6l-4 2v-8Z"/>',
    shield: '<path d="M12 2 4 5v6c0 5 3.4 8.5 8 11 4.6-2.5 8-6 8-11V5Z"/>',
    cookie2: '<circle cx="12" cy="12" r="9"/>'
  };

  function icon(name) {
    var inner = ICONS[name] || ICONS.globe;
    return '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' + inner + '</svg>';
  }
  function iconNode(name, cls) { return h('span', { class: 'ico' + (cls ? ' ' + cls : ''), html: icon(name) }); }

  function hydrateIcons(root) {
    var nodes = (root || document).querySelectorAll('[data-icon]');
    for (var i = 0; i < nodes.length; i++) nodes[i].innerHTML = icon(nodes[i].getAttribute('data-icon'));
  }

  /* ── LocalStorage helpers ───────────────────────────────────────────── */

  function lsGet(k) { try { return window.localStorage.getItem(k); } catch (e) { return null; } }
  function lsSet(k, v) { try { window.localStorage.setItem(k, v); } catch (e) { /* storage unavailable */ } }

  /* ── State ──────────────────────────────────────────────────────────── */

  var state = {
    route: 'profiles',
    api: lsGet(LS.api) || DEFAULT_API,
    token: lsGet(LS.token) || '',
    mock: false,
    loading: true,
    loadError: null,
    profiles: [],
    full: {},
    running: {},
    startedAt: {},
    selected: {},
    query: '',
    tagFilter: {},
    health: null,
    pollTimer: null
  };

  function detectMock() {
    var forced = /[?&]mock=1\b/.test(window.location.search);
    if (forced) return true;
    return lsGet(LS.mock) === '1';
  }

  function normalizeApi(url) { return String(url || '').replace(/\/+$/, ''); }

  /* ── API client ─────────────────────────────────────────────────────── */

  function ApiError(status, code, message) { this.name = 'ApiError'; this.status = status; this.code = code; this.message = message; }
  ApiError.prototype = Object.create(Error.prototype);

  function api(method, path, body) {
    if (state.mock && window.mirageMock) {
      return window.mirageMock.request(method, path, body === undefined ? null : body).then(function (res) {
        return handleResponse(res.status, res.body);
      });
    }
    var url = normalizeApi(state.api) + path;
    var headers = {};
    if (body !== undefined && body !== null) headers['content-type'] = 'application/json';
    if (state.token) headers['authorization'] = 'Bearer ' + state.token;
    return window.fetch(url, {
      method: method,
      headers: headers,
      body: (body === undefined || body === null) ? undefined : JSON.stringify(body)
    }).then(function (res) {
      return res.text().then(function (txt) {
        var data = null;
        if (txt) { try { data = JSON.parse(txt); } catch (e) { data = txt; } }
        return handleResponse(res.status, data);
      });
    }, function (err) {
      throw new ApiError(0, 'network', 'Нет связи с API ' + normalizeApi(state.api) + ' — ' + (err && err.message ? err.message : String(err)));
    });
  }

  function handleResponse(status, data) {
    if (status >= 200 && status < 300) return data;
    var err = (data && data.error) ? data.error : null;
    var message = err && err.message ? err.message
      : (typeof data === 'string' && data ? data : ('HTTP ' + status));
    var code = err && err.code ? err.code : ('http_' + status);
    throw new ApiError(status, code, message);
  }

  // api() with automatic error toast. Callers may still .catch to reset spinners.
  function apiT(method, path, body, title) {
    return api(method, path, body).catch(function (e) {
      toast('error', title || 'Ошибка API', e.message + (e.code && e.status ? ' [' + e.code + ']' : ''));
      throw e;
    });
  }

  /* ── Toasts ─────────────────────────────────────────────────────────── */

  function toast(type, title, msg) {
    var host = $('#toasts');
    if (!host) return;
    var icoName = type === 'error' ? 'warning' : (type === 'ok' ? 'check' : (type === 'warn' ? 'warning' : 'info'));
    var node = h('div', { class: 'toast toast--' + type },
      iconNode(icoName),
      h('div', { class: 'toast__body' },
        h('div', { class: 'toast__title', text: title }),
        msg ? h('div', { class: 'toast__msg', text: msg }) : null),
      h('button', { class: 'toast__x', type: 'button', 'aria-label': 'Закрыть', onclick: function () { remove(); } }, iconNode('x'))
    );
    var removed = false;
    function remove() {
      if (removed) return; removed = true;
      node.classList.add('is-out');
      window.setTimeout(function () { if (node.parentNode) node.parentNode.removeChild(node); }, 200);
    }
    host.appendChild(node);
    window.setTimeout(remove, type === 'error' ? 6500 : 4200);
  }

  /* ── Formatting helpers ─────────────────────────────────────────────── */

  function fmtDate(iso) {
    if (!iso) return '—';
    var d = new Date(iso);
    if (isNaN(d.getTime())) return String(iso);
    return d.toLocaleString('ru-RU', { day: '2-digit', month: '2-digit', year: 'numeric', hour: '2-digit', minute: '2-digit' });
  }
  function relTime(iso) {
    if (!iso) return null;
    var d = new Date(iso); if (isNaN(d.getTime())) return null;
    var s = Math.max(0, Math.floor((Date.now() - d.getTime()) / 1000));
    if (s < 60) return 'только что';
    var m = Math.floor(s / 60); if (m < 60) return m + ' мин назад';
    var hr = Math.floor(m / 60); if (hr < 24) return hr + ' ч назад';
    var dd = Math.floor(hr / 24); return dd + ' дн назад';
  }
  function osIcon(os) { return (OS_META[os] && OS_META[os].icon) || 'shield'; }

  /* ── Modals ─────────────────────────────────────────────────────────── */

  function showModal(cfg) {
    var root = $('#modal-root');
    var backdrop = h('div', { class: 'modal-backdrop' });
    var modal = h('div', { class: 'modal' + (cfg.wide ? ' modal--wide' : '') });
    var titleEl = h('h2', { text: cfg.title });
    var closeBtn = h('button', { class: 'modal__close', type: 'button', 'aria-label': 'Закрыть' }, iconNode('x'));
    var head = h('div', { class: 'modal__head' }, titleEl, closeBtn);
    var body = h('div', { class: 'modal__body' });
    appendKid(body, cfg.body);
    var foot = h('div', { class: 'modal__foot' });
    if (cfg.footer) appendKid(foot, cfg.footer);
    modal.appendChild(head); modal.appendChild(body); if (cfg.footer) modal.appendChild(foot);
    backdrop.appendChild(modal);
    root.appendChild(backdrop);

    var closed = false;
    function close() {
      if (closed) return; closed = true;
      backdrop.classList.remove('is-open');
      if (backdrop.parentNode) backdrop.parentNode.removeChild(backdrop);
      document.removeEventListener('keydown', onKey);
      if (cfg.onClose) cfg.onClose();
    }
    function onKey(e) { if (e.key === 'Escape') close(); }
    document.addEventListener('keydown', onKey);
    backdrop.addEventListener('mousedown', function (e) { if (e.target === backdrop) close(); });
    closeBtn.addEventListener('click', close);

    window.setTimeout(function () {
      var f = modal.querySelector('input, select, textarea');
      if (f) f.focus();
    }, 30);

    return { close: close, modal: modal, body: body, foot: foot, titleEl: titleEl };
  }

  function confirmDialog(title, message, confirmLabel, danger) {
    return new Promise(function (resolve) {
      var done = false;
      var ok = h('button', { class: 'btn ' + (danger ? 'btn--danger' : 'btn--primary'), type: 'button', text: confirmLabel || 'Подтвердить' });
      var cancel = h('button', { class: 'btn', type: 'button', text: 'Отмена' });
      var m = showModal({
        title: title,
        body: h('p', { class: 'muted', text: message, style: 'margin:0' }),
        footer: [h('div', { class: 'spacer' }), cancel, ok],
        onClose: function () { if (!done) { done = true; resolve(false); } }
      });
      cancel.addEventListener('click', function () { done = true; m.close(); resolve(false); });
      ok.addEventListener('click', function () { done = true; m.close(); resolve(true); });
    });
  }

  /* ── Health ─────────────────────────────────────────────────────────── */

  function setHealthUI(status, detail) {
    var dot = $('#health-dot'), text = $('#health-text');
    if (!dot || !text) return;
    if (status === 'ok') {
      dot.setAttribute('data-state', detail.vault === 'unlocked' ? 'unlocked' : 'locked');
      dot.classList.toggle('dot--pulse', detail.vault === 'unlocked');
      text.textContent = 'v' + (detail.version || '?') + ' · ' + (detail.vault === 'unlocked' ? 'разблокирован' : 'заблокирован');
      $('#sidebar-hint').textContent = state.mock ? 'Мок-режим: сервер не требуется' : ('API: ' + normalizeApi(state.api));
    } else if (status === 'error') {
      dot.setAttribute('data-state', 'error'); dot.classList.remove('dot--pulse');
      text.textContent = 'Нет связи';
      $('#sidebar-hint').textContent = state.mock ? 'Мок-режим' : ('Не отвечает: ' + normalizeApi(state.api));
    } else {
      dot.setAttribute('data-state', 'unknown'); dot.classList.remove('dot--pulse');
      text.textContent = 'Проверка…';
    }
  }

  function refreshHealth(silent) {
    setHealthUI('checking');
    return api('GET', '/api/v1/health').then(function (hlt) {
      state.health = hlt;
      setHealthUI('ok', hlt);
      return hlt;
    }).catch(function (e) {
      state.health = null;
      setHealthUI('error');
      if (!silent) toast('error', 'Сервер недоступен', e.message);
    });
  }

  /* ── Profiles data ──────────────────────────────────────────────────── */

  function loadProfiles() {
    state.loading = true;
    state.loadError = null;
    render();
    return api('GET', '/api/v1/profiles').then(function (d) {
      state.profiles = (d && d.profiles) || [];
      pruneFull();
      return hydrateFull();
    }).then(function () {
      return refreshStatus(true);
    }).catch(function (e) {
      state.loadError = e.message;
      toast('error', 'Не удалось загрузить профили', e.message);
    }).then(function () {
      state.loading = false;
      render();
    });
  }

  // Full profiles carry the fingerprint (and OS) the summary lacks.
  function pruneFull() {
    var keep = {};
    state.profiles.forEach(function (p) { keep[p.id] = true; });
    for (var id in state.full) if (!keep[id]) delete state.full[id];
  }

  function hydrateFull() {
    var missing = state.profiles.map(function (p) { return p.id; }).filter(function (id) { return !state.full[id]; });
    if (!missing.length) return Promise.resolve();
    var CHUNK = 8;
    var chain = Promise.resolve();
    var run = function (batch) {
      return Promise.all(batch.map(function (id) {
        return api('GET', '/api/v1/profiles/' + encodeURIComponent(id)).then(function (p) { state.full[id] = p; }, function () { /* summary-only fallback */ });
      }));
    };
    for (var i = 0; i < missing.length; i += CHUNK) {
      chain = chain.then(run.bind(null, missing.slice(i, i + CHUNK)));
    }
    return chain;
  }

  function refreshStatus(silent) {
    return api('GET', '/api/v1/status').then(function (d) {
      var run = {};
      var started = {};
      ((d && d.running) || []).forEach(function (r) { run[r.profile_id] = true; started[r.profile_id] = r.started_at; });
      state.running = run;
      for (var id in started) if (started[id]) state.startedAt[id] = started[id];
      applyStatus();
    }).catch(function (e) {
      if (!silent) toast('warn', 'Статус недоступен', e.message);
    });
  }

  function startPolling() {
    if (state.pollTimer) window.clearInterval(state.pollTimer);
    state.pollTimer = window.setInterval(function () {
      if (state.mock || state.health) refreshStatus(true);
      else refreshHealth(true);
    }, 2000);
  }

  /* ── Rendering ──────────────────────────────────────────────────────── */

  function render() {
    var navItems = document.querySelectorAll('.nav__item');
    for (var i = 0; i < navItems.length; i++) {
      navItems[i].classList.toggle('is-active', navItems[i].getAttribute('data-route') === state.route);
    }
    var meta = ROUTES[state.route] || ROUTES.profiles;
    $('#view-title').textContent = meta.title;
    $('#view-sub').textContent = meta.sub;
    var badge = $('#mock-badge');
    if (badge) badge.hidden = !state.mock;
    $('#health-refresh').title = state.mock ? 'Мок-режим активен' : 'Обновить статус сервера';

    var view = $('#view');
    clear(view);
    if (state.route === 'profiles') renderProfiles(view);
    else if (state.route === 'cookies') renderCookies(view);
    else if (state.route === 'import') renderImport(view);
    else renderSettings(view);
  }

  function go(route) { state.route = route; render(); }

  /* ── Profiles view ──────────────────────────────────────────────────── */

  var profileRefs = { grid: null, count: null, bulk: null };

  function renderProfiles(view) {
    var toolbar = h('div', { class: 'toolbar' });

    var search = h('div', { class: 'search' }, iconNode('search'),
      h('input', {
        type: 'text', placeholder: 'Поиск по имени…', value: state.query,
        oninput: function (e) { state.query = e.target.value; renderGrid(); }
      }));

    var allTags = collectTags();
    var tagFilters = h('div', { class: 'toolbar__tags', style: 'display:flex;gap:6px;flex-wrap:wrap' });
    allTags.forEach(function (t) {
      var active = !!state.tagFilter[t];
      var chip = h('button', {
        class: 'chip', type: 'button', style: 'cursor:pointer' + (active ? ';border-color:var(--accent-line);background:var(--accent-soft);color:var(--text)' : ''),
        onclick: function () { if (state.tagFilter[t]) delete state.tagFilter[t]; else state.tagFilter[t] = true; render(); }
      }, '#' + t);
      tagFilters.appendChild(chip);
    });
    if (allTags.length) toolbar.appendChild(tagFilters);

    toolbar.appendChild(h('div', { class: 'toolbar__spacer' }));

    profileRefs.count = h('span', { class: 'count-pill' });
    toolbar.appendChild(profileRefs.count);

    var refreshBtn = h('button', { class: 'btn btn--icon', type: 'button', title: 'Обновить список', onclick: function () { loadProfiles(); } }, iconNode('refresh'));
    toolbar.appendChild(refreshBtn);

    var newBtn = h('button', { class: 'btn btn--primary', type: 'button', onclick: function () { openProfileEditor(null); } }, iconNode('plus'), 'Новый профиль');
    toolbar.appendChild(newBtn);

    view.appendChild(search);
    view.appendChild(toolbar);

    profileRefs.bulk = h('div', { class: 'toolbar' });
    view.appendChild(profileRefs.bulk);

    // Connection hint when the real API is not reachable and mock is off.
    if (!state.mock && state.loadError) {
      var retryBtn = h('button', { class: 'btn btn--sm', type: 'button', onclick: function () { loadProfiles(); } }, iconNode('refresh'), 'Повторить');
      var mockBtn = h('button', { class: 'btn btn--sm', type: 'button', style: 'margin-left:8px', onclick: function () { enableMock(true); } }, iconNode('shield'), 'Включить мок-режим');
      var hintBody = h('div', {},
        h('div', { text: 'Нет связи с локальным API.' }),
        h('div', { class: 'muted', text: state.loadError }),
        h('div', { style: 'margin-top:8px' }, retryBtn, mockBtn));
      view.appendChild(h('div', { class: 'banner banner--warn', style: 'margin-bottom:16px' }, iconNode('warning'), hintBody));
    }

    profileRefs.grid = h('div', { class: 'grid' });
    view.appendChild(profileRefs.grid);

    renderBulk();
    renderGrid();
  }

  function collectTags() {
    var set = {};
    state.profiles.forEach(function (p) { (p.tags || []).forEach(function (t) { set[t] = true; }); });
    return Object.keys(set).sort();
  }

  function filteredProfiles() {
    var q = state.query.trim().toLowerCase();
    var tags = Object.keys(state.tagFilter);
    return state.profiles.filter(function (p) {
      if (q && String(p.name || '').toLowerCase().indexOf(q) === -1) return false;
      if (tags.length) {
        var pt = p.tags || [];
        var hit = tags.some(function (t) { return pt.indexOf(t) !== -1; });
        if (!hit) return false;
      }
      return true;
    });
  }

  function renderGrid() {
    var grid = profileRefs.grid;
    if (!grid) return;
    clear(grid);
    if (profileRefs.count) {
      var runCount = state.profiles.filter(function (p) { return state.running[p.id]; }).length;
      profileRefs.count.textContent = state.profiles.length + ' профилей · ' + runCount + ' запущено';
    }
    if (state.loading) {
      for (var i = 0; i < 6; i++) grid.appendChild(h('div', { class: 'sk-card skeleton' }));
      return;
    }
    var list = filteredProfiles();
    if (!list.length) {
      grid.appendChild(emptyState());
      return;
    }
    list.forEach(function (p) { grid.appendChild(profileCard(p)); });
    applyStatus();
  }

  function emptyState() {
    var hasProfiles = state.profiles.length > 0;
    return h('div', { class: 'empty', style: 'grid-column:1/-1' },
      iconNode('layers', 'ico'),
      h('h3', { text: hasProfiles ? 'Ничего не найдено' : 'Пока нет профилей' }),
      h('p', { text: hasProfiles ? 'Измените поиск или сбросьте фильтры.' : 'Создайте профиль или импортируйте из AdsPower, Dolphin, GoLogin.' }),
      h('div', { style: 'display:flex;gap:8px;margin-top:8px' },
        h('button', { class: 'btn btn--primary', type: 'button', onclick: function () { openProfileEditor(null); } }, iconNode('plus'), 'Новый профиль'),
        h('button', { class: 'btn', type: 'button', onclick: function () { go('import'); } }, iconNode('download'), 'Импорт')));
  }

  function profileCard(p) {
    var full = state.full[p.id];
    var os = full && full.fingerprint ? full.fingerprint.os : null;
    var running = !!state.running[p.id];

    var checkbox = h('input', { class: 'checkbox', type: 'checkbox', 'aria-label': 'Выбрать профиль', checked: !!state.selected[p.id] });
    checkbox.addEventListener('change', function () {
      if (checkbox.checked) state.selected[p.id] = true; else delete state.selected[p.id];
      renderBulk(); renderGrid();
    });

    var statusDot = h('span', { class: 'dot', dataset: { state: running ? 'running' : 'stopped' } });
    var statusText = h('span', { class: 't', text: running ? 'running' : 'stopped' });
    var statusLine = h('span', { class: 'card__status', dataset: { role: 'status' } }, statusDot, statusText);

    var head = h('div', { class: 'card__head' },
      h('label', { class: 'checkbox__wrap' }, checkbox),
      h('div', { class: 'card__os', html: h('span', { html: icon(osIcon(os)) }).innerHTML }),
      h('div', { class: 'card__id' },
        h('div', { class: 'card__name', text: p.name }),
        h('div', { class: 'card__sub' }, statusLine)));

    var tags = h('div', { class: 'card__tags' });
    (p.tags || []).forEach(function (t) { tags.appendChild(h('span', { class: 'chip', text: '#' + t })); });

    var proxyChip;
    if (p.proxy) {
      // The contract's ProxySummary exposes scheme/host/port only (no country),
      // so the chip is just the globe plus host:port in both mock and live.
      proxyChip = h('span', { class: 'card__proxy' }, iconNode('globe'),
        h('code', { text: p.proxy.scheme + '://' + p.proxy.host + ':' + p.proxy.port }));
    } else {
      proxyChip = h('span', { class: 'card__proxy' }, iconNode('globe'), h('span', { text: 'Прямое соединение' }));
    }

    var launch = h('span', { dataset: { role: 'launch' } });
    var meta = h('div', { class: 'card__meta' },
      proxyChip,
      h('span', { class: 'nowrap' }, h('b', { text: 'Запуск: ' }), launch));

    var osLabel = os ? (OS_META[os] ? OS_META[os].label : os) : (running ? '—' : '—');
    var meta2 = h('div', { class: 'card__meta' },
      h('span', { class: 'nowrap' }, h('b', { text: 'ПО: ' }), osLabel),
      h('span', { class: 'nowrap' }, h('b', { text: 'Создан: ' }), fmtDate(p.createdAt)));

    var startBtn = h('button', { class: 'btn btn--sm', type: 'button', dataset: { role: 'start' }, title: 'Запустить браузер' }, iconNode('play'), 'Start');
    var stopBtn = h('button', { class: 'btn btn--sm', type: 'button', dataset: { role: 'stop' }, title: 'Остановить браузер' }, iconNode('stop'), 'Stop');
    var editBtn = h('button', { class: 'btn btn--sm btn--icon', type: 'button', title: 'Редактировать', onclick: function () { openProfileEditor(p.id); } }, iconNode('edit'));
    var dupBtn = h('button', { class: 'btn btn--sm btn--icon', type: 'button', title: 'Дублировать', onclick: function () { duplicateProfile(p); } }, iconNode('copy'));
    var delBtn = h('button', { class: 'btn btn--sm btn--icon btn--danger', type: 'button', title: 'Удалить', onclick: function () { deleteProfile(p); } }, iconNode('trash'));

    startBtn.addEventListener('click', function () { launchProfile(p); });
    stopBtn.addEventListener('click', function () { stopProfile(p); });

    var actions = h('div', { class: 'card__actions' }, startBtn, stopBtn, editBtn, dupBtn, delBtn);

    var card = h('article', { class: 'card' + (running ? ' is-running' : '') + (state.selected[p.id] ? ' is-selected' : ''), dataset: { id: p.id } }, head, tags, meta, meta2, actions);
    if (state.mock) card.appendChild(h('div', { class: 'mocknote', title: 'Запуск браузера требует реального сервера' }, iconNode('lock'), h('span', { text: 'Start/Stop — только с реальным сервером' })));
    return card;
  }

  function applyStatus() {
    var cards = document.querySelectorAll('.card[data-id]');
    for (var i = 0; i < cards.length; i++) {
      var card = cards[i];
      var id = card.getAttribute('data-id');
      var running = !!state.running[id];
      card.classList.toggle('is-running', running);
      var st = card.querySelector('[data-role="status"]');
      if (st) {
        st.querySelector('.dot').setAttribute('data-state', running ? 'running' : 'stopped');
        st.querySelector('.t').textContent = running ? 'running' : 'stopped';
      }
      var startBtn = card.querySelector('[data-role="start"]');
      var stopBtn = card.querySelector('[data-role="stop"]');
      if (startBtn) startBtn.disabled = running;
      if (stopBtn) stopBtn.disabled = !running;
      var launch = card.querySelector('[data-role="launch"]');
      if (launch) {
        var at = state.startedAt[id];
        launch.textContent = running ? ('сейчас · ' + relTime(at)) : (at ? (fmtDate(at) + ' (' + relTime(at) + ')') : '—');
      }
    }
  }

  function renderBulk() {
    var bar = profileRefs.bulk;
    if (!bar) return;
    clear(bar);
    var ids = Object.keys(state.selected);
    if (!ids.length) return;
    bar.appendChild(h('span', { class: 'count-pill', text: 'Выбрано: ' + ids.length }));
    var startAll = h('button', { class: 'btn btn--sm', type: 'button' }, iconNode('play'), 'Запустить');
    var stopAll = h('button', { class: 'btn btn--sm', type: 'button' }, iconNode('stop'), 'Остановить');
    var delAll = h('button', { class: 'btn btn--sm btn--danger', type: 'button' }, iconNode('trash'), 'Удалить');
    var clearSel = h('button', { class: 'btn btn--sm btn--ghost', type: 'button', onclick: function () { state.selected = {}; renderBulk(); renderGrid(); } }, iconNode('x'), 'Снять выделение');
    startAll.addEventListener('click', function () { bulkStart(ids); });
    stopAll.addEventListener('click', function () { bulkStop(ids); });
    delAll.addEventListener('click', function () { bulkDelete(ids); });
    bar.appendChild(startAll); bar.appendChild(stopAll); bar.appendChild(delAll); bar.appendChild(clearSel);
    if (state.mock) bar.appendChild(h('span', { class: 'lockhint' }, iconNode('lock'), 'Start/Stop — только с реальным сервером'));
  }

  /* ── Launch / stop ──────────────────────────────────────────────────── */

  function launchProfile(p) {
    if (state.mock) { toast('info', 'Запуск браузера недоступен в мок-режиме', 'Требуется реальный сервер Mirage. Профиль «' + p.name + '» не запущен.'); return; }
    apiT('POST', '/api/v1/profiles/' + encodeURIComponent(p.id) + '/launch', null, 'Не удалось запустить профиль')
      .then(function (r) {
        toast('ok', 'Браузер запущен', p.name + ' · ws ' + (r && r.ws_url ? r.ws_url : ''));
        return refreshStatus(true);
      }).catch(function () { /* toast уже показан */ });
  }

  function stopProfile(p) {
    if (state.mock) { toast('info', 'Остановка недоступна в мок-режиме', 'Требуется реальный сервер Mirage. Профиль «' + p.name + '» не остановлен.'); return; }
    apiT('POST', '/api/v1/profiles/' + encodeURIComponent(p.id) + '/stop', null, 'Не удалось остановить профиль')
      .then(function () { toast('ok', 'Браузер остановлен', p.name); return refreshStatus(true); })
      .catch(function () {});
  }

  function bulkStart(ids) {
    if (state.mock) { toast('info', 'Массовый запуск недоступен в мок-режиме', 'Требуется реальный сервер Mirage.'); return; }
    runBulk(ids, 'launch', 'Запущено профилей');
  }
  function bulkStop(ids) {
    if (state.mock) { toast('info', 'Массовая остановка недоступна в мок-режиме', 'Требуется реальный сервер Mirage.'); return; }
    runBulk(ids, 'stop', 'Остановлено профилей');
  }
  function runBulk(ids, action, label) {
    var done = 0, failed = 0;
    Promise.all(ids.map(function (id) {
      return api('POST', '/api/v1/profiles/' + encodeURIComponent(id) + '/' + action, null).then(function () { done++; }, function () { failed++; });
    })).then(function () {
      if (done) toast('ok', label, String(done) + (failed ? (' · ошибок: ' + failed) : ''));
      else if (failed) toast('error', 'Операция не выполнена', 'Ошибок: ' + failed);
      return refreshStatus(true);
    });
  }

  /* ── Delete / duplicate ─────────────────────────────────────────────── */

  function deleteProfile(p) {
    confirmDialog('Удалить профиль?', 'Профиль «' + p.name + '» будет удалён без возможности восстановления.', 'Удалить', true).then(function (ok) {
      if (!ok) return;
      apiT('DELETE', '/api/v1/profiles/' + encodeURIComponent(p.id), null, 'Не удалось удалить профиль').then(function () {
        delete state.selected[p.id]; delete state.full[p.id];
        state.profiles = state.profiles.filter(function (x) { return x.id !== p.id; });
        toast('ok', 'Профиль удалён', p.name);
        renderGrid(); renderBulk();
      }).catch(function () {});
    });
  }

  function bulkDelete(ids) {
    confirmDialog('Удалить ' + ids.length + ' профилей?', 'Выбранные профили будут удалены без возможности восстановления.', 'Удалить', true).then(function (ok) {
      if (!ok) return;
      Promise.all(ids.map(function (id) {
        return api('DELETE', '/api/v1/profiles/' + encodeURIComponent(id)).then(function () { return true; }, function () { return false; });
      })).then(function (res) {
        var okN = res.filter(Boolean).length, errN = res.length - okN;
        ids.forEach(function (id) { delete state.selected[id]; delete state.full[id]; });
        state.profiles = state.profiles.filter(function (p) { return ids.indexOf(p.id) === -1; });
        if (okN) toast('ok', 'Удалено профилей', String(okN) + (errN ? (' · ошибок: ' + errN) : ''));
        if (errN && !okN) toast('error', 'Удаление не выполнено', 'Ошибок: ' + errN);
        renderGrid(); renderBulk();
      });
    });
  }

  function duplicateProfile(p) {
    var full = state.full[p.id];
    if (!full) { toast('warn', 'Нет данных профиля', 'Дождитесь загрузки и попробуйте снова.'); return; }
    var body = {
      name: p.name + ' (copy)',
      tags: (p.tags || []).slice(),
      os: full.fingerprint ? full.fingerprint.os : 'windows',
      proxy: p.proxy ? { scheme: p.proxy.scheme, host: p.proxy.host, port: p.proxy.port, username: full.proxy ? full.proxy.username : null, password: full.proxy ? full.proxy.password : null } : null,
      // Pass the whole fingerprint as overrides so the clone is bit-for-bit identical.
      overrides: full.fingerprint || undefined
    };
    apiT('POST', '/api/v1/profiles', body, 'Не удалось дублировать профиль').then(function () {
      toast('ok', 'Профиль дублирован', body.name);
      return loadProfiles();
    }).catch(function () {});
  }

  /* ── Profile editor modal ───────────────────────────────────────────── */

  function openProfileEditor(id) {
    var isEdit = !!id;
    var full = isEdit ? state.full[id] : null;
    if (isEdit && !full) {
      apiT('GET', '/api/v1/profiles/' + encodeURIComponent(id), null, 'Не удалось открыть профиль').then(function (p) {
        state.full[id] = p; openProfileEditor(id);
      }).catch(function () {});
      return;
    }
    var fp = full ? full.fingerprint : null;
    var isProxy = full ? full.proxy : null;

    // ── form state
    var nameInput = h('input', { type: 'text', value: (full && full.name) || '', placeholder: 'Например, Shop-01' });
    var tagsInput = arrayInput((full && full.tags) || [], 'тег + Enter');
    var osValue = fp ? fp.os : 'windows';
    // Seed is editable both when creating and when editing. Editing it and
    // saving sends `seed` in the PUT, which makes the API rebuild the whole
    // fingerprint from that seed (overrides then layer on top).
    var seedInput = h('input', { type: 'number', placeholder: 'авто' });
    seedInput.value = (full && typeof full.seed === 'number' && full.seed > 0) ? String(full.seed) : '';

    // proxy
    var proxy = {
      scheme: isProxy ? isProxy.scheme : 'socks5',
      host: isProxy ? isProxy.host : '',
      port: isProxy ? String(isProxy.port) : '',
      username: isProxy ? (isProxy.username || '') : '',
      password: isProxy ? (isProxy.password || '') : '',
      enabled: !!isProxy
    };
    var proxyEnable = h('input', { type: 'checkbox', checked: proxy.enabled });

    // fingerprint overrides
    var ov = {
      osVersion: fp ? fp.osVersion : '',
      timezone: fp ? fp.timezone : '',
      languages: fp ? fp.languages.slice() : [],
      languagesTouched: !!fp,
      screen: fp ? screenKey(fp.screen) : '',
      webRTC: fp ? fp.webRTC : '',
      canvasNoise: false,
      canvasSeed: (fp && typeof fp.canvasSeed === 'number') ? fp.canvasSeed : 0,
      platformVersion: (fp && fp.clientHints) ? fp.clientHints.platformVersion : ''
    };

    // ── build controls
    // The OS is chosen only at creation. The frozen PUT contract has no `os`
    // field and rebuilds the fingerprint from the *stored* OS, so an editable OS
    // control in edit mode is a silent no-op that additionally desyncs the
    // dependent selects (osVersion/screen) below. Show it read-only when editing.
    var osControl;
    if (isEdit) {
      osControl = h('div', { class: 'field' },
        h('label', { text: 'Операционная система' }),
        h('div', { class: 'muted', style: 'display:flex;align-items:center;gap:8px;padding:6px 0' },
          iconNode(osIcon(osValue)),
          h('span', { text: (OS_META[osValue] ? OS_META[osValue].label : osValue) + ' · задана при создании' })),
        h('div', { class: 'field__hint', text: 'ОС фиксируется при создании профиля и не меняется через PUT. Для другой ОС создайте новый профиль.' }));
    } else {
      var osSeg = segmented([
        { v: 'windows', label: 'Windows', icon: 'windows' },
        { v: 'macos', label: 'macOS', icon: 'macos' },
        { v: 'linux', label: 'Linux', icon: 'linux' }
      ], osValue, function (v) { osValue = v; syncOsVersionOptions(); syncScreenOptions(); });
      osControl = h('div', { class: 'field' }, h('label', { text: 'Операционная система' }), osSeg);
    }

    var osVersionSelect = h('select');
    function syncOsVersionOptions() {
      clear(osVersionSelect);
      osVersionSelect.appendChild(h('option', { value: '', text: 'авто' }));
      OS_META[osValue].versions.forEach(function (v) { osVersionSelect.appendChild(h('option', { value: v, text: v })); });
      if (ov.osVersion && OS_META[osValue].versions.indexOf(ov.osVersion) !== -1) osVersionSelect.value = ov.osVersion;
      else { osVersionSelect.value = ''; ov.osVersion = ''; }
    }

    var tzInput = h('input', { type: 'text', list: 'tz-list', value: ov.timezone, placeholder: 'авто (например Europe/Paris)' });
    var tzList = h('datalist', { id: 'tz-list' });
    TIMEZONES.forEach(function (t) { tzList.appendChild(h('option', { value: t })); });

    var langInput = arrayInput(ov.languages, 'язык + Enter');

    var screenSelect = h('select');
    function syncScreenOptions() {
      clear(screenSelect);
      screenSelect.appendChild(h('option', { value: '', text: 'авто' }));
      SCREENS.filter(function (s) { return !s.os || s.os === osValue; }).forEach(function (s) {
        screenSelect.appendChild(h('option', { value: s.w + 'x' + s.h + '@' + s.dpr, text: s.w + '×' + s.h + ' @' + s.dpr + 'x' }));
      });
      screenSelect.value = ov.screen && screenSelect.querySelector('option[value="' + ov.screen + '"]') ? ov.screen : '';
    }

    var webrtcSeg = segmented(WEBRTC_MODES.map(function (m) { return { v: m.v, label: m.label }; }), ov.webRTC || 'real', function (v) { ov.webRTC = v; });
    // "auto" for webrtc: add a trailing clear — represent auto as '' and default to real
    var webrtcAutoBtn = h('button', { class: 'btn btn--sm btn--ghost', type: 'button', title: 'Сбросить в авто', onclick: function () { ov.webRTC = ''; webrtcSeg.set('real'); } }, 'авто');

    var canvasSwitch = h('input', { type: 'checkbox', checked: ov.canvasNoise });
    var canvasSeedInput = h('input', { type: 'number', value: String(ov.canvasSeed || 0), disabled: !ov.canvasNoise });
    canvasSwitch.addEventListener('change', function () { ov.canvasNoise = canvasSwitch.checked; canvasSeedInput.disabled = !ov.canvasNoise; });

    var platformInput = h('input', { type: 'text', value: ov.platformVersion, placeholder: 'авто' });

    // proxy check
    var pcheck = h('div', { class: 'pcheck' });
    var proxyCheckBtn = h('button', { class: 'btn', type: 'button' }, iconNode('shield'), 'Проверить');
    proxyCheckBtn.addEventListener('click', function () {
      var host = proxy.host.trim();
      if (!host) { writePcheck(pcheck, false, 'Укажите host прокси'); return; }
      clear(pcheck); pcheck.appendChild(h('span', { class: 'muted' }, 'Проверка…'));
      proxyCheckBtn.disabled = true;
      api('POST', '/api/v1/proxy/check', {
        scheme: proxy.scheme, host: host, port: Number(proxy.port) || 0,
        username: proxy.username || null, password: proxy.password || null
      }).then(function (r) {
        if (r && r.ok) writePcheck(pcheck, true, 'IP ' + r.ip + ' · ' + r.latency_ms + ' ms');
        else writePcheck(pcheck, false, (r && r.error) ? r.error : 'Проверка не удалась');
      }).catch(function (e) {
        writePcheck(pcheck, false, e.message);
      }).then(function () { proxyCheckBtn.disabled = false; });
    });

    function writePcheck(node, ok, text) {
      clear(node);
      node.className = 'pcheck ' + (ok ? 'pcheck--ok' : 'pcheck--err');
      node.appendChild(iconNode(ok ? 'check' : 'warning'));
      node.appendChild(h('span', { text: (ok ? 'OK — ' : 'Ошибка — ') + text }));
    }

    // proxy fields need to be bound to the `proxy` object
    var schemeSeg = segmented([{ v: 'socks5', label: 'socks5' }, { v: 'http', label: 'http' }], proxy.scheme, function (v) { proxy.scheme = v; });
    var hostInput = h('input', { type: 'text', value: proxy.host, placeholder: '1.2.3.4' });
    var portInput = h('input', { type: 'number', value: proxy.port, placeholder: '1080' });
    var userInput = h('input', { type: 'text', value: proxy.username, placeholder: 'логин' });
    var passInput = h('input', { type: 'password', value: proxy.password, placeholder: 'пароль' });
    hostInput.addEventListener('input', function () { proxy.host = hostInput.value; });
    portInput.addEventListener('input', function () { proxy.port = portInput.value; });
    userInput.addEventListener('input', function () { proxy.username = userInput.value; });
    passInput.addEventListener('input', function () { proxy.password = passInput.value; });

    var proxyFields = h('div', { class: 'stack' },
      h('div', { class: 'row row--tight' }, h('div', { class: 'field' }, h('label', { text: 'Схема' }), schemeSeg)),
      h('div', { class: 'row' },
        h('div', { class: 'field field--grow' }, h('label', { text: 'Host' }), hostInput),
        h('div', { class: 'field', style: 'max-width:130px' }, h('label', { text: 'Порт' }), portInput)),
      h('div', { class: 'row' },
        h('div', { class: 'field' }, h('label', { text: 'Логин' }), userInput),
        h('div', { class: 'field' }, h('label', { text: 'Пароль' }), passInput)),
      h('div', {}, proxyCheckBtn, pcheck));
    function syncProxyDisabled() {
      var on = proxyEnable.checked;
      [schemeSeg, hostInput, portInput, userInput, passInput, proxyCheckBtn].forEach(function (el) { el.disabled = !on; });
      proxyFields.style.opacity = on ? '1' : '.5';
    }
    proxyEnable.addEventListener('change', syncProxyDisabled);

    // error banner
    var errBanner = h('div', { class: 'banner banner--error hidden' }, iconNode('warning'), h('div', { class: 'banner__msg' }));

    var body = h('div', { class: 'stack' },
      errBanner,
      h('div', { class: 'field' }, h('label', { text: 'Имя' }), nameInput, h('div', { class: 'field__err', dataset: { err: 'name' } })),
      h('div', { class: 'field' }, h('label', { text: 'Теги' }), tagsInput.node),
      osControl,
      h('div', { class: 'row' },
        h('div', { class: 'field field--grow' }, h('label', { text: 'Seed' }), seedInput,
          h('div', { class: 'field__hint', text: isEdit ? 'Число или «авто». Новый seed пересобирает фингерпринт целиком.' : 'Число или «авто» — одинаковый seed даёт одинаковый фингерпринт.' }))),
      h('div', { class: 'section-title', text: 'Прокси' }),
      h('label', { class: 'switch' }, proxyEnable, h('span', { class: 'switch__track' }), h('span', { class: 'switch__label', text: 'Использовать прокси' })),
      proxyFields,
      h('div', { class: 'section-title', text: 'Фингерпринт' }),
      h('div', { class: 'grid-2' },
        h('div', { class: 'field' }, h('label', { text: 'Версия ОС' }), osVersionSelect),
        h('div', { class: 'field' }, h('label', { text: 'Timezone' }), tzInput, tzList)),
      h('div', { class: 'field' }, h('label', { text: 'Языки (languages)' }), langInput.node),
      h('div', { class: 'grid-2' },
        h('div', { class: 'field' }, h('label', { text: 'Экран (W×H @ dpr)' }), screenSelect),
        h('div', { class: 'field' }, h('label', { text: 'WebRTC' }), h('div', { style: 'display:flex;gap:8px;align-items:center' }, webrtcSeg, webrtcAutoBtn))),
      h('div', { class: 'grid-2' },
        h('div', { class: 'field' }, h('label', { text: 'Client Hints platformVersion' }), platformInput),
        h('div', { class: 'field' }, h('label', { text: 'Canvas' }), h('div', { style: 'display:flex;gap:10px;align-items:center' },
          h('label', { class: 'switch' }, canvasSwitch, h('span', { class: 'switch__track' }), h('span', { class: 'switch__label', text: 'Шум (canvasSeed)' })),
          h('div', { style: 'flex:1' }, canvasSeedInput)))));

    // ── init dependent selects
    syncOsVersionOptions();
    syncScreenOptions();
    syncProxyDisabled();
    osVersionSelect.addEventListener('change', function () { ov.osVersion = osVersionSelect.value; });
    screenSelect.addEventListener('change', function () { ov.screen = screenSelect.value; });
    tzInput.addEventListener('input', function () { ov.timezone = tzInput.value; });
    platformInput.addEventListener('input', function () { ov.platformVersion = platformInput.value; });
    canvasSeedInput.addEventListener('input', function () { ov.canvasSeed = Number(canvasSeedInput.value) || 0; });

    // ── footer
    var saveBtn = h('button', { class: 'btn btn--primary', type: 'button', text: isEdit ? 'Сохранить' : 'Создать' });
    var cancelBtn = h('button', { class: 'btn', type: 'button', text: 'Отмена' });
    var modal = showModal({
      title: isEdit ? 'Редактирование профиля' : 'Новый профиль',
      body: body,
      footer: [h('div', { class: 'spacer' }), cancelBtn, saveBtn],
      wide: true
    });
    cancelBtn.addEventListener('click', modal.close);

    saveBtn.addEventListener('click', function () {
      hideBanner(errBanner);
      clearFieldErrors(body);
      var name = nameInput.value.trim();
      var tags = tagsInput.getValues();
      var langs = langInput.getValues();

      var ok = true;
      if (!name) { setFieldError(body, 'name', 'Укажите имя профиля'); ok = false; }
      var seed = null;
      if (seedInput.value.trim()) {
        seed = Number(seedInput.value);
        if (!isFinite(seed) || Math.floor(seed) !== seed || seed < 0) { setFieldError(body, 'name', 'Seed должен быть целым неотрицательным числом'); ok = false; }
      }
      if (proxyEnable.checked) {
        if (!proxy.host.trim()) { toast('error', 'Проверьте прокси', 'Укажите host прокси'); ok = false; }
        else if (proxy.port && (!isFinite(Number(proxy.port)) || Number(proxy.port) < 1 || Number(proxy.port) > 65535)) { toast('error', 'Проверьте прокси', 'Порт должен быть в диапазоне 1..65535'); ok = false; }
      }
      if (!ok) return;

      var proxyBody = proxyEnable.checked ? {
        scheme: proxy.scheme, host: proxy.host.trim(), port: Number(proxy.port) || 0,
        username: proxy.username || null, password: proxy.password || null
      } : null;

      var overrides = {};
      if (ov.osVersion) overrides.osVersion = ov.osVersion;
      if (ov.timezone && ov.timezone.trim()) overrides.timezone = ov.timezone.trim();
      if (langs.length) overrides.languages = langs;
      if (ov.screen) { var s = parseScreenKey(ov.screen); if (s) overrides.screen = { width: s.w, height: s.h, availWidth: s.w, availHeight: s.h - OS_META[osValue].availOffset, dpr: s.dpr, colorDepth: 24 }; }
      if (ov.webRTC) overrides.webRTC = ov.webRTC;
      if (ov.canvasNoise) overrides.canvasSeed = (Number(ov.canvasSeed) || 0) >>> 0;
      if (ov.platformVersion && ov.platformVersion.trim()) overrides.clientHints = { platformVersion: ov.platformVersion.trim() };

      var req;
      if (isEdit) {
        req = { name: name, tags: tags, proxy: proxyBody, overrides: overrides };
        if (seed !== null) req.seed = seed;
      } else {
        req = { name: name, tags: tags, os: osValue, proxy: proxyBody };
        if (seed !== null) req.seed = seed;
        if (Object.keys(overrides).length) req.overrides = overrides;
      }

      saveBtn.disabled = true;
      cancelBtn.disabled = true;
      var path = isEdit ? ('/api/v1/profiles/' + encodeURIComponent(id)) : '/api/v1/profiles';
      var method = isEdit ? 'PUT' : 'POST';
      api(method, path, req).then(function (saved) {
        toast('ok', isEdit ? 'Профиль сохранён' : 'Профиль создан', saved && saved.name ? saved.name : name);
        if (saved && saved.id) state.full[saved.id] = saved;
        modal.close();
        return loadProfiles();
      }).catch(function (e) {
        // 400 (в т.ч. несовместимая легенда) показываем в баннере модалки
        showBanner(errBanner, e.message + (e.code ? ' [' + e.code + ']' : ''));
        toast('error', 'Не удалось сохранить', e.message);
        saveBtn.disabled = false; cancelBtn.disabled = false;
      });
    });
  }

  function showBanner(banner, text) {
    banner.classList.remove('hidden');
    banner.classList.remove('banner--ok'); banner.classList.add('banner--error');
    var m = banner.querySelector('.banner__msg'); if (m) m.textContent = text;
  }
  function hideBanner(banner) { banner.classList.add('hidden'); }
  function setFieldError(root, key, text) {
    var node = root.querySelector('[data-err="' + key + '"]');
    if (node) node.textContent = text;
  }
  function clearFieldErrors(root) {
    var nodes = root.querySelectorAll('.field__err');
    for (var i = 0; i < nodes.length; i++) nodes[i].textContent = '';
  }

  function screenKey(screen) {
    if (!screen) return '';
    return screen.width + 'x' + screen.height + '@' + screen.dpr;
  }
  function parseScreenKey(key) {
    var m = /^(\d+)x(\d+)@([\d.]+)$/.exec(key);
    if (!m) return null;
    return { w: Number(m[1]), h: Number(m[2]), dpr: Number(m[3]) };
  }

  function segmented(options, current, onChange) {
    var wrap = h('div', { class: 'segmented' });
    options.forEach(function (o) {
      var b = h('button', { class: 'segmented__opt' + (o.v === current ? ' is-active' : ''), type: 'button' },
        o.icon ? iconNode(o.icon) : null, h('span', { text: o.label }));
      b.addEventListener('click', function () {
        var opts = wrap.querySelectorAll('.segmented__opt');
        for (var i = 0; i < opts.length; i++) opts[i].classList.remove('is-active');
        b.classList.add('is-active');
        onChange(o.v);
      });
      wrap.appendChild(b);
    });
    wrap.set = function (v) {
      var opts = wrap.querySelectorAll('.segmented__opt');
      for (var i = 0; i < opts.length; i++) opts[i].classList.remove('is-active');
      var idx = options.map(function (o) { return o.v; }).indexOf(v);
      if (idx >= 0) opts[idx].classList.add('is-active');
    };
    wrap.getValue = function () {
      var active = wrap.querySelector('.segmented__opt.is-active');
      var idx = Array.prototype.indexOf.call(wrap.querySelectorAll('.segmented__opt'), active);
      return idx >= 0 ? options[idx].v : null;
    };
    return wrap;
  }

  // Chip-style array editor (tags, languages).
  function arrayInput(initial, placeholder) {
    var values = (initial || []).slice();
    var wrap = h('div', { class: 'langs' });
    var input = h('input', { type: 'text', placeholder: placeholder || 'значение + Enter' });
    function renderChips() {
      clear(wrap);
      values.forEach(function (v, i) {
        wrap.appendChild(h('span', { class: 'chip', text: v },
          h('span', { class: 'chip__x', role: 'button', onclick: function () { values.splice(i, 1); renderChips(); } }, iconNode('x'))));
      });
      wrap.appendChild(input);
      input.focus();
    }
    function add() {
      var v = input.value.trim();
      if (!v) return;
      if (values.indexOf(v) === -1) values.push(v);
      input.value = '';
      renderChips();
    }
    input.addEventListener('keydown', function (e) {
      if (e.key === 'Enter' || e.key === ',') { e.preventDefault(); add(); }
      else if (e.key === 'Backspace' && !input.value && values.length) { values.pop(); renderChips(); }
    });
    input.addEventListener('blur', add);
    renderChips();
    return { node: wrap, getValues: function () { return values.slice(); } };
  }

  /* ── Cookies view ───────────────────────────────────────────────────── */

  function selectedIds() { return Object.keys(state.selected); }

  function renderCookies(view) {
    var ids = selectedIds();
    if (ids.length !== 1) {
      view.appendChild(h('div', { class: 'empty' },
        iconNode('cookie', 'ico'),
        h('h3', { text: ids.length ? 'Выберите один профиль' : 'Профиль не выбран' }),
        h('p', { text: 'Cookies импортируются и экспортируются для одного профиля. Вернитесь в «Профили» и выделите ровно один.' }),
        h('button', { class: 'btn btn--primary', type: 'button', onclick: function () { go('profiles'); } }, iconNode('layers'), 'К профилям')));
      return;
    }
    var p = state.profiles.filter(function (x) { return x.id === ids[0]; })[0];
    if (!p) { go('profiles'); return; }

    var format = 'netscape';
    var fmtSeg = segmented([{ v: 'netscape', label: 'Netscape' }, { v: 'json', label: 'JSON' }], format, function (v) { format = v; updateExt(); });
    var extHint = h('span', { class: 'field__hint' }, 'файл .txt');
    function updateExt() { extHint.textContent = format === 'json' ? 'файл .json' : 'файл .txt'; }

    // import
    var importArea = h('textarea', { placeholder: 'Вставьте cookies (Netscape: строки формата domain\\tflag\\tpath\\tsecure\\texpiry\\tname\\tvalue, либо JSON-массив).', spellcheck: 'false' });
    var importResult = h('div', {});
    var importBtn = h('button', { class: 'btn btn--primary', type: 'button' }, iconNode('upload'), 'Импортировать');
    importBtn.addEventListener('click', function () {
      var data = importArea.value;
      if (!data.trim()) { toast('error', 'Нет данных', 'Вставьте cookies перед импортом.'); return; }
      importBtn.disabled = true;
      clear(importResult);
      api('POST', '/api/v1/profiles/' + encodeURIComponent(p.id) + '/cookies/import', { format: format, data: data }).then(function (r) {
        clear(importResult);
        importResult.appendChild(h('div', { class: 'banner banner--ok' }, iconNode('check'),
          h('div', { text: 'Импортировано: ' + (r.imported || 0) + (r.skipped ? (' · пропущено: ' + r.skipped) : '') })));
        toast('ok', 'Cookies импортированы', String(r.imported || 0) + ' шт.');
      }).catch(function (e) {
        clear(importResult);
        importResult.appendChild(h('div', { class: 'banner banner--error' }, iconNode('warning'), h('div', { text: e.message })));
        toast('error', 'Импорт cookies не удался', e.message);
      }).then(function () { importBtn.disabled = false; });
    });

    // export
    var exportBtn = h('button', { class: 'btn', type: 'button' }, iconNode('download'), 'Скачать');
    exportBtn.addEventListener('click', function () {
      exportBtn.disabled = true;
      api('GET', '/api/v1/profiles/' + encodeURIComponent(p.id) + '/cookies/export?format=' + format).then(function (r) {
        var name = ('cookies-' + safeName(p.name) + (format === 'json' ? '.json' : '.txt'));
        downloadText(name, r && r.data ? r.data : '', format === 'json' ? 'application/json' : 'text/plain');
        toast('ok', 'Экспорт готов', name);
      }).catch(function (e) {
        toast('error', 'Экспорт cookies не удался', e.message);
      }).then(function () { exportBtn.disabled = false; });
    });

    view.appendChild(h('div', { class: 'panel panel--narrow' },
      h('div', { class: 'panel__head' }, iconNode('cookie'), h('div', {}, h('h2', { text: 'Cookies — ' + p.name }), h('p', { text: 'Профиль ' + p.id }))),
      h('div', { class: 'stack' },
        h('div', { class: 'field' }, h('label', { text: 'Формат' }), h('div', { style: 'display:flex;gap:10px;align-items:center' }, fmtSeg, extHint)),
        h('div', { class: 'field' }, h('label', { text: 'Импорт' }), importArea, h('div', { class: 'field__hint', text: 'Netscape: строки через табы; JSON: массив объектов cookie.' })),
        h('div', {}, importBtn),
        importResult,
        h('div', { class: 'section-title', text: 'Экспорт' }),
        h('div', {}, exportBtn))));
  }

  function safeName(s) { return String(s || 'profile').replace(/[^\w\-.]+/g, '_').slice(0, 48); }

  function downloadText(filename, text, mime) {
    var blob = new Blob([text], { type: (mime || 'text/plain') + ';charset=utf-8' });
    var url = URL.createObjectURL(blob);
    var a = h('a', { href: url, download: filename });
    document.body.appendChild(a);
    a.click();
    if (a.parentNode) a.parentNode.removeChild(a);
    window.setTimeout(function () { URL.revokeObjectURL(url); }, 3000);
  }

  /* ── Import view ────────────────────────────────────────────────────── */

  // A /import warning is either a plain string (older servers / mocks) or an
  // object `{ "field", "kind" }` as the frozen contract specifies. Render both,
  // never `[object Object]`.
  function warningText(w) {
    if (w && typeof w === 'object') {
      var field = w.field != null ? String(w.field) : '';
      var kind = w.kind != null ? String(w.kind) : '';
      if (field && kind) return field + ' (' + kind + ')';
      if (field) return field;
      if (kind) return kind;
      return JSON.stringify(w);
    }
    return w == null ? '' : String(w);
  }

  function renderImport(view) {
    var source = 'dolphin';
    var srcSeg = segmented([
      { v: 'adspower', label: 'AdsPower' },
      { v: 'dolphin', label: 'Dolphin' },
      { v: 'gologin', label: 'GoLogin' }
    ], source, function (v) { source = v; });

    var area = h('textarea', { placeholder: 'Вставьте JSON-массив профилей источника (или объект { "profiles": [...] }).', spellcheck: 'false', style: 'min-height:180px' });
    var result = h('div', {});
    var btn = h('button', { class: 'btn btn--primary', type: 'button' }, iconNode('upload'), 'Импортировать');
    btn.addEventListener('click', function () {
      if (!area.value.trim()) { toast('error', 'Нет данных', 'Вставьте JSON источника.'); return; }
      btn.disabled = true;
      clear(result);
      api('POST', '/api/v1/import', { source: source, data: area.value }).then(function (r) {
        var imported = r && r.imported ? r.imported : [];
        var warnings = r && r.warnings ? r.warnings : [];
        clear(result);
        result.appendChild(h('div', { class: 'banner ' + (imported.length ? 'banner--ok' : 'banner--warn') }, iconNode(imported.length ? 'check' : 'warning'),
          h('div', {}, h('div', { text: 'Импортировано профилей: ' + imported.length }))));
        if (imported.length) {
          var list = h('ul', { class: 'warn-list', style: 'color:var(--text-dim)' });
          imported.slice(0, 12).forEach(function (pr) { list.appendChild(h('li', { text: pr.name || pr.id })); });
          if (imported.length > 12) list.appendChild(h('li', { text: '…и ещё ' + (imported.length - 12) }));
          result.appendChild(h('div', { class: 'panel', style: 'margin-top:12px;padding:12px' }, h('div', { class: 'section-title', text: 'Профили' }), list));
        }
        if (warnings.length) {
          var wl = h('ul', { class: 'warn-list' });
          warnings.forEach(function (w) { wl.appendChild(h('li', { text: warningText(w) })); });
          result.appendChild(h('div', { class: 'banner banner--warn', style: 'margin-top:12px' }, iconNode('warning'),
            h('div', {}, h('div', { text: 'Предупреждения (' + warnings.length + ')' }), wl)));
        }
        toast(imported.length ? 'ok' : 'warn', 'Импорт завершён', 'Импортировано: ' + imported.length + (warnings.length ? ' · предупреждений: ' + warnings.length : ''));
        if (imported.length) loadProfiles();
      }).catch(function (e) {
        clear(result);
        result.appendChild(h('div', { class: 'banner banner--error' }, iconNode('warning'), h('div', { text: e.message })));
        toast('error', 'Импорт не удался', e.message);
      }).then(function () { btn.disabled = false; });
    });

    view.appendChild(h('div', { class: 'panel panel--narrow' },
      h('div', { class: 'panel__head' }, iconNode('download'), h('div', {}, h('h2', { text: 'Импорт профилей' }), h('p', { text: 'Вставьте экспорт из панели источника.' }))),
      h('div', { class: 'stack' },
        h('div', { class: 'field' }, h('label', { text: 'Источник' }), srcSeg),
        h('div', { class: 'field' }, h('label', { text: 'Данные (JSON)' }), area),
        h('div', {}, btn, h('button', { class: 'btn', type: 'button', style: 'margin-left:8px', onclick: function () { area.value = exampleJson(source); } }, 'Вставить пример')),
        result)));
  }

  function exampleJson(source) {
    var sample = [
      { name: 'Imported-01', os: 'windows', proxy: { scheme: 'socks5', host: '10.0.0.1', port: 1080 }, group: 'team-a' },
      { name: 'Imported-02', os: 'macos', proxy: { scheme: 'http', host: '10.0.0.2', port: 8080 } }
    ];
    return JSON.stringify({ source: source, profiles: sample }, null, 2);
  }

  /* ── Settings view ──────────────────────────────────────────────────── */

  function renderSettings(view) {
    // Connection panel
    var apiInput = h('input', { type: 'text', value: state.api, placeholder: DEFAULT_API });
    var tokenInput = h('input', { type: 'password', value: state.token, placeholder: 'MIRAGE_API_TOKEN (если задан)' });
    var saveConn = h('button', { class: 'btn btn--primary', type: 'button', text: 'Сохранить' });
    saveConn.addEventListener('click', function () {
      state.api = normalizeApi(apiInput.value.trim() || DEFAULT_API);
      state.token = tokenInput.value.trim();
      lsSet(LS.api, state.api); lsSet(LS.token, state.token);
      toast('ok', 'Настройки сохранены', 'API: ' + state.api);
      refreshHealth(true);
      loadProfiles();
    });
    var testConn = h('button', { class: 'btn', type: 'button' }, iconNode('refresh'), 'Проверить связь');
    testConn.addEventListener('click', function () {
      state.api = normalizeApi(apiInput.value.trim() || DEFAULT_API);
      state.token = tokenInput.value.trim();
      lsSet(LS.api, state.api); lsSet(LS.token, state.token);
      refreshHealth(false);
    });

    var conn = h('div', { class: 'panel panel--narrow' },
      h('div', { class: 'panel__head' }, iconNode('server'), h('div', {}, h('h2', { text: 'Подключение к API' }), h('p', { text: 'Адрес локального REST API и токен доступа.' }))),
      h('div', { class: 'stack' },
        h('div', { class: 'field' }, h('label', { text: 'API URL' }), apiInput),
        h('div', { class: 'field' }, h('label', { text: 'API-токен' }), tokenInput, h('div', { class: 'field__hint', text: 'Нужен, только если на сервере задан MIRAGE_API_TOKEN.' })),
        h('div', {}, saveConn, h('span', { style: 'display:inline-block;width:8px' }), testConn)));

    // Vault + health panel
    var vaultKv = h('dl', { class: 'kv' });
    var pwInput = h('input', { type: 'password', placeholder: 'Пароль хранилища' });
    var unlockBtn = h('button', { class: 'btn', type: 'button' }, iconNode('key'), 'Разблокировать');
    var unlockResult = h('div', {});
    unlockBtn.addEventListener('click', function () {
      if (!pwInput.value) { toast('error', 'Пустой пароль', 'Введите пароль хранилища.'); return; }
      unlockBtn.disabled = true;
      clear(unlockResult);
      api('POST', '/api/v1/vault/unlock', { password: pwInput.value }).then(function () {
        toast('ok', 'Хранилище разблокировано');
        pwInput.value = '';
        clear(unlockResult);
        unlockResult.appendChild(h('div', { class: 'banner banner--ok' }, iconNode('check'), h('div', { text: 'Хранилище разблокировано.' })));
        refreshHealth(true);
      }).catch(function (e) {
        clear(unlockResult);
        unlockResult.appendChild(h('div', { class: 'banner banner--error' }, iconNode('warning'), h('div', { text: e.message })));
        toast('error', 'Не удалось разблокировать', e.message);
      }).then(function () { unlockBtn.disabled = false; });
    });

    function paintVault() {
      clear(vaultKv);
      var hl = state.health;
      vaultKv.appendChild(h('dt', { text: 'Хранилище' }));
      vaultKv.appendChild(h('dd', {}, h('span', { class: 'dot', dataset: { state: hl ? (hl.vault === 'unlocked' ? 'unlocked' : 'locked') : 'unknown' } }), ' ' + (hl ? (hl.vault || '?') : 'н/д')));
      vaultKv.appendChild(h('dt', { text: 'Версия' }));
      vaultKv.appendChild(h('dd', { text: hl ? (hl.version || '?') : 'н/д' }));
      vaultKv.appendChild(h('dt', { text: 'Запущено' }));
      vaultKv.appendChild(h('dd', { text: hl ? String(hl.running != null ? hl.running : '?') : 'н/д' }));
    }
    paintVault();

    var vault = h('div', { class: 'panel panel--narrow' },
      h('div', { class: 'panel__head' }, iconNode('lock'), h('div', {}, h('h2', { text: 'Хранилище и статус' }), h('p', { text: 'Состояние vault и версия сервера (GET /health).' }))),
      h('div', { class: 'stack' },
        vaultKv,
        h('div', { class: 'row' }, h('div', { class: 'field field--grow' }, h('label', { text: 'Пароль хранилища' }), pwInput)),
        h('div', {}, unlockBtn),
        unlockResult));

    // Mock panel
    var mockSwitch = h('input', { type: 'checkbox', checked: state.mock });
    mockSwitch.addEventListener('change', function () { enableMock(mockSwitch.checked); });
    var mock = h('div', { class: 'panel panel--narrow' },
      h('div', { class: 'panel__head' }, iconNode('shield'), h('div', {}, h('h2', { text: 'Мок-режим' }), h('p', { text: 'Работа без сервера на встроенных фикстурах (mock.js).' }))),
      h('div', { class: 'stack' },
        h('label', { class: 'switch' }, mockSwitch, h('span', { class: 'switch__track' }), h('span', { class: 'switch__label', text: 'Включён' })),
        h('div', { class: 'banner banner--info' }, iconNode('info'),
          h('div', {},
            h('div', { text: 'В мок-режиме работает всё, кроме запуска браузера: Start/Stop помечены «только с реальным сервером» и не запускают Chromium.' }),
            h('div', { class: 'muted', style: 'margin-top:4px', text: 'Переключается через localStorage (mirage.mock) или ?mock=1 в URL.' })))));

    view.appendChild(h('div', { class: 'stack' }, conn, vault, mock));

    // keep vault panel fresh when health updates elsewhere
    renderSettings.repaint = paintVault;
  }

  function renderSettingsRepaint() { if (typeof renderSettings.repaint === 'function') renderSettings.repaint(); }

  /* ── Mock toggle ────────────────────────────────────────────────────── */

  function enableMock(on) {
    state.mock = !!on;
    lsSet(LS.mock, on ? '1' : '0');
    if (on) {
      // Drop a forced ?mock=1 so the choice is honest after this point.
      if (/[?&]mock=1\b/.test(window.location.search) && window.history && window.history.replaceState) {
        var url = window.location.pathname + window.location.search.replace(/([?&])mock=1\b&?/, '$1').replace(/[?&]$/, '');
        window.history.replaceState({}, '', url || window.location.pathname);
      }
    }
    var badge = $('#mock-badge'); if (badge) badge.hidden = !state.mock;
    toast('info', on ? 'Мок-режим включён' : 'Мок-режим выключен', on ? 'Данные берутся из фикстур.' : 'Обращения идут к ' + normalizeApi(state.api));
    if (on) state.health = { ok: true, version: '0.1.0', vault: 'unlocked', running: 0 };
    loadProfiles();
    refreshHealth(true);
    renderSettingsRepaint();
  }

  /* ── Bootstrap ──────────────────────────────────────────────────────── */

  function bindNav() {
    var navItems = document.querySelectorAll('.nav__item');
    for (var i = 0; i < navItems.length; i++) {
      (function (el) {
        el.addEventListener('click', function () { go(el.getAttribute('data-route')); });
      })(navItems[i]);
    }
    $('#health-refresh').addEventListener('click', function () { refreshHealth(false); });
  }

  function init() {
    state.mock = detectMock();
    var params = new URLSearchParams(window.location.search);
    var deepRoute = params.get('route');
    if (deepRoute && ROUTES[deepRoute]) state.route = deepRoute;
    hydrateIcons(document);
    bindNav();
    render();
    refreshHealth(state.mock ? true : false);
    loadProfiles().then(function () {
      // Deep-link: ?select=<profile-id> (or ?select=first) preselects a profile.
      var sel = params.get('select');
      if (!sel) return;
      if (sel === 'first' && state.profiles[0]) state.selected[state.profiles[0].id] = true;
      else if (state.profiles.some(function (p) { return p.id === sel; })) state.selected[sel] = true;
      if (Object.keys(state.selected).length) render();
    });
    startPolling();
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init);
  else init();
})();

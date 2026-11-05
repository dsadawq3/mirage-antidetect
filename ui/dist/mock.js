/* ============================================================================
   Mirage UI — mock backend (mock.js)
   ----------------------------------------------------------------------------
   Intercepts the local API (docs/API.md) entirely in the browser, so the whole
   UI is demonstrable without the Rust core. Activated by localStorage
   `mirage.mock = "1"` or a `?mock=1` query string.

   Exposes: window.mirageMock.request(method, path, body)
            -> Promise<{ status, ok, body }>

   Fixtures follow docs/CONTRACT.md: seeds produce deterministic, consistent
   fingerprints (deviceMemory in {2,4,8}, hardwareConcurrency in the allowed
   set, availHeight = height - OS chrome, renderer matching the OS, ...).
   ========================================================================= */
(function () {
  'use strict';

  // ── Seeded RNG + helpers ───────────────────────────────────────────────

  function mulberry32(seed) {
    let a = seed >>> 0;
    return function () {
      a |= 0; a = (a + 0x6D2B79F5) | 0;
      let t = Math.imul(a ^ (a >>> 15), 1 | a);
      t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  }
  function pick(rng, arr) { return arr[Math.floor(rng() * arr.length) % arr.length]; }
  function intBetween(rng, lo, hi) { return lo + Math.floor(rng() * (hi - lo + 1)); }

  function uuid4() {
    if (typeof crypto !== 'undefined' && crypto.randomUUID) return crypto.randomUUID();
    // RFC 4122 v4 fallback
    const b = new Uint8Array(16);
    (crypto && crypto.getRandomValues ? crypto.getRandomValues(b) : b.forEach((_, i) => { b[i] = Math.floor(Math.random() * 256); }));
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    const h = [];
    for (let i = 0; i < 16; i++) h.push(b[i].toString(16).padStart(2, '0'));
    return h.slice(0, 4).join('') + '-' + h.slice(4, 6).join('') + '-' + h.slice(6, 8).join('') + '-' + h.slice(8, 10).join('') + '-' + h.slice(10, 16).join('');
  }
  function clone(v) { return JSON.parse(JSON.stringify(v)); }

  // Deep merge: patch wins, nested objects merge (partial Fingerprint overrides).
  function deepMerge(base, patch) {
    if (patch === undefined) return clone(base);
    if (patch === null) return null;
    if (Array.isArray(patch)) return clone(patch);
    if (typeof patch !== 'object') return patch;
    const out = (base && typeof base === 'object' && !Array.isArray(base)) ? clone(base) : {};
    for (const k of Object.keys(patch)) {
      const pv = patch[k];
      out[k] = (pv && typeof pv === 'object' && !Array.isArray(pv)) ? deepMerge(out[k], pv) : clone(pv);
    }
    return out;
  }

  // ── Trimmed fp-db samples (representative subset, kept small on purpose) ─

  const CHROME_MAJOR = 156;
  const CHROME_FULL = '156.0.8078.80';
  const GREASE_BRAND = 'Not:A-Brand';
  const GREASE_VERSION = '8';
  const GREASE_FULL_VERSION = '8.0.0.0';

  const OS_META = {
    windows: {
      versions: ['10', '11'],
      platform: 'Win32', chPlatform: 'Windows',
      availOffset: 40,
      ua: (m) => 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/' + m + '.0.0.0 Safari/537.36'
    },
    macos: {
      versions: ['10.15', '11', '12', '13', '14'],
      platform: 'MacIntel', chPlatform: 'macOS',
      availOffset: 25,
      ua: (m) => 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/' + m + '.0.0.0 Safari/537.36'
    },
    linux: {
      versions: ['5.15', '6.1', '6.5'],
      platform: 'Linux x86_64', chPlatform: 'Linux',
      availOffset: 24,
      ua: (m) => 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/' + m + '.0.0.0 Safari/537.36'
    }
  };

  const LINUX_DRIVERS = ['550.107.02', '535.216.01', '535.183.01'];

  const SCREENS = [
    { width: 1920, height: 1080, dpr: 1, os: null },
    { width: 1366, height: 768, dpr: 1, os: null },
    { width: 2560, height: 1440, dpr: 1, os: null },
    { width: 1440, height: 900, dpr: 1, os: null },
    { width: 1600, height: 900, dpr: 1, os: null },
    { width: 1536, height: 864, dpr: 1.25, os: 'windows' },
    { width: 2560, height: 1440, dpr: 1.5, os: 'windows' },
    { width: 1440, height: 900, dpr: 2, os: 'macos' },
    { width: 1512, height: 982, dpr: 2, os: 'macos' },
    { width: 1728, height: 1117, dpr: 2, os: 'macos' }
  ];

  const GPUS = {
    windows: [
      { vendor: 'Google Inc. (NVIDIA)', renderer: 'ANGLE (NVIDIA, NVIDIA GeForce GTX 1660 SUPER (0x000021C4) Direct3D11 vs_5_0 ps_5_0, D3D11)' },
      { vendor: 'Google Inc. (NVIDIA)', renderer: 'ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 (0x00002504) Direct3D11 vs_5_0 ps_5_0, D3D11)' },
      { vendor: 'Google Inc. (AMD)', renderer: 'ANGLE (AMD, AMD Radeon RX 580 Series (0x000067DF) Direct3D11 vs_5_0 ps_5_0, D3D11)' },
      { vendor: 'Google Inc. (Intel)', renderer: 'ANGLE (Intel, Intel(R) UHD Graphics 630 (0x00003E9B) Direct3D11 vs_5_0 ps_5_0, D3D11)' }
    ],
    macos: [
      { vendor: 'Google Inc. (Apple)', renderer: 'ANGLE (Apple, ANGLE Metal Renderer: Apple M1, Unspecified Version)' },
      { vendor: 'Google Inc. (Apple)', renderer: 'ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified Version)' },
      { vendor: 'Google Inc. (AMD)', renderer: 'ANGLE (AMD, ANGLE Metal Renderer: AMD Radeon Pro 5500M, Unspecified Version)' }
    ],
    linux: [
      { vendor: 'Google Inc. (NVIDIA)', renderer: 'ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 (0x00002503) OpenGL ES 3.2 NVIDIA 550.107.02)' },
      { vendor: 'Google Inc. (Intel)', renderer: 'ANGLE (Intel, Mesa Intel(R) UHD Graphics 620 (Kaby Lake GT2) (0x00005917) OpenGL ES 3.2 Mesa 23.2.1)' },
      { vendor: 'Google Inc. (Mesa)', renderer: 'ANGLE (Mesa, llvmpipe (LLVM 15.0.7, 256 bits))' }
    ]
  };

  const LOCALES = [
    { languages: ['en-US', 'en'], timezone: 'America/New_York', lat: 40.7128, lon: -74.006 },
    { languages: ['en-US', 'en'], timezone: 'America/Los_Angeles', lat: 34.0522, lon: -118.2437 },
    { languages: ['en-GB', 'en'], timezone: 'Europe/London', lat: 51.5074, lon: -0.1278 },
    { languages: ['de-DE', 'de', 'en'], timezone: 'Europe/Berlin', lat: 52.52, lon: 13.405 },
    { languages: ['fr-FR', 'fr', 'en'], timezone: 'Europe/Paris', lat: 48.8566, lon: 2.3522 },
    { languages: ['pt-BR', 'pt'], timezone: 'America/Sao_Paulo', lat: -23.5505, lon: -46.6333 },
    { languages: ['ja-JP', 'ja', 'en'], timezone: 'Asia/Tokyo', lat: 35.6762, lon: 139.6503 },
    { languages: ['ru-RU', 'ru', 'en'], timezone: 'Europe/Moscow', lat: 55.7558, lon: 37.6173 }
  ];

  const FONTS = {
    windows: ['Arial', 'Calibri', 'Cambria', 'Consolas', 'Courier New', 'Georgia', 'Segoe UI', 'Tahoma', 'Times New Roman', 'Trebuchet MS', 'Verdana', 'Impact', 'Comic Sans MS', 'Palatino Linotype', 'Lucida Console', 'Microsoft Sans Serif', 'Franklin Gothic Medium', 'Candara'],
    macos: ['Helvetica', 'Helvetica Neue', 'Arial', 'Menlo', 'Monaco', 'Georgia', 'Times', 'Verdana', 'Courier New', 'Impact', 'Copperplate', 'Optima', 'Palatino', 'Tahoma', 'Trebuchet MS', 'Geneva', 'Rockwell', 'Chalkboard'],
    linux: ['DejaVu Sans', 'DejaVu Sans Mono', 'DejaVu Serif', 'Liberation Sans', 'Liberation Serif', 'Liberation Mono', 'Noto Sans', 'Noto Serif', 'Ubuntu', 'Ubuntu Mono', 'Cantarell', 'Carlito', 'Arimo', 'Tinos', 'Cousine', 'Noto Color Emoji']
  };

  const HW = [2, 4, 6, 8, 12, 16, 20, 24];
  const MEM = [2, 4, 8];

  function darwinFor(v) {
    return ({ '10.15': '19.6.0', '11': '20.6.0', '12': '21.6.0', '13': '22.5.0', '14': '23.4.0' })[v] || '23.4.0';
  }

  function generateFingerprint(seed, os, overrides) {
    const meta = OS_META[os] || OS_META.windows;
    const rng = mulberry32(seed >>> 0);

    const osVersion = pick(rng, meta.versions);
    const locale = pick(rng, LOCALES);
    const screens = SCREENS.filter((s) => !s.os || s.os === os);
    const screen = pick(rng, screens.length ? screens : SCREENS);
    const gpu = pick(rng, GPUS[os] || GPUS.windows);
    const fonts = (FONTS[os] || FONTS.windows).slice(0, 16 + intBetween(rng, 0, 4));

    let chPlatformVersion;
    if (os === 'windows') chPlatformVersion = osVersion === '10' ? '10.0.0' : pick(rng, ['13.0.0', '15.0.0']);
    else if (os === 'macos') chPlatformVersion = darwinFor(osVersion);
    else chPlatformVersion = osVersion + '.0';

    const renderer = (os === 'linux' && gpu.renderer.includes('OpenGL ES 3.2 NVIDIA'))
      ? gpu.renderer.replace(/NVIDIA \d+(\.\d+)*/, 'NVIDIA ' + pick(rng, LINUX_DRIVERS))
      : gpu.renderer;

    const fp = {
      os: os,
      osVersion: osVersion,
      platform: meta.platform,
      ua: meta.ua(CHROME_MAJOR),
      chromeVersion: CHROME_FULL,
      languages: locale.languages.slice(),
      timezone: locale.timezone,
      geo: { lat: locale.lat, lon: locale.lon },
      screen: {
        width: screen.width, height: screen.height,
        availWidth: screen.width,
        availHeight: screen.height - meta.availOffset,
        dpr: screen.dpr, colorDepth: 24
      },
      hardwareConcurrency: pick(rng, HW),
      deviceMemory: pick(rng, MEM),
      webgl: {
        vendor: gpu.vendor,
        renderer: renderer,
        maxTextureSize: 16384,
        maxViewportDims: os === 'windows' ? [32767, 32767] : [16384, 16384],
        maxRenderbufferSize: 16384
      },
      canvasSeed: Math.floor(rng() * 4294967295) >>> 0,
      audioSeed: Math.floor(rng() * 4294967295) >>> 0,
      fonts: fonts,
      webRTC: 'real',
      clientHints: {
        brands: [
          { brand: GREASE_BRAND, version: GREASE_VERSION },
          { brand: 'Chromium', version: String(CHROME_MAJOR) },
          { brand: 'Google Chrome', version: String(CHROME_MAJOR) }
        ],
        mobile: false,
        platform: meta.chPlatform,
        platformVersion: chPlatformVersion,
        architecture: 'x86',
        bitness: '64',
        uaFullVersion: CHROME_FULL
      }
    };

    return overrides ? deepMerge(fp, overrides) : fp;
  }

  function summaryOf(p) {
    return {
      id: p.id, name: p.name, tags: p.tags || [],
      running: RUNNING.has(p.id),
      proxy: p.proxy ? { scheme: p.proxy.scheme, host: p.proxy.host, port: p.proxy.port } : null,
      createdAt: p.createdAt, updatedAt: p.updatedAt
    };
  }

  // ── In-memory store ──────────────────────────────────────────────────

  const now = () => new Date().toISOString();

  const STORE = { profiles: [], seeds: {} };
  const RUNNING = new Set();

  function seedProfile(opts) {
    const request = {
      name: opts.name, tags: opts.tags || [], os: opts.os || 'windows',
      seed: opts.seed, proxy: opts.proxy || null,
      notes: opts.notes || ''
    };
    return createProfile(request);
  }

  function createProfile(req) {
    const os = (req.os && OS_META[req.os]) ? req.os : 'windows';
    const seed = (typeof req.seed === 'number' && isFinite(req.seed)) ? (req.seed >>> 0) : (Math.floor(Math.random() * 4294967295) >>> 0);
    const id = uuid4();
    const t = now();
    const profile = {
      id: id,
      name: String(req.name || 'Профиль'),
      tags: Array.isArray(req.tags) ? req.tags.slice() : [],
      seed: seed,
      fingerprint: generateFingerprint(seed, os, req.overrides || null),
      proxy: normalizeProxy(req.proxy),
      notes: String(req.notes || ''),
      createdAt: t,
      updatedAt: t
    };
    STORE.profiles.push(profile);
    STORE.seeds[id] = { seed: seed, os: os };
    return profile;
  }

  function normalizeProxy(p) {
    if (!p || !p.host) return null;
    return {
      scheme: (p.scheme === 'http' || p.scheme === 'socks5') ? p.scheme : 'socks5',
      host: String(p.host),
      port: Number(p.port) || 1080,
      username: p.username ? String(p.username) : null,
      password: p.password ? String(p.password) : null
    };
  }

  function findProfile(id) { return STORE.profiles.find((p) => p.id === id) || null; }

  // ── Seed fixtures ────────────────────────────────────────────────────

  seedProfile({ name: 'US Shop — Amazon', tags: ['work', 'shop'], os: 'windows', seed: 20261103, proxy: { scheme: 'socks5', host: '45.133.1.10', port: 1080, username: 'u1001', password: 'p1001' } });
  seedProfile({ name: 'DE Ads — Facebook', tags: ['ads'], os: 'windows', seed: 20261104, proxy: { scheme: 'http', host: '91.206.14.55', port: 8080, username: 'user', password: 'pass' } });
  seedProfile({ name: 'FR Dropship', tags: ['work', 'shop'], os: 'macos', seed: 20261105, proxy: { scheme: 'socks5', host: '51.83.44.12', port: 1080, username: null, password: null } });
  seedProfile({ name: 'UK Support', tags: ['support'], os: 'windows', seed: 20261106, proxy: null });
  seedProfile({ name: 'JP Scraper', tags: ['scrape'], os: 'linux', seed: 20261107, proxy: { scheme: 'socks5', host: '103.55.9.7', port: 1080, username: 'jp', password: 'jp' } });
  seedProfile({ name: 'BR Affiliate', tags: ['ads', 'aff'], os: 'windows', seed: 20261108, proxy: { scheme: 'socks5', host: '177.54.10.3', port: 1080, username: null, password: null } });
  seedProfile({ name: 'NL Test', tags: ['test'], os: 'linux', seed: 20261109, proxy: null });
  seedProfile({ name: 'CA Personal', tags: ['personal'], os: 'macos', seed: 20261110, proxy: { scheme: 'http', host: '142.44.20.9', port: 3128, username: 'c', password: 'c' } });

  // Two profiles start "running" so the grid shows both states on first load.
  RUNNING.add(STORE.profiles[4].id);
  RUNNING.add(STORE.profiles[1].id);

  // ── Response helpers ─────────────────────────────────────────────────

  function respond(status, body) { return { status: status, ok: status >= 200 && status < 300, body: body }; }
  function ok(body) { return respond(200, body); }
  function apiError(status, code, message) { return respond(status, { error: { code: code, message: message } }); }

  // ── Router ───────────────────────────────────────────────────────────

  function route(method, pathname, query, body) {
    const parts = pathname.replace(/^\/+|\/+$/g, '').split('/');
    // parts: ["api","v1", ...]
    const seg = parts.slice(2);

    // health
    if (method === 'GET' && seg.length === 1 && seg[0] === 'health') {
      return ok({ ok: true, version: '0.1.0', vault: 'unlocked', running: RUNNING.size });
    }
    // vault unlock
    if (method === 'POST' && seg.length === 2 && seg[0] === 'vault' && seg[1] === 'unlock') {
      const pw = body && typeof body.password === 'string' ? body.password : '';
      if (!pw) return apiError(401, 'vault_locked', 'Пустой пароль хранилища');
      return ok({});
    }
    // profiles collection
    if (seg.length === 1 && seg[0] === 'profiles') {
      if (method === 'GET') return ok({ profiles: STORE.profiles.map(summaryOf) });
      if (method === 'POST') {
        if (!body || typeof body.name !== 'string' || !body.name.trim()) {
          return apiError(400, 'invalid_request', 'Поле name обязательно');
        }
        const p = createProfile(body);
        return respond(201, p);
      }
    }
    // profile item + sub-resources
    if (seg.length >= 2 && seg[0] === 'profiles') {
      const id = seg[1];
      const p = findProfile(id);
      if (!p) return apiError(404, 'not_found', 'Профиль не найден: ' + id);

      if (seg.length === 2) {
        if (method === 'GET') return ok(p);
        if (method === 'PUT') {
          if (body && typeof body.name === 'string' && body.name.trim()) p.name = body.name;
          if (body && Array.isArray(body.tags)) p.tags = body.tags.slice();
          if (body && Object.prototype.hasOwnProperty.call(body, 'proxy')) p.proxy = normalizeProxy(body.proxy);
          if (body && Object.prototype.hasOwnProperty.call(body, 'notes')) p.notes = String(body.notes || '');
          const s = STORE.seeds[id];
          const hasSeed = body && typeof body.seed === 'number' && isFinite(body.seed);
          const hasOverrides = body && Object.prototype.hasOwnProperty.call(body, 'overrides');
          // Mirrors the real API PUT semantics exactly: a PUT can never change
          // the OS (the contract has no `os` field — it rebuilds from the stored
          // OS), so a new seed regenerates from `s.os`, then overrides layer on
          // top. `s.os` is set once at creation and never patched here.
          if (hasSeed) {
            s.seed = body.seed >>> 0;
            p.seed = s.seed;
          }
          if (hasSeed || hasOverrides) {
            p.fingerprint = generateFingerprint(s.seed, s.os, hasOverrides ? (body.overrides || null) : null);
          }
          p.updatedAt = now();
          return ok(p);
        }
        if (method === 'DELETE') {
          const i = STORE.profiles.indexOf(p);
          if (i >= 0) STORE.profiles.splice(i, 1);
          RUNNING.delete(id);
          delete STORE.seeds[id];
          return respond(204, null);
        }
      }
      if (seg.length === 3 && seg[2] === 'launch' && method === 'POST') {
        if (RUNNING.has(id)) return apiError(409, 'already_running', 'Профиль уже запущен');
        RUNNING.add(id);
        return ok({ target_id: 'target-' + id.slice(0, 8), ws_url: 'ws://127.0.0.1:9222/devtools/browser/' + id, debug_port: 9222, profile_dir: '%LOCALAPPDATA%\\Mirage\\profiles\\' + id });
      }
      if (seg.length === 3 && seg[2] === 'stop' && method === 'POST') {
        if (!RUNNING.has(id)) return apiError(409, 'not_running', 'Профиль не запущен');
        RUNNING.delete(id);
        return ok({});
      }
      if (seg.length === 4 && seg[2] === 'cookies' && seg[3] === 'import' && method === 'POST') {
        const data = body && typeof body.data === 'string' ? body.data : '';
        const fmt = body && body.format === 'json' ? 'json' : 'netscape';
        let imported = 0, skipped = 0;
        if (fmt === 'json') {
          try { const arr = JSON.parse(data); imported = Array.isArray(arr) ? arr.length : 0; skipped = Array.isArray(arr) ? 0 : 1; }
          catch (e) { imported = 0; skipped = 1; }
        } else {
          data.split(/\r?\n/).forEach((line) => {
            const s = line.trim();
            if (!s) return;
            if (s.charAt(0) === '#') { skipped++; return; }
            imported++;
          });
        }
        return ok({ imported: imported, skipped: skipped });
      }
      if (seg.length === 4 && seg[2] === 'cookies' && seg[3] === 'export' && method === 'GET') {
        const fmt = query.get('format') === 'json' ? 'json' : 'netscape';
        if (fmt === 'json') {
          return ok({ format: 'json', data: JSON.stringify([
            { name: '_ga', value: 'GA1.2.' + id.slice(0, 8), domain: '.example.com', path: '/', expires: 1798761600, httpOnly: false, secure: true, sameSite: 'Lax' },
            { name: 'sessionid', value: id.replace(/-/g, '').slice(0, 20), domain: '.example.com', path: '/', expires: 1798761600, httpOnly: true, secure: true, sameSite: 'Lax' }
          ], null, 2) });
        }
        const lines = [
          '# Netscape HTTP Cookie File',
          '.example.com\tTRUE\t/\tTRUE\t1798761600\t_ga\tGA1.2.' + id.slice(0, 8),
          '.example.com\tTRUE\t/\tTRUE\t1798761600\tsessionid\t' + id.replace(/-/g, '').slice(0, 20)
        ];
        return ok({ format: 'netscape', data: lines.join('\n') + '\n' });
      }
    }
    // status
    if (method === 'GET' && seg.length === 1 && seg[0] === 'status') {
      const running = STORE.profiles.filter((p) => RUNNING.has(p.id)).map((p) => ({
        profile_id: p.id, target_id: 'target-' + p.id.slice(0, 8),
        debug_port: 9222, started_at: p.updatedAt
      }));
      return ok({ running: running });
    }
    // proxy check
    if (method === 'POST' && seg.length === 2 && seg[0] === 'proxy' && seg[1] === 'check') {
      const host = body && body.host ? String(body.host) : '';
      if (!host) return ok({ ok: false, error: 'host обязателен' });
      if (/fail|dead|bad/i.test(host)) return ok({ ok: false, error: 'connection refused (mock)' });
      const rng = mulberry32((host.length * 2654435761) >>> 0);
      const ip = [intBetween(rng, 5, 220), intBetween(rng, 1, 254), intBetween(rng, 1, 254), intBetween(rng, 1, 254)].join('.');
      return ok({ ok: true, ip: ip, latency_ms: intBetween(rng, 18, 190) });
    }
    // import
    if (method === 'POST' && seg.length === 1 && seg[0] === 'import') {
      const source = body && body.source ? String(body.source) : 'dolphin';
      const data = body && typeof body.data === 'string' ? body.data : '';
      // Unknown source is a client error on the real API (run_import), not a warning.
      if (source !== 'dolphin' && source !== 'adspower' && source !== 'gologin') {
        return apiError(400, 'invalid_request', 'unknown import source `' + source + '` (use adspower, dolphin or gologin)');
      }
      let parsed;
      try { parsed = JSON.parse(data); }
      catch (e) { return apiError(400, 'invalid_request', 'Данные не являются валидным JSON'); }
      const list = Array.isArray(parsed) ? parsed : (Array.isArray(parsed.profiles) ? parsed.profiles : null);
      // Warnings follow the frozen contract: [{ "field": ..., "kind": "mapped"|"defaulted"|"ignored" }].
      if (!list) return ok({ imported: [], warnings: [{ field: 'data', kind: 'ignored' }] });
      const imported = [];
      const warnings = [];
      list.forEach((item, i) => {
        const name = item && (item.name || item.profile_name || item.title) ? String(item.name || item.profile_name || item.title) : ('Imported ' + (i + 1));
        const osRaw = item && (item.os || item.platform) ? String(item.os || item.platform).toLowerCase() : 'windows';
        const os = OS_META[osRaw] ? osRaw : (osRaw.indexOf('mac') === 0 ? 'macos' : (osRaw.indexOf('lin') === 0 ? 'linux' : 'windows'));
        const p = createProfile({
          name: name, tags: ['imported'], os: os,
          proxy: item && item.proxy ? item.proxy : null
        });
        imported.push(p);
        if (item && (item.fingerprint === undefined)) warnings.push({ field: 'fingerprint', kind: 'defaulted' });
        if (item && (item.group || item.group_id)) warnings.push({ field: 'group', kind: 'ignored' });
      });
      return ok({ imported: imported, warnings: warnings });
    }

    return apiError(404, 'not_found', 'Неизвестный эндпоинт: ' + method + ' ' + pathname);
  }

  // ── Public API ───────────────────────────────────────────────────────

  function request(method, path, body) {
    return new Promise((resolve) => {
      let url;
      try { url = new URL(path, 'http://mock.local'); }
      catch (e) { resolve(apiError(400, 'invalid_request', 'Некорректный путь: ' + path)); return; }
      const m = String(method || 'GET').toUpperCase();
      // Small latency so skeletons and loading states are actually visible.
      const delay = 90 + Math.floor(Math.random() * 140);
      setTimeout(() => {
        let res;
        try { res = route(m, url.pathname, url.searchParams, body || null); }
        catch (e) { res = apiError(500, 'internal', 'Ошибка мок-бэкенда: ' + (e && e.message ? e.message : String(e))); }
        resolve(clone(res));
      }, delay);
    });
  }

  window.mirageMock = { request: request };
})();

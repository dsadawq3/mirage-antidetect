/*
 * Mirage inject layer - module "navigator" (see docs/CONTRACT.md).
 *
 * Spoofs the Navigator.prototype identity fields that fingerprint pages
 * read first: platform, hardwareConcurrency, deviceMemory, languages,
 * language, userAgent, webdriver, maxTouchPoints, vendor.
 *
 * Also covers the WORKER scope: a page-level getter never runs inside
 * new Worker(...), so worker scripts would keep reading the real
 * hardwareConcurrency / deviceMemory / languages / platform / userAgent
 * through WorkerNavigator. Every worker whose source this page can
 * produce is therefore started from a bootstrap that first runs a
 * self-contained snippet patching WorkerNavigator.prototype with the
 * same legend (details and limits in the worker section below).
 *
 * Why a JS layer at all: CDP covers userAgent/platform via
 * Network.setUserAgentOverride, but hardwareConcurrency, deviceMemory,
 * languages, webdriver and maxTouchPoints have no CDP override - the
 * prototype accessor swap is the only way to align them with the profile.
 * Overriding on Navigator.prototype (never on the instance) keeps
 * getOwnPropertyDescriptor(Navigator.prototype, ...) looking like a normal
 * WebIDL accessor and covers the single shared navigator object.
 *
 * Fidelity decisions:
 * - `languages` hands out ONE frozen array, stable between calls: real
 *   Chrome returns the same frozen array object on every access.
 * - `userAgent` returns fp.ua; CDP already sets the same string - the
 *   prototype getter is defence in depth, the values can never drift apart.
 * - `maxTouchPoints` is 0 for desktop profiles (clientHints.mobile ===
 *   false) and 5 for mobile ones, like the calibrated hardware.
 * - `oscpu` is NOT defined at all: Chrome has no such property (it is
 *   Firefox-only), and even an always-undefined getter flips
 *   'oscpu' in navigator from false to true - a homemade detector.
 *   Native absence is reproduced by doing nothing.
 * - Every installed getter / constructor wrapper is registered in the
 *   toString-mask registry (window.__MIRAGE_RNG__.track) so that
 *   Function.prototype.toString reports it as native code; the mask
 *   itself is installed once, by webrtc.js (the last module).
 *
 * Worker spoofing - what is covered and what is not (by design):
 * - Blob URL of a Blob constructed by page script: the worker is rebuilt
 *   from [snippet, ...originalParts] with the same type. Works for
 *   classic AND module workers (the snippet is plain statement code that
 *   runs before any import statement of the original source).
 * - data: URL: the payload is decoded (percent or base64) and prepended
 *   the same way; classic workers fall back to an importScripts bootstrap
 *   if decoding fails.
 * - Classic http(s)/relative script URL: the worker is started from a
 *   bootstrap blob running "snippet + importScripts(<absolutized url>)".
 * - Module workers from an EXTERNAL http(s) URL cannot be patched
 *   (importScripts does not exist in module scope): constructed as-is,
 *   the legend does NOT apply inside them. Known limit.
 * - Blob URLs of Blobs this page did not construct (e.g. from
 *   response.blob()), workers nested inside a worker (the snippet patches
 *   only WorkerNavigator, not the worker-scope Worker constructor) and
 *   ServiceWorker registrations are out of reach: honest construction.
 * - Inside a bootstrapped http(s) worker, self.location is the bootstrap
 *   blob URL, not the original script URL (importScripts loads the
 *   original absolute URL). Scripts reading self.location see the blob.
 * - Bootstrap URLs are cached per original URL string, so two
 *   SharedWorker(sameUrl) still share ONE shared instance, like native
 *   Chrome (shared workers are keyed by script URL + name).
 * - Any failure path (unknown blob URL, cross-realm Blob, malformed
 *   data: URL, missing baseURI, revoked URL) falls back to the original
 *   constructor silently: this module never blocks worker construction
 *   and never throws on its own.
 */
;(function () {
    'use strict';
    try {
        var MARK = 'navigator';

        var done = window.__MIRAGE_DONE__;
        if (!done || typeof done !== 'object') {
            done = {};
            window.__MIRAGE_DONE__ = done;
        }
        if (done[MARK]) return;

        // Marked BEFORE patching: the constructor swaps done by time.js and
        // webrtc.js are not double-run-safe, so a re-injection of this
        // bundle into the same document must be a hard no-op.
        done[MARK] = true;

        var fp = window.__MIRAGE_FP__;
        if (!fp || typeof fp !== 'object') return;

        var NAV_PROTO = window.Navigator && window.Navigator.prototype;
        if (!NAV_PROTO) return;

        // One frozen languages array, cached in the closure ([SameObject]
        // plus FrozenArray semantics of the native accessor).
        var languagesFrozen = null;
        if (Object.prototype.toString.call(fp.languages) === '[object Array]') {
            var langs = [];
            for (var i = 0; i < fp.languages.length; i++) {
                if (typeof fp.languages[i] === 'string') langs.push(fp.languages[i]);
            }
            if (langs.length > 0) {
                try { Object.freeze(langs); } catch (e) { /* best effort */ }
                languagesFrozen = langs;
            }
        }

        // Desktop profiles (clientHints.mobile === false) report 0 touch
        // points; mobile hardware reports at least 5.
        var isMobile = !!(fp.clientHints && fp.clientHints.mobile === true);
        var touchPoints = isMobile ? 5 : 0;

        // Native-looking wrapper factory (shared helper from rng.js).
        var makeNative = (function () {
            try {
                var r = window.__MIRAGE_RNG__;
                if (r && typeof r.makeNative === 'function') return r.makeNative;
            } catch (e) { /* fall back below */ }
            return function (name, length, impl) {
                var f = function () {
                    var a = [];
                    for (var i = 0; i < arguments.length; i++) a.push(arguments[i]);
                    return impl(this, a);
                };
                try { Object.defineProperty(f, 'name', { value: name, configurable: true }); } catch (e) { }
                return f;
            };
        })();

        var NAV_CTOR = window.Navigator;

        // Register a function in the toString-mask registry (installed by
        // webrtc.js). Best effort: the spoof must never depend on it.
        function track(fn, name) {
            try {
                var r = window.__MIRAGE_RNG__;
                if (r && typeof r.track === 'function') r.track(fn, name);
            } catch (e) {
                // Mask stays incomplete, nothing else breaks.
            }
        }

        // Replace one prototype accessor, matching the native WebIDL shape
        // (enumerable, configurable, no setter). The getter is a
        // native-looking wrapper with the native brand check: a real native
        // Navigator getter throws "Illegal invocation" when read off the
        // prototype itself (detectors rely on that), so this one does too.
        // Own try/catch per field.
        function defineGetter(name, valueFn) {
            try {
                var getter = makeNative(name, 0, function (thisArg) {
                    if (thisArg === NAV_PROTO || !(thisArg instanceof NAV_CTOR)) {
                        throw new TypeError('Illegal invocation');
                    }
                    return valueFn();
                });
                track(getter, name);
                Object.defineProperty(NAV_PROTO, name, {
                    get: getter,
                    set: undefined,
                    enumerable: true,
                    configurable: true
                });
            } catch (e) {
                // Field stays native, the others still get installed.
            }
        }

        if (typeof fp.platform === 'string') {
            defineGetter('platform', function platform() { return fp.platform; });
        }

        if (typeof fp.hardwareConcurrency === 'number') {
            defineGetter('hardwareConcurrency', function hardwareConcurrency() { return fp.hardwareConcurrency; });
        }

        if (typeof fp.deviceMemory === 'number') {
            defineGetter('deviceMemory', function deviceMemory() { return fp.deviceMemory; });
        }

        if (languagesFrozen) {
            defineGetter('languages', function languages() { return languagesFrozen; });
            defineGetter('language', function language() { return languagesFrozen[0]; });
        }

        if (typeof fp.ua === 'string') {
            defineGetter('userAgent', function userAgent() { return fp.ua; });
        }

        defineGetter('webdriver', function webdriver() { return false; });

        defineGetter('maxTouchPoints', function maxTouchPoints() { return touchPoints; });

        defineGetter('vendor', function vendor() { return 'Google Inc.'; });

        // NOTE: no `oscpu` here, not even as an undefined getter - see the
        // header. Chrome has no such property at all.

        // Speech-synthesis voices (definition; invoked right below). The OS
        // voice list is a hard device tell: a fr-FR / Windows legend that
        // exposes "Microsoft Irina - Russian" (or any host-language voice)
        // leaks the real machine, and a lone non-legend default voice makes
        // detectors flag the timezone/locale. getVoices() is replaced with a
        // deterministic, legend-consistent catalog that is stable between
        // calls and never falls back to the native list - even while the
        // native list is still loading (empty).
        function installVoicesSpoof() {
            var r = window.__MIRAGE_RNG__;
            var SS = window.SpeechSynthesis;
            if (typeof SS !== 'function' || !SS.prototype) return;
            var d = Object.getOwnPropertyDescriptor(SS.prototype, 'getVoices');
            if (!d || typeof d.value !== 'function' || d.configurable === false) return;
            var origGetVoices = d.value;

            // Deterministic pick seed (pure function of the profile seeds).
            var vseed = 0;
            if (typeof fp.canvasSeed === 'number') vseed = (vseed ^ (fp.canvasSeed | 0)) | 0;
            if (typeof fp.audioSeed === 'number') vseed = (vseed ^ (fp.audioSeed | 0)) | 0;
            function pick(i, max) {
                try {
                    if (r && typeof r.int === 'function') return r.int(vseed, i, 0, max);
                } catch (e) { }
                return 0;
            }

            // Legend languages (primary first).
            var langs = [];
            if (Object.prototype.toString.call(fp.languages) === '[object Array]') {
                for (var i = 0; i < fp.languages.length && i < 4; i++) {
                    if (typeof fp.languages[i] === 'string' && fp.languages[i]) langs.push(fp.languages[i]);
                }
            }
            if (langs.length === 0) langs.push('en-US');

            var all = [];
            var seen = {};
            function push(name, lang, localService, isDefault, voiceURI) {
                if (typeof name !== 'string' || !name) return;
                var uri = (typeof voiceURI === 'string' && voiceURI) ? voiceURI : name;
                if (Object.prototype.hasOwnProperty.call(seen, uri)) return;
                seen[uri] = 1;
                all.push({
                    name: name,
                    lang: lang,
                    localService: !!localService,
                    default: !!isDefault,
                    voiceURI: uri
                });
            }

            function primaryOf(tag) {
                var s = String(tag).toLowerCase();
                var cut = s.indexOf('-');
                if (cut < 0) cut = s.indexOf('_');
                return cut < 0 ? s : s.slice(0, cut);
            }
            function normTag(tag) {
                var s = String(tag).replace('_', '-');
                return s;
            }

            // Per-primary catalogs: `google` is Chrome's built-in remote voice,
            // `local` are the OS SAPI voices (localService), `lang` the voice
            // language a real Windows install would tag them with.
            var CATALOG = {
                en: { google: 'Google US English', lang: 'en-US', local: ['Microsoft David - English (United States)', 'Microsoft Zira - English (United States)'] },
                fr: { google: 'Google français', lang: 'fr-FR', local: ['Microsoft Hortense - French (France)', 'Microsoft Paul - French (France)'] },
                de: { google: 'Google Deutsch', lang: 'de-DE', local: ['Microsoft Hedda - German (Germany)', 'Microsoft Katja - German (Germany)'] },
                es: { google: 'Google español', lang: 'es-ES', local: ['Microsoft Helena - Spanish (Spain)', 'Microsoft Laura - Spanish (Spain)'] },
                it: { google: 'Google italiano', lang: 'it-IT', local: ['Microsoft Elsa - Italian (Italy)', 'Microsoft Cosimo - Italian (Italy)'] },
                nl: { google: 'Google Nederlands', lang: 'nl-NL', local: ['Microsoft Hanna - Dutch (Netherlands)', 'Microsoft Frank - Dutch (Netherlands)'] },
                pl: { google: 'Google polski', lang: 'pl-PL', local: ['Microsoft Paulina - Polish (Poland)', 'Microsoft Marek - Polish (Poland)'] },
                sv: { google: 'Google svenska', lang: 'sv-SE', local: ['Microsoft Hedvig - Swedish (Sweden)'] },
                ru: { google: 'Google русский', lang: 'ru-RU', local: ['Microsoft Irina - Russian (Russia)', 'Microsoft Pavel - Russian (Russia)'] },
                tr: { google: 'Google Türkçe', lang: 'tr-TR', local: ['Microsoft Tolga - Turkish (Turkey)'] },
                pt: { google: 'Google português do Brasil', lang: 'pt-BR', local: ['Microsoft Maria - Portuguese (Brazil)', 'Microsoft Daniel - Portuguese (Brazil)'] },
                ja: { google: 'Google 日本語', lang: 'ja-JP', local: ['Microsoft Haruka - Japanese (Japan)', 'Microsoft Ichiro - Japanese (Japan)'] },
                ko: { google: 'Google 한국의', lang: 'ko-KR', local: ['Microsoft Heami - Korean (Korea)'] },
                zh: { google: 'Google 普通话（中国大陆）', lang: 'zh-CN', local: ['Microsoft Huihui - Chinese (Simplified, PRC)', 'Microsoft Yaoyao - Chinese (Simplified, PRC)'] }
            };
            // Region variants whose OS voice name embeds the region.
            var REGION = {
                'fr-ca': { lang: 'fr-CA', local: ['Microsoft Sylvie - French (Canada)', 'Microsoft Claude - French (Canada)'] },
                'en-gb': { lang: 'en-GB', local: ['Microsoft Hazel - English (United Kingdom)', 'Microsoft George - English (United Kingdom)'] },
                'es-mx': { lang: 'es-MX', local: ['Microsoft Sabina - Spanish (Mexico)', 'Microsoft Raul - Spanish (Mexico)'] }
            };

            var primary = primaryOf(langs[0]);
            var fullTag = normTag(langs[0]);
            var region = REGION[fullTag.toLowerCase()];
            var cat = CATALOG[primary] || { google: 'Google ' + primary, lang: fullTag, local: [] };
            var voiceLang = region ? region.lang : (cat.lang || fullTag);
            var localNames = region ? region.local : cat.local;

            // A deterministic subset (1..N) of the OS voices; the first is the
            // default - exactly one default LOCAL voice, which is what real
            // Chrome/Windows reports and what creepjs compares to the locale.
            var nLocal = localNames.length ? (1 + pick(1, localNames.length)) : 0;
            if (nLocal > localNames.length) nLocal = localNames.length;
            for (var li = 0; li < nLocal; li++) {
                push(localNames[li], voiceLang, true, li === 0, localNames[li]);
            }
            // Chrome's built-in remote voice for the legend language.
            push(cat.google, voiceLang, false, false, cat.google);

            // Base English fallback (the legend lists an 'en' tail; keep the
            // set plausible by carrying the usual en remote + one local voice).
            if (primary !== 'en') {
                var enTag = 'en-US';
                for (var e = 0; e < langs.length; e++) {
                    if (primaryOf(langs[e]) === 'en') { enTag = normTag(langs[e]); break; }
                }
                push('Google US English', enTag, false, false, 'Google US English');
                push('Microsoft David - English (United States)', 'en-US', true, false, 'Microsoft David - English (United States)');
            }

            // Native getVoices hands out a fresh Array of the same voice
            // objects on every call; reproduce that (stable contents).
            var getVoices = makeNative('getVoices', origGetVoices.length, function (thisArg, args) {
                return all.slice();
            });
            track(getVoices, 'getVoices');
            try {
                Object.defineProperty(SS.prototype, 'getVoices', {
                    value: getVoices,
                    writable: d.writable,
                    enumerable: d.enumerable,
                    configurable: d.configurable
                });
            } catch (e) { /* stay native on this method */ }

            // onvoiceschanged: keep the page's handler reachable through the
            // native event machinery (per instance) so a late native
            // `voiceschanged` never hands the page a half-loaded list. Same
            // shape as webrtc.js onicecandidate.
            try {
                var dOn = Object.getOwnPropertyDescriptor(SS.prototype, 'onvoiceschanged');
                if (dOn && typeof dOn.set === 'function' && typeof dOn.get === 'function' && dOn.configurable !== false) {
                    var handlers = new WeakMap();
                    var onGet = makeNative('onvoiceschanged', 0, function (thisArg) {
                        try { return handlers.get(thisArg) || null; } catch (e) { return null; }
                    });
                    var onSet = makeNative('onvoiceschanged', 1, function (thisArg, args) {
                        var handler = args[0];
                        if (typeof handler === 'function') {
                            handlers.set(thisArg, handler);
                            dOn.set.call(thisArg, function (event) {
                                return handler.call(this, event);
                            });
                        } else {
                            try { handlers.delete(thisArg); } catch (e) { }
                            dOn.set.call(thisArg, handler);
                        }
                    });
                    track(onGet, 'onvoiceschanged');
                    track(onSet, 'onvoiceschanged');
                    Object.defineProperty(SS.prototype, 'onvoiceschanged', {
                        get: onGet,
                        set: onSet,
                        enumerable: dOn.enumerable,
                        configurable: dOn.configurable
                    });
                }
            } catch (e) { /* native accessor stays */ }
        }

        // Worker-scope spoofing (definition; invoked right below).
        function installWorkerSpoof() {
            // Self-contained bootstrap prepended to every worker we can
            // rewrite. Inlined config: hardwareConcurrency, deviceMemory,
            // languages, language, platform, userAgent. Runs in the worker
            // realm: no window, only self / WorkerNavigator.
            function buildWorkerSnippet() {
                var cfg = {};
                if (typeof fp.hardwareConcurrency === 'number') cfg.hardwareConcurrency = fp.hardwareConcurrency;
                if (typeof fp.deviceMemory === 'number') cfg.deviceMemory = fp.deviceMemory;
                if (languagesFrozen) {
                    cfg.languages = languagesFrozen;
                    cfg.language = languagesFrozen[0];
                }
                if (typeof fp.platform === 'string') cfg.platform = fp.platform;
                if (typeof fp.ua === 'string') cfg.userAgent = fp.ua;
                var count = 0;
                for (var k in cfg) {
                    if (Object.prototype.hasOwnProperty.call(cfg, k)) count++;
                }
                if (count === 0) return '';
                var json;
                try { json = JSON.stringify(cfg); } catch (e) { return ''; }
                return [
                    ';(function () {',
                    "    'use strict';",
                    '    try {',
                    '        var cfg = ' + json + ';',
                    "        var P = (typeof WorkerNavigator !== 'undefined' && WorkerNavigator && WorkerNavigator.prototype) ? WorkerNavigator.prototype : null;",
                    '        if (P) {',
                    '            var langs = cfg.languages;',
                    '            if (langs) { try { Object.freeze(langs); } catch (e0) { } }',
                    '            var tracked = new Map();',
                    '            function def(n, g) {',
                    '                try {',
                    '                    Object.defineProperty(P, n, { get: g, set: undefined, enumerable: true, configurable: true });',
                    '                    tracked.set(g, n);',
                    '                } catch (e1) { }',
                    '            }',
                    '            function has(n) {',
                    '                try { return (n in P); } catch (e2) { return false; }',
                    '            }',
                    "            if (typeof cfg.hardwareConcurrency === 'number' && has('hardwareConcurrency')) {",
                    '                def(\'hardwareConcurrency\', function () { return cfg.hardwareConcurrency; });',
                    '            }',
                    "            if (typeof cfg.deviceMemory === 'number' && has('deviceMemory')) {",
                    '                def(\'deviceMemory\', function () { return cfg.deviceMemory; });',
                    '            }',
                    "            if (langs && has('languages')) {",
                    '                def(\'languages\', function () { return langs; });',
                    '            }',
                    "            if (typeof cfg.language === 'string' && has('language')) {",
                    '                def(\'language\', function () { return cfg.language; });',
                    '            }',
                    "            if (typeof cfg.platform === 'string' && has('platform')) {",
                    '                def(\'platform\', function () { return cfg.platform; });',
                    '            }',
                    "            if (typeof cfg.userAgent === 'string' && has('userAgent')) {",
                    '                def(\'userAgent\', function () { return cfg.userAgent; });',
                    '            }',
                    '            var ots = Function.prototype.toString;',
                    '            function ts() {',
                    '                try {',
                    "                    if (this === ts) { return 'function toString() { [native code] }'; }",
                    '                    var n = tracked.get(this);',
                    "                    if (typeof n === 'string') { return 'function ' + n + '() { [native code] }'; }",
                    '                } catch (e3) { }',
                    '                return Reflect.apply(ots, this, arguments);',
                    '            }',
                    '            try {',
                    "                Object.defineProperty(Function.prototype, 'toString', { value: ts, writable: true, enumerable: false, configurable: true });",
                    '            } catch (e4) { }',
                    '        }',
                    '    } catch (e5) { }',
                    '})();'
                ].join('\n');
            }

            var SNIPPET = buildWorkerSnippet();
            if (!SNIPPET) return; // nothing to spoof in workers

            var OrigBlob = window.Blob;
            var URL_OBJ = window.URL;
            var OrigURLCreate = (URL_OBJ && typeof URL_OBJ.createObjectURL === 'function') ? URL_OBJ.createObjectURL : null;
            var OrigURLRevoke = (URL_OBJ && typeof URL_OBJ.revokeObjectURL === 'function') ? URL_OBJ.revokeObjectURL : null;
            var OrigWorker = window.Worker;
            var OrigSharedWorker = window.SharedWorker;

            // Blob -> {parts: [...], type: '...'} (parts snapshotted
            // synchronously at construction, while they are still readable).
            var blobPartsMap = new WeakMap();
            // blob URL string -> Blob (recorded by our createObjectURL wrap).
            var blobUrlMap = new Map();
            // original worker URL -> bootstrap blob URL. Keeps the identity
            // semantics of SharedWorker: same input URL -> same bootstrap
            // URL -> one shared instance, like native Chrome.
            var bootUrlCache = new Map();

            function isHostBlob(obj) {
                try {
                    return typeof OrigBlob === 'function' && (obj instanceof OrigBlob);
                } catch (e) {
                    return false;
                }
            }

            // Decode the payload of a data: URL (percent-encoded or base64)
            // into script source text; null when it cannot be decoded.
            function decodeDataUrlScript(u) {
                var comma = u.indexOf(',');
                if (comma < 5) return null; // no 'data:' prefix payload
                var meta = u.slice(5, comma);
                var payload = u.slice(comma + 1);
                if (/;base64$/i.test(meta)) {
                    try {
                        var b64 = payload.replace(/-/g, '+').replace(/_/g, '/');
                        while (b64.length % 4 !== 0) b64 += '=';
                        var bin = atob(b64);
                        var bytes = new Uint8Array(bin.length);
                        for (var j = 0; j < bin.length; j++) bytes[j] = bin.charCodeAt(j) & 0xFF;
                        if (typeof TextDecoder === 'function') {
                            return new TextDecoder('utf-8').decode(bytes);
                        }
                        var raw = '';
                        for (var j2 = 0; j2 < bin.length; j2++) raw += String.fromCharCode(bytes[j2]);
                        return decodeURIComponent(escape(raw));
                    } catch (e) {
                        return null;
                    }
                }
                try { return decodeURIComponent(payload); } catch (e2) { return payload; }
            }

            // Build a bootstrap blob URL for this worker source, or null
            // when the worker must be constructed honestly.
            function buildBootstrapUrl(url, options) {
                if (typeof url !== 'string' || !url) return null;
                var isModule = !!(options && typeof options === 'object' && options.type === 'module');

                var cached = bootUrlCache.get(url);
                if (cached) return cached;

                // 1) blob: URL of a Blob constructed through our Blob wrap:
                //    rebuild the Blob with the snippet prepended (same type).
                var b = blobUrlMap.get(url);
                if (b) {
                    var snap = null;
                    try { snap = blobPartsMap.get(b); } catch (e) { snap = null; }
                    if (!snap || Object.prototype.toString.call(snap.parts) !== '[object Array]') {
                        return null; // blob we cannot rebuild: honest worker
                    }
                    var first = snap.parts.length > 0 ? snap.parts[0] : '';
                    if (typeof first === 'string' && first.charAt(0) === '#') {
                        return null; // shebang first line: prepend breaks it
                    }
                    try {
                        var newParts = [SNIPPET + '\n'].concat(snap.parts);
                        var opts = snap.type ? { type: snap.type } : undefined;
                        var nb = new OrigBlob(newParts, opts);
                        var nu = OrigURLCreate.call(URL_OBJ, nb);
                        if (typeof nu === 'string' && nu) {
                            bootUrlCache.set(url, nu);
                            return nu;
                        }
                    } catch (e2) { /* fall through to honest */ }
                    return null;
                }

                // 2) data: URL: decode the payload and prepend the snippet
                //    (works for classic and module workers alike). Classic
                //    workers fall back to an importScripts bootstrap when
                //    the payload cannot be decoded.
                if (/^data:/i.test(url)) {
                    var src = decodeDataUrlScript(url);
                    if (src !== null && src.charAt(0) !== '#') {
                        try {
                            var db = new OrigBlob([SNIPPET + '\n' + src], { type: 'application/javascript' });
                            var du = OrigURLCreate.call(URL_OBJ, db);
                            if (typeof du === 'string' && du) {
                                bootUrlCache.set(url, du);
                                return du;
                            }
                        } catch (e3) { /* fall through */ }
                    }
                    if (isModule) return null; // module: no importScripts fallback
                    try {
                        var dbb = new OrigBlob(
                            [SNIPPET + '\nimportScripts(' + JSON.stringify(url) + ');\n'],
                            { type: 'application/javascript' }
                        );
                        var dbu = OrigURLCreate.call(URL_OBJ, dbb);
                        if (typeof dbu === 'string' && dbu) {
                            bootUrlCache.set(url, dbu);
                            return dbu;
                        }
                    } catch (e4) { /* fall through to the honest worker */ }
                    return null;
                }

                // 3) classic worker from a script URL (absolute http(s) or
                //    document-relative): bootstrap blob that runs the
                //    snippet and then importScripts(<absolutized url>).
                if (!isModule) {
                    var abs = null;
                    try {
                        if (/^[a-zA-Z][a-zA-Z0-9+.\-]*:/.test(url)) {
                            // Already has a scheme: only http(s) can be
                            // importScripts-ed from a same-origin blob
                            // worker; anything else is out of reach.
                            if (!/^https?:/i.test(url)) return null;
                            abs = url;
                        } else {
                            if (typeof document === 'undefined' || !document ||
                                typeof document.baseURI !== 'string' || !document.baseURI) {
                                return null; // cannot absolutize safely
                            }
                            if (typeof window.URL !== 'function') return null;
                            abs = new window.URL(url, document.baseURI).href;
                        }
                    } catch (e4) { return null; }
                    if (typeof abs !== 'string' || !abs) return null;
                    try {
                        var boot = new OrigBlob(
                            [SNIPPET + '\nimportScripts(' + JSON.stringify(abs) + ');\n'],
                            { type: 'application/javascript' }
                        );
                        var bu = OrigURLCreate.call(URL_OBJ, boot);
                        if (typeof bu === 'string' && bu) {
                            bootUrlCache.set(url, bu);
                            return bu;
                        }
                    } catch (e5) { /* fall through */ }
                }
                return null;
            }

            // ---- Blob constructor wrap: snapshot parts synchronously ------
            if (typeof OrigBlob === 'function') {
                var BlobCtor = function Blob(parts, options) {
                    if (!new.target) {
                        // Native Blob is a class: calling without `new`
                        // throws the same TypeError.
                        throw new TypeError("Failed to construct 'Blob': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
                    }
                    var blob = Reflect.construct(OrigBlob, arguments, new.target || BlobCtor);
                    try {
                        if (Object.prototype.toString.call(parts) === '[object Array]') {
                            var type = '';
                            if (options && typeof options === 'object' && typeof options.type === 'string') {
                                type = options.type;
                            }
                            blobPartsMap.set(blob, { parts: parts.slice(), type: type });
                        }
                    } catch (e) {
                        // Untracked blob: a worker from it stays honest.
                    }
                    return blob;
                };
                try { BlobCtor.prototype = OrigBlob.prototype; } catch (e) { /* cosmetic */ }
                try {
                    Object.defineProperty(BlobCtor, 'length', { value: 0, configurable: true });
                } catch (e) { /* cosmetic */ }
                try {
                    Object.defineProperty(OrigBlob.prototype, 'constructor', {
                        value: BlobCtor,
                        writable: true,
                        enumerable: false,
                        configurable: true
                    });
                } catch (e) { /* cosmetic */ }
                track(BlobCtor, 'Blob');
                window.Blob = BlobCtor;
            }

            // ---- URL.createObjectURL / revokeObjectURL wraps --------------
            if (OrigURLCreate) {
                var createObjectURL = makeNative('createObjectURL', OrigURLCreate.length, function (thisArg, args) {
                    var obj = args[0];
                    var url = OrigURLCreate.apply(URL_OBJ, args);
                    try {
                        if (typeof url === 'string' && url && isHostBlob(obj)) {
                            blobUrlMap.set(url, obj);
                        }
                    } catch (e) {
                        // Recording failed: a worker from this URL stays honest.
                    }
                    return url;
                });
                try {
                    Object.defineProperty(URL_OBJ, 'createObjectURL', {
                        value: createObjectURL,
                        writable: true,
                        enumerable: true,
                        configurable: true
                    });
                    track(createObjectURL, 'createObjectURL');
                } catch (e) { /* stay native */ }

                if (OrigURLRevoke) {
                    var revokeObjectURL = makeNative('revokeObjectURL', OrigURLRevoke.length, function (thisArg, args) {
                        var url = args[0];
                        try {
                            // A revoked URL must stop working for Worker()
                            // too: forget both mappings, exactly like the
                            // native URL registry does.
                            blobUrlMap.delete(url);
                            bootUrlCache.delete(url);
                        } catch (e) { /* cleanup is best effort */ }
                        return OrigURLRevoke.apply(URL_OBJ, args);
                    });
                    try {
                        Object.defineProperty(URL_OBJ, 'revokeObjectURL', {
                            value: revokeObjectURL,
                            writable: true,
                            enumerable: true,
                            configurable: true
                        });
                        track(revokeObjectURL, 'revokeObjectURL');
                    } catch (e) { /* stay native */ }
                }
            }

            // ---- Worker constructor wrap ------------------------------------
            if (typeof OrigWorker === 'function') {
                var WorkerCtor = function Worker(scriptURL, options) {
                    if (!new.target) {
                        throw new TypeError("Failed to construct 'Worker': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
                    }
                    var patched = null;
                    try { patched = buildBootstrapUrl(scriptURL, options); } catch (e) { patched = null; }
                    if (typeof patched === 'string' && patched) {
                        try {
                            // Options (name/credentials/type/...) pass through
                            // untouched; module blob workers keep working
                            // because the snippet is plain pre-import code.
                            return Reflect.construct(OrigWorker, [patched, options], new.target || WorkerCtor);
                        } catch (e2) {
                            // Bootstrap construction failed: fall through to
                            // the honest worker below.
                        }
                    }
                    return Reflect.construct(OrigWorker, arguments, new.target || WorkerCtor);
                };
                try { WorkerCtor.prototype = OrigWorker.prototype; } catch (e) { /* cosmetic */ }
                try {
                    Object.defineProperty(WorkerCtor, 'length', { value: 1, configurable: true });
                } catch (e) { /* cosmetic */ }
                try {
                    Object.defineProperty(OrigWorker.prototype, 'constructor', {
                        value: WorkerCtor,
                        writable: true,
                        enumerable: false,
                        configurable: true
                    });
                } catch (e) { /* cosmetic */ }
                track(WorkerCtor, 'Worker');
                window.Worker = WorkerCtor;
            }

            // ---- SharedWorker constructor wrap (best effort) ---------------
            if (typeof OrigSharedWorker === 'function') {
                var SharedWorkerCtor = function SharedWorker(scriptURL, options) {
                    if (!new.target) {
                        throw new TypeError("Failed to construct 'SharedWorker': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
                    }
                    // options may be a plain name string; only object options
                    // can carry type:'module'.
                    var opts = (options && typeof options === 'object') ? options : null;
                    var patched = null;
                    try { patched = buildBootstrapUrl(scriptURL, opts); } catch (e) { patched = null; }
                    if (typeof patched === 'string' && patched) {
                        try {
                            return Reflect.construct(OrigSharedWorker, [patched, options], new.target || SharedWorkerCtor);
                        } catch (e2) {
                            // fall through to the honest worker below
                        }
                    }
                    return Reflect.construct(OrigSharedWorker, arguments, new.target || SharedWorkerCtor);
                };
                try { SharedWorkerCtor.prototype = OrigSharedWorker.prototype; } catch (e) { /* cosmetic */ }
                try {
                    Object.defineProperty(SharedWorkerCtor, 'length', { value: 1, configurable: true });
                } catch (e) { /* cosmetic */ }
                try {
                    Object.defineProperty(OrigSharedWorker.prototype, 'constructor', {
                        value: SharedWorkerCtor,
                        writable: true,
                        enumerable: false,
                        configurable: true
                    });
                } catch (e) { /* cosmetic */ }
                track(SharedWorkerCtor, 'SharedWorker');
                window.SharedWorker = SharedWorkerCtor;
            }
        }

        // ---- Speech-synthesis voices: install -------------------------------
        try {
            installVoicesSpoof();
        } catch (e) {
            // Voice spoofing is best effort; the getters above already work.
        }

        // ---- Worker-scope spoofing: install ---------------------------------
        try {
            installWorkerSpoof();
        } catch (e) {
            // Worker patching is best effort; the getters above already work.
        }
    } catch (e) {
        // Contract: never leak anything to the page.
    }
})();

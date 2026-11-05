/*
 * Mirage inject layer - worker snippet (NOT a page module).
 *
 * This file is NOT part of INJECT_MODULES. The launcher never concatenates it
 * into the page bundle; it is injected ONLY into worker-scope CDP targets
 * (`worker` / `service_worker` / `shared_worker`) as the source of a
 * `Runtime.evaluate` that runs while the worker is held paused by
 * `Target.setAutoAttach { waitForDebuggerOnStart }`, i.e. before the worker's
 * own script starts. That is the only place a worker (and, crucially, a
 * service worker, whose global scope page script cannot reach) can be patched
 * from the CDP side.
 *
 * Configuration arrives in `self.__MIRAGE_FP__` (the launcher prepends
 * `self.__MIRAGE_FP__ = <fingerprint JSON>;` ahead of this file). Worker scope
 * has no `window`, no `Navigator`, no `document`: only `self`, `WorkerNavigator`
 * and the worker-exposed WebIDL surfaces. Everything here is therefore
 * self-contained - including a private copy of the deterministic PRNG from
 * `rng.js`, because the page global `__MIRAGE_RNG__` does not exist in a
 * worker.
 *
 * What is spoofed (kept in sync with the page modules):
 *   - `WorkerNavigator.prototype`: hardwareConcurrency, deviceMemory, platform,
 *     userAgent, languages / language (one frozen array, [SameObject]).
 *   - `userAgentData` (brands / mobile / platform, getHighEntropyValues,
 *     toJSON), [SecureContext] and [SameObject], exactly like clienthints.js.
 *   - WebGL `getParameter` on WebGLRenderingContext / WebGL2RenderingContext
 *     prototypes (UNMASKED vendor/renderer, masked strings, MAX_* limits) and
 *     `readPixels` LSB noise - the worker `OffscreenCanvas.getContext('webgl')`
 *     would otherwise leak the real GPU string, which differs from the legend.
 *   - OffscreenCanvas 2D `getImageData` LSB noise plus `convertToBlob`
 *     routed through the same noised pixel path (see the note below).
 *   - A `Function.prototype.toString` mask so every installed wrapper answers
 *     as native code; the registry is local to this snippet.
 *
 * Contract: IIFE, 'use strict', every failure swallowed, zero console output,
 * no `window` reference anywhere (ES2017). Runs once per worker at start.
 *
 * convertToBlob note: a JS layer cannot edit already-encoded PNG bytes, so the
 * wrapper decodes the produced blob (createImageBitmap), draws it onto a
 * scratch OffscreenCanvas, reads it back through our noised `getImageData`,
 * and re-encodes with the ORIGINAL convertToBlob. Any failure (decode, size,
 * unsupported type) falls back to the untouched original blob: the worker is
 * never broken by this patch.
 */
;(function () {
    'use strict';
    try {
        var G = (typeof self !== 'undefined') ? self : null;
        if (!G) return;

        var fp = G.__MIRAGE_FP__;
        if (!fp || typeof fp !== 'object') return;

        // ---- deterministic PRNG (verbatim copy of inject/rng.js) ----------
        function hash32(a, b, c) {
            var h = (a | 0) ^ 0x9E3779B9;
            h ^= h << 13;
            h ^= h >>> 17;
            h ^= h << 5;
            h = (h ^ (b | 0)) | 0;
            h = Math.imul(h, 0x85EBCA6B);
            h ^= h >>> 13;
            h = Math.imul(h, 0xC2B2AE35);
            h ^= h >>> 16;
            h = (h ^ (c | 0)) | 0;
            h = Math.imul(h, 0x27D4EB2F);
            h ^= h >>> 15;
            h = Math.imul(h, 0x165667B1);
            h ^= h >>> 13;
            return h >>> 0;
        }

        function mulberry32(a) {
            a = (a + 0x6D2B79F5) | 0;
            var t = Math.imul(a ^ (a >>> 15), 1 | a);
            t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
            return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
        }

        var INT_SALT = 0x5BD1E995;

        function noise(seed, x, y) {
            return mulberry32(hash32(seed, x, y));
        }

        function intn(seed, x, y, max) {
            var m = Math.floor(+max);
            if (!isFinite(m) || m <= 0) return 0;
            return Math.floor(mulberry32(hash32((seed | 0) ^ INT_SALT, x, y)) * m);
        }

        // ---- toString-mask registry (local to the worker realm) -----------
        // spoofed function -> display name; the mask below answers
        // "function <name>() { [native code] }" for exactly these functions.
        var tracked = new Map();

        function track(fn, name) {
            try {
                if (typeof fn === 'function') {
                    var n = (typeof name === 'string' && name) ? name : (fn.name || '');
                    tracked.set(fn, n);
                }
            } catch (e) {
                // Masking is best effort; never break the caller.
            }
            return fn;
        }

        function installToStringMask() {
            var ots = Function.prototype.toString;
            function ts() {
                try {
                    if (this === ts) return 'function toString() { [native code] }';
                    var n = tracked.get(this);
                    if (typeof n === 'string') return 'function ' + n + '() { [native code] }';
                } catch (e) {
                    // fall through to the native path
                }
                return Reflect.apply(ots, this, arguments);
            }
            try {
                Object.defineProperty(Function.prototype, 'toString', {
                    value: ts,
                    writable: true,
                    enumerable: false,
                    configurable: true
                });
            } catch (e) {
                // mask stays incomplete; nothing else breaks
            }
        }

        // ---- WorkerNavigator identity -------------------------------------
        var NAV_PROTO = null;
        try {
            if (typeof WorkerNavigator !== 'undefined' && WorkerNavigator && WorkerNavigator.prototype) {
                NAV_PROTO = WorkerNavigator.prototype;
            }
        } catch (e) {
            NAV_PROTO = null;
        }

        var languagesFrozen = null;
        if (Object.prototype.toString.call(fp.languages) === '[object Array]') {
            var langs = [];
            for (var li = 0; li < fp.languages.length; li++) {
                if (typeof fp.languages[li] === 'string') langs.push(fp.languages[li]);
            }
            if (langs.length > 0) {
                try { Object.freeze(langs); } catch (e) { /* best effort */ }
                languagesFrozen = langs;
            }
        }

        // Replace one prototype accessor, matching the native WebIDL shape
        // (enumerable, configurable, no setter). Own try/catch per field.
        function defineGetter(proto, name, getter) {
            try {
                track(getter, name);
                Object.defineProperty(proto, name, {
                    get: getter,
                    set: undefined,
                    enumerable: true,
                    configurable: true
                });
            } catch (e) {
                // Field stays native; the others still install.
            }
        }

        if (NAV_PROTO) {
            if (typeof fp.platform === 'string') {
                defineGetter(NAV_PROTO, 'platform', function platform() { return fp.platform; });
            }
            if (typeof fp.hardwareConcurrency === 'number') {
                defineGetter(NAV_PROTO, 'hardwareConcurrency', function hardwareConcurrency() { return fp.hardwareConcurrency; });
            }
            if (typeof fp.deviceMemory === 'number') {
                defineGetter(NAV_PROTO, 'deviceMemory', function deviceMemory() { return fp.deviceMemory; });
            }
            if (languagesFrozen) {
                defineGetter(NAV_PROTO, 'languages', function languages() { return languagesFrozen; });
                defineGetter(NAV_PROTO, 'language', function language() { return languagesFrozen[0]; });
            }
            if (typeof fp.ua === 'string') {
                defineGetter(NAV_PROTO, 'userAgent', function userAgent() { return fp.ua; });
            }
            // NOTE: WorkerNavigator has no `webdriver`/`vendor`/`maxTouchPoints`
            // in Chrome; defining them would invent properties that never exist
            // in a worker - a homemade tell. Nothing is added for them.
        }

        // ---- userAgentData ([SecureContext], [SameObject]) ---------------
        function installUserAgentData(proto) {
            // Gate on the native surface, not on `isSecureContext`: the worker
            // global's own `isSecureContext` proved unreliable (live creepjs run
            // kept leaking the real platformVersion when this branch bailed on
            // it). If the engine exposes `userAgentData` here at all, it is a
            // secure context by definition - and if it does not, we must not
            // invent it (matching native on insecure origins).
            var nativeDesc = null;
            try {
                nativeDesc = Object.getOwnPropertyDescriptor(proto, 'userAgentData');
            } catch (e) {
                nativeDesc = null;
            }
            if (!nativeDesc) return;

            var ch = fp.clientHints;
            if (!ch || typeof ch !== 'object') return;

            var uaDataIfaceProto = null;
            var uaDataIfaceCtor = null;
            try {
                if (typeof G.NavigatorUAData === 'function' && G.NavigatorUAData.prototype) {
                    uaDataIfaceCtor = G.NavigatorUAData;
                    uaDataIfaceProto = G.NavigatorUAData.prototype;
                }
            } catch (e) { /* engine without NavigatorUAData in worker scope */ }

            function isList(x) {
                return Object.prototype.toString.call(x) === '[object Array]';
            }

            function buildBrandList(versionOverride) {
                var out = [];
                if (isList(ch.brands)) {
                    for (var i = 0; i < ch.brands.length; i++) {
                        var b = ch.brands[i];
                        if (b && typeof b === 'object' &&
                            typeof b.brand === 'string' && typeof b.version === 'string') {
                            out.push({
                                brand: b.brand,
                                version: (typeof versionOverride === 'string') ? versionOverride : b.version
                            });
                        }
                    }
                }
                try { Object.freeze(out); } catch (e) { /* best effort */ }
                return out;
            }

            function getHighEntropyValues(hints) {
                return new Promise(function (resolve) {
                    var out = {};
                    try { out.brands = buildBrandList(null); } catch (e) { /* base stays incomplete */ }
                    out.mobile = ch.mobile === true;
                    if (typeof ch.platform === 'string') out.platform = ch.platform;
                    try {
                        var list = null;
                        if (isList(hints)) {
                            list = hints;
                        } else if (hints && typeof hints === 'object' && typeof hints.length === 'number') {
                            list = hints; // array-like sequence
                        }
                        if (list) {
                            for (var i = 0; i < list.length; i++) {
                                var key = list[i];
                                if (typeof key !== 'string') continue;
                                switch (key) {
                                    case 'architecture':
                                        if (typeof ch.architecture === 'string') out.architecture = ch.architecture;
                                        break;
                                    case 'bitness':
                                        if (typeof ch.bitness === 'string') out.bitness = ch.bitness;
                                        break;
                                    case 'mobile':
                                        out.mobile = ch.mobile === true;
                                        break;
                                    case 'model':
                                        out.model = '';
                                        break;
                                    case 'platformVersion':
                                        if (typeof ch.platformVersion === 'string') out.platformVersion = ch.platformVersion;
                                        break;
                                    case 'uaFullVersion':
                                        if (typeof ch.uaFullVersion === 'string') out.uaFullVersion = ch.uaFullVersion;
                                        break;
                                    case 'fullVersionList':
                                        out.fullVersionList = buildBrandList(
                                            (typeof ch.uaFullVersion === 'string') ? ch.uaFullVersion : null
                                        );
                                        break;
                                    case 'wow64':
                                        out.wow64 = false;
                                        break;
                                    case 'formFactors': {
                                        var ff = ['Desktop'];
                                        try { Object.freeze(ff); } catch (e2) { /* best effort */ }
                                        out.formFactors = ff;
                                        break;
                                    }
                                    default:
                                        break; // unknown hint: omitted, like native
                                }
                            }
                        }
                    } catch (e3) {
                        // Resolve with whatever was collected - never throw.
                    }
                    resolve(out);
                });
            }

            function toJSON() {
                var out = { brands: [], mobile: ch.mobile === true, platform: ch.platform };
                if (isList(ch.brands)) {
                    for (var i = 0; i < ch.brands.length; i++) {
                        var b = ch.brands[i];
                        if (b && typeof b === 'object' &&
                            typeof b.brand === 'string' && typeof b.version === 'string') {
                            out.brands.push({ brand: b.brand, version: b.version });
                        }
                    }
                }
                return out;
            }

            // Fake prototype between the instance and the real interface
            // prototype: instanceof keeps passing, members live here (native
            // instances have zero own properties).
            var uaDataProto = Object.create(uaDataIfaceProto || Object.prototype);
            if (uaDataIfaceCtor) {
                try {
                    Object.defineProperty(uaDataProto, 'constructor', {
                        value: uaDataIfaceCtor,
                        writable: true,
                        enumerable: false,
                        configurable: true
                    });
                } catch (e) { /* cosmetic */ }
            }

            var brandsFrozen = buildBrandList(null); // [SameObject]

            var brandsGetter = function brands() { return brandsFrozen; };
            var mobileGetter = function mobile() { return ch.mobile === true; };
            var platformGetter = function platform() { return ch.platform; };

            track(brandsGetter, 'brands');
            track(mobileGetter, 'mobile');
            track(platformGetter, 'platform');
            track(getHighEntropyValues, 'getHighEntropyValues');
            track(toJSON, 'toJSON');

            try {
                Object.defineProperty(uaDataProto, 'brands', {
                    get: brandsGetter, set: undefined, enumerable: true, configurable: true
                });
                Object.defineProperty(uaDataProto, 'mobile', {
                    get: mobileGetter, set: undefined, enumerable: true, configurable: true
                });
                Object.defineProperty(uaDataProto, 'platform', {
                    get: platformGetter, set: undefined, enumerable: true, configurable: true
                });
                Object.defineProperty(uaDataProto, 'getHighEntropyValues', {
                    value: getHighEntropyValues, writable: true, enumerable: true, configurable: true
                });
                Object.defineProperty(uaDataProto, 'toJSON', {
                    value: toJSON, writable: true, enumerable: true, configurable: true
                });
            } catch (e) {
                // A partially built prototype is still readable.
            }

            var uaDataInstance = Object.create(uaDataProto);
            var userAgentDataGetter = function userAgentData() { return uaDataInstance; };
            try {
                track(userAgentDataGetter, 'userAgentData');
                Object.defineProperty(proto, 'userAgentData', {
                    get: userAgentDataGetter,
                    set: undefined,
                    enumerable: true,
                    configurable: true
                });
            } catch (e) { /* stay native */ }
        }

        if (NAV_PROTO) {
            try { installUserAgentData(NAV_PROTO); } catch (e) { /* best effort */ }
        }

        // ---- shared descriptor-safe method patcher ------------------------
        function patchMethod(proto, prop, makeWrapper) {
            try {
                var d = Object.getOwnPropertyDescriptor(proto, prop);
                if (!d) return false;
                if (d.get !== undefined || d.set !== undefined) return false;
                if (typeof d.value !== 'function') return false;
                if (d.configurable === false) return false;
                var w = makeWrapper(d.value);
                if (typeof w !== 'function') return false;
                try {
                    Object.defineProperty(w, 'name', { value: prop, configurable: true });
                } catch (e2) { /* cosmetic only */ }
                track(w, prop);
                Object.defineProperty(proto, prop, {
                    value: w,
                    writable: d.writable,
                    enumerable: d.enumerable,
                    configurable: d.configurable
                });
                return true;
            } catch (e) {
                return false;
            }
        }

        // ---- WebGL getParameter + readPixels ------------------------------
        (function installWebgl() {
            var glCfg = (fp.webgl && typeof fp.webgl === 'object') ? fp.webgl : null;
            var seed = (typeof fp.canvasSeed === 'number') ? (fp.canvasSeed | 0) : null;
            if (!glCfg && seed === null) return;

            var MASKED_VENDOR = 'WebKit';
            var MASKED_RENDERER = 'WebKit WebGL';
            var VERSION_WGL2 = 'WebGL 2.0 (OpenGL ES 3.0 Chromium)';
            var VERSION_WGL1 = 'WebGL 1.0 (OpenGL ES 2.0 Chromium)';
            var GLSL_WGL2 = 'WebGL GLSL ES 3.00 (OpenGL ES GLSL ES 3.0 Chromium)';
            var GLSL_WGL1 = 'WebGL GLSL ES 1.00 (OpenGL ES GLSL ES 1.0 Chromium)';

            var UNMASKED_VENDOR_WEBGL = 0x9245;
            var UNMASKED_RENDERER_WEBGL = 0x9246;
            var GL_VENDOR = 0x1F00;
            var GL_RENDERER = 0x1F01;
            var GL_VERSION = 0x1F02;
            var GL_SHADING_LANGUAGE_VERSION = 0x8B8C;
            var GL_MAX_TEXTURE_SIZE = 0x0D33;
            var GL_MAX_VIEWPORT_DIMS = 0x0D3A;
            var GL_MAX_RENDERBUFFER_SIZE = 0x84E8;
            var GL_RGBA = 0x1908;
            var GL_UNSIGNED_BYTE = 0x1401;
            var READ_RATE = 0.02;

            function makeGetParameter(orig, ctor, isWebGL2) {
                return function (pname) {
                    try {
                        var p = pname >>> 0;
                        if (p === UNMASKED_VENDOR_WEBGL || p === UNMASKED_RENDERER_WEBGL ||
                            p === GL_VENDOR || p === GL_RENDERER || p === GL_VERSION ||
                            p === GL_SHADING_LANGUAGE_VERSION || p === GL_MAX_TEXTURE_SIZE ||
                            p === GL_MAX_RENDERBUFFER_SIZE || p === GL_MAX_VIEWPORT_DIMS) {
                            if (this instanceof ctor) {
                                if (p === UNMASKED_VENDOR_WEBGL) {
                                    if (glCfg && typeof glCfg.vendor === 'string' && glCfg.vendor) return glCfg.vendor;
                                } else if (p === UNMASKED_RENDERER_WEBGL) {
                                    if (glCfg && typeof glCfg.renderer === 'string' && glCfg.renderer) return glCfg.renderer;
                                } else if (p === GL_VENDOR) {
                                    return MASKED_VENDOR;
                                } else if (p === GL_RENDERER) {
                                    return MASKED_RENDERER;
                                } else if (p === GL_VERSION) {
                                    return isWebGL2 ? VERSION_WGL2 : VERSION_WGL1;
                                } else if (p === GL_SHADING_LANGUAGE_VERSION) {
                                    return isWebGL2 ? GLSL_WGL2 : GLSL_WGL1;
                                } else if (p === GL_MAX_TEXTURE_SIZE) {
                                    if (glCfg && typeof glCfg.maxTextureSize === 'number' && glCfg.maxTextureSize > 0) {
                                        return glCfg.maxTextureSize | 0;
                                    }
                                } else if (p === GL_MAX_RENDERBUFFER_SIZE) {
                                    if (glCfg && typeof glCfg.maxRenderbufferSize === 'number' && glCfg.maxRenderbufferSize > 0) {
                                        return glCfg.maxRenderbufferSize | 0;
                                    }
                                } else if (p === GL_MAX_VIEWPORT_DIMS) {
                                    if (glCfg && Object.prototype.toString.call(glCfg.maxViewportDims) === '[object Array]' &&
                                        glCfg.maxViewportDims.length >= 2) {
                                        return new Int32Array([glCfg.maxViewportDims[0] | 0, glCfg.maxViewportDims[1] | 0]);
                                    }
                                }
                            }
                        }
                    } catch (e) {
                        // fall through to the original call below
                    }
                    return orig.call(this, pname);
                };
            }

            function makeReadPixels(orig) {
                return function readPixels(x, y, width, height, format, type, dstData) {
                    orig.apply(this, arguments);
                    try {
                        if (seed === null) return;
                        if ((format >>> 0) !== GL_RGBA || (type >>> 0) !== GL_UNSIGNED_BYTE) return;
                        if (!dstData || typeof dstData !== 'object' || dstData.BYTES_PER_ELEMENT !== 1) return;
                        var dstOffset = arguments.length > 7 ? arguments[7] : 0;
                        var base = (typeof dstOffset === 'number' && dstOffset > 0) ? dstOffset : 0;
                        var total = (width * height) * 4;
                        var lim = dstData.length;
                        for (var i = 0; i < total; i += 4) {
                            var px = x + ((i >> 2) % width);
                            var py = y + (((i >> 2) / width) | 0);
                            if (noise(seed, px, py) < READ_RATE) {
                                var o = base + i;
                                var v0 = dstData[o], v1 = dstData[o + 1];
                                var v2 = dstData[o + 2], v3 = dstData[o + 3];
                                if ((v0 === 0 && v1 === 0 && v2 === 0 && v3 === 0) ||
                                    (v0 === 255 && v1 === 255 && v2 === 255 && v3 === 255)) {
                                    continue;
                                }
                                var di = o + intn(seed, px, py, 4);
                                if (di >= 0 && di < lim) {
                                    dstData[di] = dstData[di] ^ 1;
                                }
                            }
                        }
                    } catch (e) {
                        // The noise step must never break the read.
                    }
                };
            }

            function installOnPrototype(ctor, proto, isWebGL2) {
                patchMethod(proto, 'getParameter', function (orig) {
                    return makeGetParameter(orig, ctor, isWebGL2);
                });
                if (seed !== null) {
                    patchMethod(proto, 'readPixels', makeReadPixels);
                }
            }

            if (typeof WebGLRenderingContext !== 'undefined' && WebGLRenderingContext.prototype) {
                installOnPrototype(WebGLRenderingContext, WebGLRenderingContext.prototype, false);
            }
            if (typeof WebGL2RenderingContext !== 'undefined' && WebGL2RenderingContext.prototype) {
                installOnPrototype(WebGL2RenderingContext, WebGL2RenderingContext.prototype, true);
            }
        })();

        // ---- OffscreenCanvas 2D noise (getImageData + convertToBlob) ------
        (function installOffscreenCanvas() {
            var seed = (typeof fp.canvasSeed === 'number') ? (fp.canvasSeed | 0) : null;
            if (seed === null) return;
            var RATE = 0.03;

            function isVirginPixel(d, o) {
                var v0 = d[o], v1 = d[o + 1], v2 = d[o + 2], v3 = d[o + 3];
                return (v0 === 0 && v1 === 0 && v2 === 0 && v3 === 0) ||
                       (v0 === 255 && v1 === 255 && v2 === 255 && v3 === 255);
            }

            function makeGetImageData(orig) {
                return function getImageData(sx, sy, sw, sh) {
                    var img = orig.apply(this, arguments);
                    try {
                        if (img && img.data) {
                            var d = img.data;
                            var width = img.width | 0;
                            var height = img.height | 0;
                            for (var yy = 0; yy < height; yy++) {
                                var row = yy * width;
                                for (var xx = 0; xx < width; xx++) {
                                    var ax = sx + xx;
                                    var ay = sy + yy;
                                    if (noise(seed, ax, ay) < RATE) {
                                        var o = (row + xx) << 2;
                                        if (isVirginPixel(d, o)) continue;
                                        d[o + intn(seed, ax, ay, 4)] ^= 1;
                                    }
                                }
                            }
                        }
                    } catch (e) {
                        // The noise step must never break the read.
                    }
                    return img;
                };
            }

            if (typeof OffscreenCanvasRenderingContext2D !== 'undefined' && OffscreenCanvasRenderingContext2D.prototype) {
                patchMethod(OffscreenCanvasRenderingContext2D.prototype, 'getImageData', makeGetImageData);
            }

            // convertToBlob: decode -> draw on a scratch OffscreenCanvas ->
            // read back through the patched (noised) getImageData -> re-encode
            // with the ORIGINAL convertToBlob. Falls back to the original blob
            // on any failure.
            if (typeof OffscreenCanvas !== 'undefined' && OffscreenCanvas.prototype &&
                typeof createImageBitmap === 'function') {
                patchMethod(OffscreenCanvas.prototype, 'convertToBlob', function (orig) {
                    return function convertToBlob(options) {
                        var canvas = this;
                        var produced = orig.apply(canvas, arguments);
                        try {
                            if (!produced || typeof produced.then !== 'function') return produced;
                            return produced.then(function (blob) {
                                var type = (options && typeof options === 'object' && typeof options.type === 'string')
                                    ? options.type : '';
                                if (type && type.toLowerCase().indexOf('image/') !== 0) return blob;
                                return createImageBitmap(blob).then(function (bitmap) {
                                    try {
                                        var w = bitmap.width | 0;
                                        var h = bitmap.height | 0;
                                        if (w <= 0 || h <= 0) return blob;
                                        var scratch = new G.OffscreenCanvas(w, h);
                                        var ctx = scratch.getContext('2d');
                                        if (!ctx) return blob;
                                        ctx.drawImage(bitmap, 0, 0);
                                        var img = ctx.getImageData(0, 0, w, h);
                                        ctx.putImageData(img, 0, 0);
                                        return orig.call(scratch, options);
                                    } catch (e) {
                                        return blob;
                                    }
                                }, function () { return blob; });
                            }, function () { return produced; });
                        } catch (e) {
                            return produced;
                        }
                    };
                });
            }
        })();

        installToStringMask();
    } catch (e) {
        // Contract: never leak anything to the worker's own script.
    }
})();

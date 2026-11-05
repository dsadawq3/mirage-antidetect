/*
 * Mirage inject layer - module "clienthints" (see docs/CONTRACT.md).
 *
 * Spoofs navigator.userAgentData (User-Agent Client Hints). CDP has no
 * override for this surface, so the JS layer is the only way to align it
 * with the spoofed UA.
 *
 * [SecureContext]: native Chrome does not expose userAgentData at all on
 * insecure pages (plain http://, data: URLs, ...). This module reproduces
 * that exactly: when window.isSecureContext is false it defines NOTHING.
 *
 * Shape (matching the native 156 surface):
 * - Navigator.prototype.userAgentData is a [SameObject] getter: one cached
 *   instance per document, handed out on every access.
 * - The instance has ZERO own properties. brands / mobile / platform
 *   (getter accessors), getHighEntropyValues and toJSON live on a
 *   fake prototype object created with Object.create(NavigatorUAData.prototype)
 *   when the engine exposes the interface, so instanceof NavigatorUAData
 *   keeps passing; the fake prototype also mirrors the native
 *   non-enumerable 'constructor' own property. getOwnPropertyNames of the
 *   instance is empty, like the native one.
 * - brands is ONE frozen array of fresh {brand, version} objects, stable
 *   between accesses ([SameObject] FrozenArray semantics).
 * - getHighEntropyValues(hints) resolves with brands, mobile and platform
 *   ALWAYS present, plus the requested high-entropy hints (architecture,
 *   bitness, model ("", desktop), platformVersion, uaFullVersion,
 *   fullVersionList (the brands with every version replaced by
 *   uaFullVersion, per the CDP userAgentMetadata contract), wow64 (false)
 *   and formFactors (["Desktop"], frozen)). Unknown hint keys are
 *   silently omitted, like the native implementation; the promise never
 *   rejects.
 * - toJSON (on the fake prototype) returns a fresh
 *   {brands, mobile, platform} plain object, so
 *   JSON.stringify(navigator.userAgentData) works exactly like native
 *   (the native prototype toJSON would throw "Illegal invocation" on a
 *   receiver without native internals - it must stay shadowed).
 */
;(function () {
    'use strict';
    try {
        var MARK = 'clienthints';

        var done = window.__MIRAGE_DONE__;
        if (!done || typeof done !== 'object') {
            done = {};
            window.__MIRAGE_DONE__ = done;
        }
        if (done[MARK]) return;
        done[MARK] = true;

        // [SecureContext] attribute: on insecure pages native Chrome has
        // no userAgentData whatsoever - we must not define ours either.
        if (!window.isSecureContext) return;

        var fp = window.__MIRAGE_FP__;
        if (!fp || typeof fp !== 'object') return;

        var ch = fp.clientHints;
        if (!ch || typeof ch !== 'object') return;

        var NAV_PROTO = window.Navigator && window.Navigator.prototype;
        if (!NAV_PROTO) return;

        var UA_DATA_IFACE_PROTO = null;
        var UA_DATA_IFACE_CTOR = null;
        try {
            if (typeof window.NavigatorUAData === 'function' && window.NavigatorUAData.prototype) {
                UA_DATA_IFACE_CTOR = window.NavigatorUAData;
                UA_DATA_IFACE_PROTO = window.NavigatorUAData.prototype;
            }
        } catch (e) { /* engine without NavigatorUAData */ }

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

        function isList(x) {
            return Object.prototype.toString.call(x) === '[object Array]';
        }

        // Fresh frozen array of fresh brand objects; versionOverride (when
        // given) replaces every version - used for fullVersionList.
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
                // Per spec (and the native implementation) the result
                // ALWAYS carries the low-entropy base: brands, mobile,
                // platform - on top of whatever was requested.
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
                    // Resolve with whatever was collected - never throw,
                    // never reject.
                }
                resolve(out);
            });
        }

        // Dictionary semantics: a fresh plain object with fresh brand
        // entries on every call, exactly like the native toJSON.
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

        // ---- the fake prototype object ---------------------------------------
        // Sits between the instance and the real NavigatorUAData.prototype:
        // instanceof keeps passing, while all members live here (native
        // instances have zero own properties).
        var uaDataProto = Object.create(UA_DATA_IFACE_PROTO || Object.prototype);
        if (UA_DATA_IFACE_CTOR) {
            try {
                Object.defineProperty(uaDataProto, 'constructor', {
                    value: UA_DATA_IFACE_CTOR,
                    writable: true,
                    enumerable: false,
                    configurable: true
                });
            } catch (e) { /* cosmetic */ }
        }

        var brandsFrozen = buildBrandList(null); // [SameObject]

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

        var brandsGetter = makeNative('brands', 0, function () { return brandsFrozen; });
        var mobileGetter = makeNative('mobile', 0, function () { return ch.mobile === true; });
        var platformGetter = makeNative('platform', 0, function () { return ch.platform; });
        var getHighEntropyValuesFn = makeNative('getHighEntropyValues', getHighEntropyValues.length, function (thisArg, args) {
            return getHighEntropyValues.apply(thisArg, args);
        });
        var toJSONFn = makeNative('toJSON', toJSON.length, function (thisArg, args) {
            return toJSON.apply(thisArg, args);
        });

        track(brandsGetter, 'brands');
        track(mobileGetter, 'mobile');
        track(platformGetter, 'platform');
        track(getHighEntropyValuesFn, 'getHighEntropyValues');
        track(toJSONFn, 'toJSON');

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
                value: getHighEntropyValuesFn, writable: true, enumerable: true, configurable: true
            });
            Object.defineProperty(uaDataProto, 'toJSON', {
                value: toJSONFn, writable: true, enumerable: true, configurable: true
            });
        } catch (e) {
            // A partially built prototype is still readable.
        }

        // [SameObject]: the one cached instance, no own properties. The getter
        // carries the native brand check (throws off the prototype itself).
        var uaDataInstance = Object.create(uaDataProto);

        var userAgentDataGetter = makeNative('userAgentData', 0, function (thisArg) {
            if (thisArg === NAV_PROTO || !(thisArg instanceof NAV_CTOR)) {
                throw new TypeError('Illegal invocation');
            }
            return uaDataInstance;
        });
        try {
            track(userAgentDataGetter, 'userAgentData');
            Object.defineProperty(NAV_PROTO, 'userAgentData', {
                get: userAgentDataGetter,
                set: undefined,
                enumerable: true,
                configurable: true
            });
        } catch (e) { /* stay native */ }
    } catch (e) {
        // Contract: never leak anything to the page.
    }
})();

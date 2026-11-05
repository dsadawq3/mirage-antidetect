/*
 * Mirage inject layer - module "plugins" (see docs/CONTRACT.md).
 *
 * Spoofs navigator.plugins / navigator.mimeTypes with the static set a
 * modern Chrome ships: 5 PDF-family plugins ("PDF Viewer", "Chrome PDF
 * Viewer", "Chromium PDF Viewer", "Microsoft Edge PDF Viewer", "WebKit
 * built-in PDF"), filename "internal-pdf-viewer", description "Portable
 * Document Format". EVERY plugin carries both mime types
 * (application/pdf + text/pdf, length 2), exactly like the calibrated
 * native Chrome. navigator.mimeTypes is the DEDUPLICATED view over the
 * plugins' mime types: exactly 2 entries (application/pdf, text/pdf).
 * The MimeType instances are shared per type - plugins[i]['application/pdf']
 * === navigator.mimeTypes['application/pdf'] for every plugin, and
 * enabledPlugin points at the first plugin owning the type (PDF Viewer).
 *
 * Own-property hygiene (native Chrome shapes, verified against 156):
 * - Native PluginArray / Plugin / MimeTypeArray instances have NO own
 *   length/item/namedItem/refresh, native Plugin has no own
 *   name/description/filename/length/item/namedItem - all of those live
 *   on the interface prototypes; only the indexed/named entries
 *   (0..n-1, plugin names, mime type names) are own properties of the
 *   instances, and a native MimeType instance has no own props at all.
 *   The fakes reproduce this with an intermediate fake prototype layer:
 *   every fake prototype is Object.create(<real interface prototype>),
 *   so instanceof against Plugin / PluginArray / MimeType /
 *   MimeTypeArray still passes, while the fields and methods live on the
 *   fake prototypes and the instances carry ONLY indices and names
 *   (MimeType instances: nothing). Each fake prototype also mirrors the
 *   native non-enumerable 'constructor' own property.
 * - item()/namedItem() use ToUint32 semantics (Number(index) >>> 0),
 *   matching the native WebIDL getters: non-numeric indices coerce to
 *   0, out-of-range returns null; namedItem resolves case-sensitively to
 *   the first matching entry, like the native named accessors.
 * - [SameObject]: the arrays, the plugins and the two MimeType objects
 *   are built once; the getters hand out the same instances every time.
 * - Everything installed is registered in the toString-mask registry
 *   (window.__MIRAGE_RNG__.track) so Function.prototype.toString shows
 *   native code for the fake methods and getters.
 */
;(function () {
    'use strict';
    try {
        var MARK = 'plugins';

        var done = window.__MIRAGE_DONE__;
        if (!done || typeof done !== 'object') {
            done = {};
            window.__MIRAGE_DONE__ = done;
        }
        if (done[MARK]) return;
        done[MARK] = true;

        var fp = window.__MIRAGE_FP__;
        if (!fp || typeof fp !== 'object') return;

        // Chrome's static PDF-only plugin set (the Pepper plugins are long
        // gone; the 5 names differ per distribution channel). Every plugin
        // carries BOTH mime types, like the real thing.
        var PLUGIN_NAMES = [
            'PDF Viewer',
            'Chrome PDF Viewer',
            'Chromium PDF Viewer',
            'Microsoft Edge PDF Viewer',
            'WebKit built-in PDF'
        ];
        var PLUGIN_DESCRIPTION = 'Portable Document Format';
        var PLUGIN_FILENAME = 'internal-pdf-viewer';
        var MIME_DEFS = [
            { type: 'application/pdf', suffix: 'pdf', description: 'Portable Document Format' },
            { type: 'text/pdf', suffix: 'pdf', description: 'Portable Document Format' }
        ];

        function ifaceCtor(ctorName) {
            try {
                var ctor = window[ctorName];
                if (typeof ctor === 'function' && ctor.prototype) {
                    return ctor;
                }
            } catch (e) { /* legacy runtime without the interface */ }
            return null;
        }

        var PluginCtor = ifaceCtor('Plugin');
        var PluginArrayCtor = ifaceCtor('PluginArray');
        var MimeTypeCtor = ifaceCtor('MimeType');
        var MimeTypeArrayCtor = ifaceCtor('MimeTypeArray');

        var pluginIfaceProto = PluginCtor ? PluginCtor.prototype : Object.prototype;
        var pluginArrayIfaceProto = PluginArrayCtor ? PluginArrayCtor.prototype : Object.prototype;
        var mimeTypeIfaceProto = MimeTypeCtor ? MimeTypeCtor.prototype : Object.prototype;
        var mimeTypeArrayIfaceProto = MimeTypeArrayCtor ? MimeTypeArrayCtor.prototype : Object.prototype;

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

        function defProp(target, name, descriptor) {
            try {
                Object.defineProperty(target, name, descriptor);
            } catch (e) {
                // A partially built object is still readable.
            }
        }

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

        function defGetter(target, name, fn) {
            var g = makeNative(name, 0, function (thisArg) {
                return fn.call(thisArg);
            });
            track(g, name);
            defProp(target, name, {
                get: g,
                set: undefined,
                enumerable: true,
                configurable: true
            });
        }

        function defMethod(target, name, fn) {
            var w = makeNative(name, fn.length, function (thisArg, args) {
                return fn.apply(thisArg, args);
            });
            track(w, name);
            defProp(target, name, {
                value: w,
                writable: true,
                enumerable: true,
                configurable: true
            });
        }

        function defConstructor(target, ctor) {
            if (ctor) {
                defProp(target, 'constructor', {
                    value: ctor,
                    writable: true,
                    enumerable: false,
                    configurable: true
                });
            }
        }

        // ---- shared MimeType instances (one per type) -----------------------
        // Built after the plugins exist (enabledPlugin needs plugins[0]);
        // both mime types belong to the first plugin in the native set.
        var mimes = [];            // [MimeType 'application/pdf', MimeType 'text/pdf']
        var mimeByName = {};       // type -> shared instance
        var plugins = [];          // the 5 fake Plugin objects

        // ---- fake Plugin prototype layer ------------------------------------
        // Shared base: everything plugin-invariant (length, item, namedItem,
        // constructor) lives here once, BETWEEN the instances and the real
        // Plugin.prototype.
        var pluginBaseProto = Object.create(pluginIfaceProto);
        defConstructor(pluginBaseProto, PluginCtor);
        defGetter(pluginBaseProto, 'length', function length() { return mimes.length; });
        defMethod(pluginBaseProto, 'item', function item(index) {
            var n = Number(index) >>> 0;
            return n < mimes.length ? mimes[n] : null;
        });
        defMethod(pluginBaseProto, 'namedItem', function namedItem(name) {
            var key = (name === null || name === undefined) ? '' : String(name);
            if (Object.prototype.hasOwnProperty.call(mimeByName, key)) return mimeByName[key];
            return null;
        });

        function makePlugin(name) {
            // Per-plugin prototype: the identity fields differ per plugin,
            // so they need one fake prototype each; the instance itself
            // gets ONLY the indexed/named mime entries (wired later, after
            // the shared MimeType instances exist).
            var pluginName = name;
            var proto = Object.create(pluginBaseProto);
            defGetter(proto, 'name', function name() { return pluginName; });
            defGetter(proto, 'description', function description() { return PLUGIN_DESCRIPTION; });
            defGetter(proto, 'filename', function filename() { return PLUGIN_FILENAME; });
            return Object.create(proto);
        }

        // ---- fake PluginArray prototype layer -------------------------------
        var pluginArrayProto = Object.create(pluginArrayIfaceProto);
        defConstructor(pluginArrayProto, PluginArrayCtor);
        defGetter(pluginArrayProto, 'length', function length() { return plugins.length; });
        defMethod(pluginArrayProto, 'item', function item(index) {
            var n = Number(index) >>> 0;
            return n < plugins.length ? plugins[n] : null;
        });
        defMethod(pluginArrayProto, 'namedItem', function namedItem(name) {
            var key = (name === null || name === undefined) ? '' : String(name);
            for (var j = 0; j < plugins.length; j++) {
                if (plugins[j].name === key) return plugins[j];
            }
            return null;
        });
        defMethod(pluginArrayProto, 'refresh', function refresh() {
            // Static set in Chrome: nothing to reload, no-op like native.
        });

        // ---- fake MimeType prototype layer (one per shared instance) --------
        function makeMimeType(def, enabledPlugin) {
            var owner = enabledPlugin;
            var proto = Object.create(mimeTypeIfaceProto);
            defConstructor(proto, MimeTypeCtor);
            defGetter(proto, 'type', function type() { return def.type; });
            defGetter(proto, 'suffix', function suffix() { return def.suffix; });
            defGetter(proto, 'description', function description() { return def.description; });
            defGetter(proto, 'enabledPlugin', function enabledPlugin() { return owner; });
            // Native MimeType instances have no own properties at all.
            return Object.create(proto);
        }

        // ---- fake MimeTypeArray prototype layer -----------------------------
        var mimeArrayEntries = []; // [shared app/pdf, shared text/pdf]
        var mimeArrayProto = Object.create(mimeTypeArrayIfaceProto);
        defConstructor(mimeArrayProto, MimeTypeArrayCtor);
        defGetter(mimeArrayProto, 'length', function length() { return mimeArrayEntries.length; });
        defMethod(mimeArrayProto, 'item', function item(index) {
            var n = Number(index) >>> 0;
            return n < mimeArrayEntries.length ? mimeArrayEntries[n] : null;
        });
        defMethod(mimeArrayProto, 'namedItem', function namedItem(name) {
            var key = (name === null || name === undefined) ? '' : String(name);
            if (Object.prototype.hasOwnProperty.call(mimeByName, key)) return mimeByName[key];
            return null;
        });

        // ---- build the graph -------------------------------------------------
        // Plugins first (with empty mime lists), then the shared MimeType
        // instances (enabledPlugin -> the first plugin), then wire the
        // indexed/named entries of every plugin to the shared instances.
        for (var i = 0; i < PLUGIN_NAMES.length; i++) {
            plugins.push(makePlugin(PLUGIN_NAMES[i]));
        }
        for (var j = 0; j < MIME_DEFS.length; j++) {
            var mt = makeMimeType(MIME_DEFS[j], plugins[0]);
            mimes.push(mt);
            mimeByName[MIME_DEFS[j].type] = mt;
        }
        for (var k = 0; k < plugins.length; k++) {
            var pl = plugins[k];
            for (var m = 0; m < mimes.length; m++) {
                var shared = mimes[m];
                defProp(pl, String(m), { value: shared, writable: true, enumerable: true, configurable: true });
                defProp(pl, shared.type, { value: shared, writable: true, enumerable: true, configurable: true });
            }
        }

        var pluginArray = Object.create(pluginArrayProto);
        for (var a = 0; a < plugins.length; a++) {
            defProp(pluginArray, String(a), { value: plugins[a], writable: true, enumerable: true, configurable: true });
            defProp(pluginArray, plugins[a].name, { value: plugins[a], writable: true, enumerable: true, configurable: true });
        }

        // navigator.mimeTypes: the deduplicated view - exactly one entry
        // per distinct type, the shared instances.
        for (var b = 0; b < mimes.length; b++) {
            mimeArrayEntries.push(mimes[b]);
        }
        var mimeArray = Object.create(mimeArrayProto);
        for (var c = 0; c < mimeArrayEntries.length; c++) {
            var entry = mimeArrayEntries[c];
            defProp(mimeArray, String(c), { value: entry, writable: true, enumerable: true, configurable: true });
            if (!Object.prototype.hasOwnProperty.call(mimeArray, entry.type)) {
                defProp(mimeArray, entry.type, { value: entry, writable: true, enumerable: true, configurable: true });
            }
        }

        // ---- install the accessors ------------------------------------------
        // [SameObject]: the same cached instances on every access. The getters
        // carry the native brand check (a real Navigator getter throws
        // "Illegal invocation" when read off the prototype itself).
        var NAV_PROTO = window.Navigator && window.Navigator.prototype;
        if (NAV_PROTO) {
            function navGetter(name, valueFn) {
                var g = makeNative(name, 0, function (thisArg) {
                    if (thisArg === NAV_PROTO || !(thisArg instanceof NAV_CTOR)) {
                        throw new TypeError('Illegal invocation');
                    }
                    return valueFn();
                });
                track(g, name);
                return g;
            }
            try {
                Object.defineProperty(NAV_PROTO, 'plugins', {
                    get: navGetter('plugins', function () { return pluginArray; }),
                    set: undefined,
                    enumerable: true,
                    configurable: true
                });
            } catch (e) { /* stay native */ }
            try {
                Object.defineProperty(NAV_PROTO, 'mimeTypes', {
                    get: navGetter('mimeTypes', function () { return mimeArray; }),
                    set: undefined,
                    enumerable: true,
                    configurable: true
                });
            } catch (e) { /* stay native */ }
        }
    } catch (e) {
        // Contract: never leak anything to the page.
    }
})();

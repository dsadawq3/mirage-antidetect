/*
 * Mirage inject layer - module "screen" (see docs/CONTRACT.md).
 *
 * Spoofs the screen geometry (Screen.prototype: width, height, availWidth,
 * availHeight, colorDepth, pixelDepth) and the window-level metrics
 * (devicePixelRatio, outerWidth, outerHeight) from fp.screen.
 *
 * Why: screen size, avail size and devicePixelRatio are core fingerprint
 * dimensions with no CDP override. Screen is a regular WebIDL interface -
 * its accessors live on Screen.prototype, so a prototype-level swap is both
 * effective and descriptor-clean.
 *
 * Window properties need different treatment: Window is a [Global] WebIDL
 * interface, and Blink installs its attributes as OWN accessors of the
 * window instance, not on Window.prototype. A plain Window.prototype
 * override would be silently shadowed by the instance accessor (and would
 * leave a visible extra property on the prototype), so this module
 * redefines the own instance accessor in place first (preserving the native
 * enumerability and setter) and falls back to Window.prototype only when
 * the instance has no own property for the name. Both engine layouts are
 * covered; in Chrome the instance path is the one that fires.
 *
 * Deliberately NOT touched:
 * - innerWidth / innerHeight - they must stay consistent with the real
 *   viewport, otherwise layout-measurement detection fires instantly; the
 *   honest value is the safe value.
 * - outerWidth / outerHeight mirror availWidth / availHeight: a maximized
 *   Chrome window on Windows covers the work area, not the full screen -
 *   the taskbar is excluded. Reporting outerHeight == screen.height is
 *   geometrically impossible next to availHeight == screen.height - 40
 *   (outerHeight > availHeight never happens for a maximized window) and
 *   is a trivially detectable inconsistency. (The launcher opens profiles
 *   with --start-maximized; a non-maximized window still leaks its real
 *   outer bounds - accepted trade-off of this wave.)
 */
;(function () {
    'use strict';
    try {
        var MARK = 'screen';

        var done = window.__MIRAGE_DONE__;
        if (!done || typeof done !== 'object') {
            done = {};
            window.__MIRAGE_DONE__ = done;
        }
        if (done[MARK]) return;
        done[MARK] = true;

        var fp = window.__MIRAGE_FP__;
        if (!fp || typeof fp !== 'object') return;

        var scr = fp.screen;
        if (!scr || typeof scr !== 'object') return;

        var SCREEN_PROTO = window.Screen && window.Screen.prototype;
        if (!SCREEN_PROTO) return;

        var WIN_PROTO = window.Window && window.Window.prototype;

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
        var SCREEN_CTOR = window.Screen;

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

        // Screen getter with the native brand check (throws "Illegal
        // invocation" when read off Screen.prototype, like the real one).
        function defineGetter(name, valueFn) {
            try {
                var g = makeNative(name, 0, function (thisArg) {
                    if (thisArg === SCREEN_PROTO || !(thisArg instanceof SCREEN_CTOR)) {
                        throw new TypeError('Illegal invocation');
                    }
                    return valueFn();
                });
                track(g, name);
                Object.defineProperty(SCREEN_PROTO, name, {
                    get: g,
                    set: undefined,
                    enumerable: true,
                    configurable: true
                });
            } catch (e) {
                // Field stays native.
            }
        }

        if (typeof scr.width === 'number') {
            defineGetter('width', function width() { return scr.width; });
        }
        if (typeof scr.height === 'number') {
            defineGetter('height', function height() { return scr.height; });
        }
        if (typeof scr.availWidth === 'number') {
            defineGetter('availWidth', function availWidth() { return scr.availWidth; });
        }
        if (typeof scr.availHeight === 'number') {
            defineGetter('availHeight', function availHeight() { return scr.availHeight; });
        }

        // Chrome always reports colorDepth === pixelDepth (24 for the
        // calibrated profiles); fall back to 24 when the field is missing.
        var depth = (typeof scr.colorDepth === 'number') ? scr.colorDepth : 24;
        defineGetter('colorDepth', function colorDepth() { return depth; });
        defineGetter('pixelDepth', function pixelDepth() { return depth; });

        // Window-level metrics: own instance accessor first (see header),
        // prototype only as a fallback. The getter is native-looking; no
        // brand check (Window getters are read off the window instance).
        function overrideWindowProp(name, valueFn) {
            var g = makeNative(name, 0, function () { return valueFn(); });
            track(g, name);
            try {
                var own = Object.getOwnPropertyDescriptor(window, name);
                if (own) {
                    if (own.configurable) {
                        Object.defineProperty(window, name, {
                            get: g,
                            set: own.set,
                            enumerable: own.enumerable,
                            configurable: true
                        });
                    }
                    return; // own but non-configurable: nothing safe left to do
                }
            } catch (e) {
                // fall through to the prototype attempt
            }
            if (WIN_PROTO) {
                try {
                    Object.defineProperty(WIN_PROTO, name, {
                        get: g,
                        set: undefined,
                        enumerable: true,
                        configurable: true
                    });
                } catch (e) {
                    // give up silently
                }
            }
        }

        if (typeof scr.dpr === 'number') {
            overrideWindowProp('devicePixelRatio', function devicePixelRatio() { return scr.dpr; });
        }
        // Maximized-window geometry: outer == avail (see header). Falling
        // back to the full width/height only when avail is not configured.
        if (typeof scr.availWidth === 'number') {
            overrideWindowProp('outerWidth', function outerWidth() { return scr.availWidth; });
        } else if (typeof scr.width === 'number') {
            overrideWindowProp('outerWidth', function outerWidth() { return scr.width; });
        }
        if (typeof scr.availHeight === 'number') {
            overrideWindowProp('outerHeight', function outerHeight() { return scr.availHeight; });
        } else if (typeof scr.height === 'number') {
            overrideWindowProp('outerHeight', function outerHeight() { return scr.height; });
        }

        // innerWidth / innerHeight intentionally left native (real viewport).
    } catch (e) {
        // Contract: never leak anything to the page.
    }
})();

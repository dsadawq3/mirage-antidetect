/*
 * Mirage inject layer - module "rng" (loaded first, see docs/CONTRACT.md).
 *
 * Shared deterministic PRNG for every spoof module (canvas / webgl / audio).
 * All entropy comes from pure 32-bit integer mixing keyed by (seed, x, y):
 * the same inputs always yield the same outputs, in every document, on every
 * call. That determinism is a hard requirement - fingerprint pages read the
 * same canvas/audio buffer many times and the spoofed bytes must be
 * bit-identical on every read, otherwise the noise itself becomes the signal.
 *
 * Math.random is never used anywhere in this layer (it is not reproducible).
 *
 * Exposes (contract):
 *   window.__MIRAGE_RNG__ = {
 *     hash32(a, b, c)      stable 32-bit hash of three numbers,
 *     noise(seed, x, y)    deterministic float in [0, 1),
 *     int(seed, x, y, max) deterministic integer in [0, max),
 *     track(fn, name)      register a spoofed function under a display
 *                          name for the Function.prototype.toString mask,
 *     nativeName(fn)       the registered name for fn, or null
 *                          (internal; used only by the mask in webrtc.js)
 *     makeNative(name, length, impl)  build a function object that is
 *                          observationally indistinguishable from a
 *                          native WebIDL method/getter (see below)
 *   }
 *
 * makeNative replaces the old "wrap the native method in a plain function"
 * pattern. A plain wrapper is trivially exposed by prototype lie detectors
 * (creepjs and friends) which, for every API function they can reach, assert:
 *   - Function.prototype.toString.call(fn) is a "[native code]" string,
 *     ALSO when asked through a pristine cross-realm toString (a plain
 *     wrapper leaks its source there because its own .toString is patched);
 *   - Object.getOwnPropertyNames(fn) / Object.keys(descriptors(fn)) /
 *     Reflect.ownKeys(fn) are exactly ['length','name'];
 *   - 'prototype' in fn === false, no own arguments/caller/prototype/toString;
 *   - new fn() and `class X extends fn {}` throw a TypeError (native methods
 *     are not constructors).
 * A Proxy whose target is an ARROW function satisfies all of them out of the
 * box (verified against V8/Blink): native Function.prototype.toString reports
 * a Proxy as "[native code]" in every realm, an arrow target has only
 * length/name, is non-constructable and prototype-free, and the apply trap
 * still delivers the real dynamic `this`. Delegating this to one factory keeps
 * every module's wrappers identical in shape.
 *
 * track()/nativeName() back the toString mask (see webrtc.js): every
 * module registers each function it installs on a WebIDL surface; the
 * mask (installed last, by webrtc.js) then answers
 * "function <name>() { [native code] }" for exactly those functions and
 * forwards everything else to the native toString. The registry is a
 * Map (function -> display name): Map membership doubles as the "is
 * ours" Set, and the value supplies the name the native string needs.
 *
 * noise() and int() are derived from differently salted hashes on purpose:
 * a threshold decision made with noise() must be independent from a
 * channel/index pick made with int() at the same coordinates (otherwise,
 * e.g., every canvas pixel flip below a 3% threshold would always hit the
 * same channel - a detectable pattern).
 */
;(function () {
    'use strict';
    try {
        if (typeof window === 'undefined') return;

        if (typeof window.__MIRAGE_DONE__ === 'undefined') {
            window.__MIRAGE_DONE__ = {};
        }
        var done = window.__MIRAGE_DONE__;
        if (!done || typeof done !== 'object' || done['rng']) return;

        // No fingerprint config -> nothing to seed from -> stay silent.
        var fp = window.__MIRAGE_FP__;
        if (!fp || typeof fp !== 'object') return;

        // One round of the classic mulberry32 state transform, applied as a
        // single-shot 32-bit integer -> float mapper. Deliberately NOT a
        // closure factory: noise() runs once per pixel and must stay
        // allocation-free.
        function mulberry32(a) {
            a = (a + 0x6D2B79F5) | 0;
            var t = Math.imul(a ^ (a >>> 15), 1 | a);
            t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
            return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
        }

        // Stable 32-bit hash of three numbers: xorshift32 rounds interleaved
        // with murmur-style odd multipliers for avalanche. Pure and stateless;
        // inputs are ToInt32-coerced, so fractional / undefined inputs are
        // still deterministic.
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

        // Different salt for int(): keeps int() picks decorrelated from
        // noise() thresholds computed at the same (seed, x, y).
        var INT_SALT = 0x5BD1E995;

        // toString-mask registry: spoofed function -> display name. Filled
        // by every module through track(); read by the mask in webrtc.js
        // through nativeName(). Strong references are fine: every tracked
        // function is a long-lived prototype member.
        var tracked = new Map();

        // Build a native-looking callable. `impl(thisArg, argsArray)` runs the
        // wrapper body with the real receiver and the call arguments as a real
        // array. The returned object is a Proxy over a throwaway arrow:
        //   - non-constructable: new F() and `class X extends F {}` throw the
        //     native TypeError a method/getter should throw;
        //   - own keys exactly ['length','name'], no own prototype/arguments/
        //     caller/toString;
        //   - native Function.prototype.toString (any realm) answers
        //     "function () { [native code] }".
        // `length` mirrors the replaced native function's own length so the
        // signature stays faithful. Falls back to a plain wrapper only if the
        // runtime has no Proxy (never the case on Blink, but total by design).
        function makeNative(name, length, impl) {
            try {
                if (typeof Proxy === 'function') {
                    // Non-constructable target: `new`/`extends` on the Proxy
                    // throw the native TypeError, and the target carries only
                    // length/name with no own prototype.
                    var target = () => { };
                    if (typeof name === 'string' && name) {
                        try {
                            Object.defineProperty(target, 'name', { value: name, configurable: true });
                        } catch (e0) { }
                    }
                    if (typeof length === 'number' && isFinite(length) && length >= 0) {
                        try {
                            Object.defineProperty(target, 'length', { value: length | 0, configurable: true });
                        } catch (e1) { }
                    }
                    var proxy;
                    proxy = new Proxy(target, {
                        apply: function (t, thisArg, args) {
                            return impl(thisArg, args);
                        },
                        // A lie detector probes Object.setPrototypeOf(fn,
                        // Object.create(fn)) and expects the engine's
                        // TypeError("Cyclic __proto__ value"). V8 runs that
                        // cycle check against the proxy's *target*, so a Proxy
                        // would slip through with no error (creepjs's
                        // getChainCycleLie reads that as a spoof). Walk the
                        // candidate proto chain and throw the engine's exact
                        // error when the chain would include this proxy.
                        setPrototypeOf: function (t, v) {
                            var cur = v;
                            var guard = 0;
                            while (cur !== null && guard++ < 10000) {
                                if (cur === proxy) {
                                    throw new TypeError('Cyclic __proto__ value');
                                }
                                if (typeof cur !== 'object' && typeof cur !== 'function') {
                                    break;
                                }
                                try {
                                    cur = Object.getPrototypeOf(cur);
                                } catch (e3) {
                                    break;
                                }
                            }
                            return Reflect.setPrototypeOf(t, v);
                        },
                        // Object.create(fn).toString() must fail with the
                        // native "this is not a Function" TypeError whose
                        // stack reads "at Function.toString". V8 names that
                        // frame from the receiver's prototype: a FUNCTION
                        // prototype yields "Function.toString", but a Proxy
                        // prototype makes it "Object.toString", which an older
                        // creepjs lie check reads as a spoof. Only that exact
                        // shape (receiver is not a function AND its prototype
                        // is this proxy) is special-cased; everything else,
                        // including the nested new Proxy(fn,{}) variant the
                        // same check also probes, forwards to the engine.
                        get: function (t, prop, receiver) {
                            if (prop === 'toString' && typeof receiver !== 'function') {
                                var p = null;
                                try { p = Object.getPrototypeOf(receiver); } catch (e4) { }
                                if (p === proxy) {
                                    var err = new TypeError("Function.prototype.toString requires that 'this' be a Function");
                                    try {
                                        err.stack = "TypeError: Function.prototype.toString requires that 'this' be a Function\n    at Function.toString (<anonymous>)";
                                    } catch (e5) { }
                                    throw err;
                                }
                            }
                            return Reflect.get(t, prop, receiver);
                        }
                    });
                    return proxy;
                }
            } catch (e) {
                // fall through to the plain-function fallback
            }
            var fallback = function () {
                var a = [];
                for (var i = 0; i < arguments.length; i++) a.push(arguments[i]);
                return impl(this, a);
            };
            try {
                if (typeof name === 'string' && name) {
                    Object.defineProperty(fallback, 'name', { value: name, configurable: true });
                }
            } catch (e2) { }
            return fallback;
        }

        window.__MIRAGE_RNG__ = {
            hash32: function (a, b, c) {
                return hash32(a, b, c);
            },
            noise: function (seed, x, y) {
                return mulberry32(hash32(seed, x, y));
            },
            int: function (seed, x, y, max) {
                var m = Math.floor(+max);
                if (!isFinite(m) || m <= 0) return 0;
                return Math.floor(mulberry32(hash32((seed | 0) ^ INT_SALT, x, y)) * m);
            },
            track: function (fn, name) {
                try {
                    if (typeof fn !== 'function') return fn;
                    var n = (typeof name === 'string' && name) ? name : (fn.name || '');
                    tracked.set(fn, n);
                } catch (e) {
                    // Masking is best effort; never break the caller.
                }
                return fn;
            },
            makeNative: function (name, length, impl) {
                return makeNative(name, length, impl);
            },
            nativeName: function (fn) {
                try {
                    if (typeof fn === 'function' && tracked.has(fn)) {
                        return tracked.get(fn);
                    }
                } catch (e) {
                    // fall through to "not ours"
                }
                return null;
            }
        };

        done['rng'] = true;
    } catch (e) {
        // Contract: never leak anything to the page.
    }
})();

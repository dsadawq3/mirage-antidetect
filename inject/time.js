/*
 * Mirage inject layer - module "time" (see docs/CONTRACT.md).
 *
 * Timezone spoof fallback behind the CDP override. The launcher applies
 * Emulation.setTimezoneOverride before any page script runs, which
 * emulates the zone natively - undetectable from JS. This module only
 * activates when the runtime zone does NOT equal fp.timezone (override
 * missing, failed or reset) and then fakes the JS-visible timezone
 * surface:
 *
 *   - Date.prototype.getTimezoneOffset - offset of fp.timezone for the
 *     receiver's instant, DST-correct by construction: the offset is
 *     derived per-instant from Intl wall-clock components, not from a
 *     fixed table;
 *   - Date.prototype.toString / toTimeString - the native string with the
 *     GMT+/-HHMM label rewritten to the spoofed offset (the "GMT" part is
 *     verified to exist before any replacement);
 *   - Intl.DateTimeFormat - constructor wrapper that injects fp.timezone
 *     into a COPY of the options whenever the caller passed no explicit
 *     timeZone (the caller's object is never mutated; null options keep
 *     the native TypeError instead of being silently "fixed");
 *   - Intl.DateTimeFormat.prototype.resolvedOptions - when the result
 *     carries the runtime zone, it is replaced with fp.timezone in a
 *     copy of the result object.
 *
 * If the runtime zone already equals fp.timezone, NOTHING is patched: a
 * JS-patched Date is easier to detect than the native emulation and must
 * never be stacked on top of it.
 *
 * Deliberately NOT patched (per contract): Date#toLocaleString,
 * toLocaleDateString, toLocaleTimeString. Caveat documented for the team:
 * per ECMA-402 these are specified against the %DateTimeFormat% intrinsic,
 * NOT against the Intl.DateTimeFormat property this module replaces, so on
 * the fallback path they keep formatting with the real runtime zone - the
 * CDP override stays the mechanism that has to cover them.
 *
 * Cosmetic limits on the fallback path: the wall-clock digits and the
 * parenthesized zone name in Date#toString output come from the original
 * native call - only the GMT offset label is rewritten.
 */
;(function () {
    'use strict';
    try {
        var MARK = 'time';

        var done = window.__MIRAGE_DONE__;
        if (!done || typeof done !== 'object') {
            done = {};
            window.__MIRAGE_DONE__ = done;
        }
        if (done[MARK]) return;

        // Marked BEFORE patching: this module swaps the Intl.DateTimeFormat
        // constructor, which is not double-run-safe (a second run would
        // capture the already-patched constructor as "original").
        done[MARK] = true;

        var fp = window.__MIRAGE_FP__;
        if (!fp || typeof fp !== 'object') return;

        var TZ = (typeof fp.timezone === 'string') ? fp.timezone : '';
        if (!TZ) return;

        var INTL = window.Intl;
        var OrigDTF = INTL && INTL.DateTimeFormat;
        if (typeof OrigDTF !== 'function' || !OrigDTF.prototype) return;
        var DTF_PROTO = OrigDTF.prototype;
        if (typeof DTF_PROTO.resolvedOptions !== 'function') return;

        // Actual runtime zone (the one the CDP override would have changed).
        function zoneOf() {
            try {
                var o = new OrigDTF().resolvedOptions();
                return (o && typeof o.timeZone === 'string') ? o.timeZone : null;
            } catch (e) {
                return null;
            }
        }

        var runtimeZone = zoneOf();
        if (runtimeZone === TZ) {
            // Native CDP emulation is already in effect - do not stack a
            // JS patch on top of it.
            return;
        }

        // Validate the configured zone before installing anything: an
        // invalid IANA name would turn every patched call into RangeError.
        try {
            new OrigDTF('en-US', { timeZone: TZ });
        } catch (e) {
            return; // bad config - stay fully native instead of breaking Date
        }

        var DATE_PROTO = window.Date && window.Date.prototype;
        if (!DATE_PROTO) return;

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

        // One reusable formatter for all offset math (bound to the target
        // zone once; formatToParts is called per conversion).
        var PARTS_DTF = new OrigDTF('en-US', {
            timeZone: TZ,
            hour12: false,
            year: 'numeric',
            month: '2-digit',
            day: '2-digit',
            hour: '2-digit',
            minute: '2-digit',
            second: '2-digit'
        });
        if (typeof PARTS_DTF.formatToParts !== 'function') return;

        var cacheKey = null; // minute bucket of the last computed offset
        var cacheVal = 0;

        // Offset of TZ for `date`, in minutes, in the "local - UTC" sense
        // (UTC+3 -> 180, UTC-5 -> -300, Europe/London -> 0 in winter).
        // NaN for invalid dates. Wall-clock components of `date` in TZ are
        // interpreted as UTC and diffed against the real UTC timestamp:
        // that difference IS the zone offset for that exact instant, so
        // DST transitions are handled for free.
        function offsetFor(date) {
            var t = date.getTime();
            if (isNaN(t)) return NaN;
            var key = Math.floor(t / 60000); // offsets change on minute edges only
            if (key === cacheKey) return cacheVal;
            var parts;
            try {
                parts = PARTS_DTF.formatToParts(date);
            } catch (e) {
                return NaN;
            }
            var y = 0, mo = 0, d = 0, h = 0, mi = 0, s = 0;
            for (var i = 0; i < parts.length; i++) {
                var p = parts[i];
                switch (p.type) {
                    case 'year': y = parseInt(p.value, 10); break;
                    case 'month': mo = parseInt(p.value, 10); break;
                    case 'day': d = parseInt(p.value, 10); break;
                    case 'hour': h = parseInt(p.value, 10); break;
                    case 'minute': mi = parseInt(p.value, 10); break;
                    case 'second': s = parseInt(p.value, 10); break;
                    default: break;
                }
            }
            if (h === 24) h = 0; // some engines emit hour 24 at midnight
            var asUTC = Date.UTC(y, mo - 1, d, h, mi, s);
            var off = Math.round((asUTC - t) / 60000);
            cacheKey = key;
            cacheVal = off;
            return off;
        }

        // "GMT+0300" / "GMT-0530" style label from an offset in minutes.
        function gmtLabel(off) {
            var sign = off < 0 ? '-' : '+';
            var abs = off < 0 ? -off : off;
            var hh = Math.floor(abs / 60);
            var mm = abs - hh * 60;
            return 'GMT' + sign + (hh < 10 ? '0' : '') + hh + (mm < 10 ? '0' : '') + mm;
        }

        var origGetTZOffset = DATE_PROTO.getTimezoneOffset;
        var origToString = DATE_PROTO.toString;
        var origToTimeString = DATE_PROTO.toTimeString;

        // getTimezoneOffset() returns "UTC - local" in minutes, i.e. the
        // negated offsetFor() result (Moscow UTC+3 -> -180).
        function getTimezoneOffset() {
            // Brand check instead of instanceof: stays correct for Date
            // objects from other realms; wrong receivers get the native
            // TypeError shape.
            if (Object.prototype.toString.call(this) !== '[object Date]') {
                throw new TypeError('this is not a Date object.');
            }
            var off = offsetFor(this);
            if (isNaN(off)) return NaN; // invalid date: native returns NaN
            return -off;
        }

        // Rewrite the GMT+/-HHMM part of a native Date string; everything
        // else (wall digits, parenthesized zone name) stays untouched.
        function rewriteGmt(str, date) {
            if (typeof str !== 'string' || str.indexOf('GMT') === -1) return str;
            var off = offsetFor(date);
            if (isNaN(off)) return str;
            return str.replace(/GMT[+-]\d{2}:?\d{2}/, gmtLabel(off));
        }

        function toString() {
            var s = origToString.call(this); // native TypeError propagates, as before
            return rewriteGmt(s, this);
        }

        function toTimeString() {
            var s = origToTimeString.call(this);
            return rewriteGmt(s, this);
        }

        // Native-looking wrappers around the Date methods above (own keys
        // length,name; no own prototype; not constructable).
        var getTimezoneOffsetW = makeNative('getTimezoneOffset', origGetTZOffset.length, function (thisArg, args) {
            return getTimezoneOffset.apply(thisArg, args);
        });
        var toStringW = makeNative('toString', origToString.length, function (thisArg, args) {
            return toString.apply(thisArg, args);
        });
        var toTimeStringW = makeNative('toTimeString', origToTimeString.length, function (thisArg, args) {
            return toTimeString.apply(thisArg, args);
        });
        track(getTimezoneOffsetW, 'getTimezoneOffset');
        track(toStringW, 'toString');
        track(toTimeStringW, 'toTimeString');

        try {
            Object.defineProperty(DATE_PROTO, 'getTimezoneOffset', {
                value: getTimezoneOffsetW,
                writable: true,
                enumerable: false,
                configurable: true
            });
        } catch (e) { /* stay native on this method */ }

        try {
            Object.defineProperty(DATE_PROTO, 'toString', {
                value: toStringW,
                writable: true,
                enumerable: false,
                configurable: true
            });
        } catch (e) { /* stay native on this method */ }

        try {
            Object.defineProperty(DATE_PROTO, 'toTimeString', {
                value: toTimeStringW,
                writable: true,
                enumerable: false,
                configurable: true
            });
        } catch (e) { /* stay native on this method */ }

        // ---- Intl.DateTimeFormat constructor ---------------------------------

        function DateTimeFormat() {
            var rawArgs = arguments;
            var args;
            try {
                args = [];
                for (var i = 0; i < rawArgs.length; i++) args.push(rawArgs[i]);

                if (args.length === 0) {
                    args.push(undefined);        // locales slot
                    args.push({ timeZone: TZ }); // options slot
                } else if (args.length === 1) {
                    args.push({ timeZone: TZ }); // options slot after locales
                } else {
                    var options = args[1];
                    if (options === undefined) {
                        args[1] = { timeZone: TZ };
                    } else if (options === null) {
                        // Native throws TypeError for null options - keep
                        // that behaviour instead of silently "fixing" it
                        // (a divergence here would be fingerprintable).
                    } else if (typeof options === 'object') {
                        if (options.timeZone === undefined) {
                            // Copy: never mutate the caller's options object.
                            var merged = {};
                            for (var k in options) {
                                if (Object.prototype.hasOwnProperty.call(options, k)) {
                                    merged[k] = options[k];
                                }
                            }
                            merged.timeZone = TZ;
                            args[1] = merged;
                        }
                    } else {
                        // Primitive options are boxed by the spec and never
                        // carry a timeZone: replacing keeps the semantics
                        // identical while injecting the zone.
                        args[1] = { timeZone: TZ };
                    }
                }
            } catch (e) {
                // Argument surgery failed - pass through untouched.
                return Reflect.construct(OrigDTF, rawArgs, new.target || DateTimeFormat);
            }
            return Reflect.construct(OrigDTF, args, new.target || DateTimeFormat);
        }

        // Reuse the original prototype: instanceof, existing references and
        // the prototype methods keep working for both constructors.
        try { DateTimeFormat.prototype = DTF_PROTO; } catch (e) { /* non-standard engine */ }
        try {
            Object.defineProperty(DateTimeFormat, 'name', {
                value: 'DateTimeFormat',
                configurable: true
            });
        } catch (e) { /* name is cosmetic */ }
        try {
            if (typeof OrigDTF.supportedLocalesOf === 'function') {
                DateTimeFormat.supportedLocalesOf = OrigDTF.supportedLocalesOf;
            }
        } catch (e) { /* static is cosmetic */ }
        // Keep constructor identity consistent after the swap (native:
        // Intl.DateTimeFormat.prototype.constructor === Intl.DateTimeFormat).
        try {
            Object.defineProperty(DTF_PROTO, 'constructor', {
                value: DateTimeFormat,
                writable: true,
                enumerable: false,
                configurable: true
            });
        } catch (e) { /* constructor ref is cosmetic */ }
        track(DateTimeFormat, 'DateTimeFormat');

        INTL.DateTimeFormat = DateTimeFormat;

        // ---- Intl.DateTimeFormat.prototype.resolvedOptions ------------------

        var origResolvedOptions = DTF_PROTO.resolvedOptions;

        function resolvedOptions() {
            // Native TypeError for a wrong receiver propagates, as before.
            var res = origResolvedOptions.apply(this, arguments);
            try {
                if (res && typeof res === 'object' && res.timeZone === runtimeZone) {
                    var copy = {};
                    for (var k in res) {
                        if (Object.prototype.hasOwnProperty.call(res, k)) {
                            copy[k] = res[k];
                        }
                    }
                    copy.timeZone = TZ;
                    return copy;
                }
            } catch (e) {
                // fall through: hand out the original result object
            }
            return res;
        }

        try {
            Object.defineProperty(DTF_PROTO, 'resolvedOptions', {
                value: makeNative('resolvedOptions', origResolvedOptions.length, function (thisArg, args) {
                    return resolvedOptions.apply(thisArg, args);
                }),
                writable: true,
                enumerable: false,
                configurable: true
            });
            track(DTF_PROTO.resolvedOptions, 'resolvedOptions');
        } catch (e) { /* stay native on this method */ }
    } catch (e) {
        // Contract: never leak anything to the page.
    }
})();

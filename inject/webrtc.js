/*
 * Mirage inject layer - module "webrtc" (see docs/CONTRACT.md).
 *
 * Loaded LAST in the bundle (see docs/CONTRACT.md) - after every other
 * module has registered its spoofed functions in the toString registry.
 * Two independent jobs:
 *
 * 1) WebRTC IP-leak protection, mode from fp.webRTC:
 *
 *   "real"  - nothing is patched, WebRTC stays fully native.
 *
 *   "off"   - window.RTCPeerConnection (and the webkit alias when
 *             present) is DELETED. A Chrome without WebRTC does not
 *             expose the constructor at all; a constructor that throws
 *             TypeError is an invention that no native build exhibits.
 *             After the delete, typeof RTCPeerConnection === 'undefined',
 *             exactly like a WebRTC-less runtime.
 *
 *   "relay" - RTCPeerConnection is wrapped: a real instance is built via
 *             Reflect.construct(original, args, new.target || wrapper)
 *             and the wrapper installs INSTANCE-level filtering:
 *               - own onicecandidate accessor: the setter stores the page
 *                 handler AND registers a filtering handler through the
 *                 native prototype setter (so the native event machinery
 *                 really dispatches through the filter), the getter hands
 *                 the page its own handler back, not the wrapper;
 *               - own addEventListener/removeEventListener: 'icecandidate'
 *                 listeners are wrapped with the same filter, with a
 *                 per-instance map so repeated adds dedupe (native
 *                 behaviour) and removes unwrap to the original reference
 *                 (object listeners with handleEvent are wrapped too);
 *               - own getStats: the RTCStatsReport the native promise
 *                 resolves with is REBUILT without the records that would
 *                 leak real addresses - candidate records
 *                 (local-candidate / remote-candidate / legacy candidate)
 *                 whose candidateType is not 'relay', and the
 *                 candidate-pair records that reference them. Every other
 *                 record is passed through untouched. The rebuilt report
 *                 keeps the native shape (prototype chain through
 *                 RTCStatsReport.prototype, so instanceof passes; get /
 *                 has / size / forEach / entries / keys / values /
 *                 iteration all work) and the promise semantics are
 *                 preserved (the wrapper returns the native promise
 *                 chained through the filter; rejections pass through).
 *             Only relay candidates ("typ relay") and end-of-candidates
 *             events (null/empty candidate string) reach page JS; host and
 *             srflx candidates are dropped, so no local/real address
 *             leaks. Candidate filtering fails closed: an event that
 *             cannot be inspected is suppressed.
 *
 * 2) The Function.prototype.toString mask (the industry-standard
 *    whitelist approach): every spoofed function of the whole inject
 *    layer was registered in window.__MIRAGE_RNG__ (track(fn, name)).
 *    Function.prototype.toString is replaced once, here: for a registered
 *    receiver it answers "function <name>() { [native code] }", for
 *    everything else it forwards to the captured native toString via
 *    Reflect.apply - honest functions (including bound and cross-realm
 *    ones) keep their exact native behaviour. The mask itself reports
 *    the canonical native string for itself. If the registry is missing
 *    (rng.js did not run), the mask is not installed and nothing changes.
 *
 * Deliberately NOT touched:
 * - createOffer / createAnswer - they carry no addresses; the transport
 *   policy itself is enforced by the launcher flag
 *   --force-webrtc-ip-handling-policy=relay_only.
 * - RTCPeerConnection.prototype - the filters live on the single instance
 *   created by the wrapper (simpler and safer than a global prototype
 *   interception). The wrapper keeps the native prototype, name, static
 *   generateCertificate and constructor semantics (calling without `new`
 *   throws the native TypeError), so instanceof and feature probes stay
 *   native-looking.
 */
;(function () {
    'use strict';

    // Register a function in the toString-mask registry. Best effort: the
    // spoof must never depend on it.
    function track(fn, name) {
        try {
            var r = window.__MIRAGE_RNG__;
            if (r && typeof r.track === 'function') r.track(fn, name);
        } catch (e) {
            // Mask stays incomplete, nothing else breaks.
        }
    }

    function runWebrtcMode() {
        var MARK = 'webrtc';

        var done = window.__MIRAGE_DONE__;
        if (!done || typeof done !== 'object') {
            done = {};
            window.__MIRAGE_DONE__ = done;
        }
        if (done[MARK]) return;

        // Marked BEFORE patching: the constructor swap below is not
        // double-run-safe (a second run would wrap the wrapper).
        done[MARK] = true;

        var fp = window.__MIRAGE_FP__;
        if (!fp || typeof fp !== 'object') return;

        var mode = fp.webRTC;
        if (mode !== 'off' && mode !== 'relay') {
            // "real" - fully native. Unknown values: stay native, never guess.
            return;
        }

        var OrigPC = null;
        try {
            if (typeof window.RTCPeerConnection === 'function') {
                OrigPC = window.RTCPeerConnection;
            }
        } catch (e) { /* no WebRTC in this runtime */ }
        if (!OrigPC) return;

        // ---- "off": the constructor is gone, like in a WebRTC-less build --
        if (mode === 'off') {
            try { delete window.RTCPeerConnection; } catch (e) { /* nothing to do */ }
            try { delete window.webkitRTCPeerConnection; } catch (e) { /* alias is best effort */ }
            return;
        }

        // ---- "relay" ----------------------------------------------------------

        var PROTO = OrigPC.prototype;

        var origOnIceDesc = null;
        try { origOnIceDesc = Object.getOwnPropertyDescriptor(PROTO, 'onicecandidate'); } catch (e) { }
        var origOnIceSet = (origOnIceDesc && typeof origOnIceDesc.set === 'function') ? origOnIceDesc.set : null;

        var origAdd = null;
        var origRemove = null;
        try {
            if (PROTO && typeof PROTO.addEventListener === 'function') {
                origAdd = PROTO.addEventListener;
            }
        } catch (e) { }
        try {
            if (PROTO && typeof PROTO.removeEventListener === 'function') {
                origRemove = PROTO.removeEventListener;
            }
        } catch (e) { }

        var origGetStats = null;
        try {
            if (PROTO && typeof PROTO.getStats === 'function') {
                origGetStats = PROTO.getStats;
            }
        } catch (e) { }

        // Keep only relay candidates and end-of-candidates notifications.
        // A relay profile must never leak host/srflx addresses. Anything
        // that cannot be verified as relay or end-of-candidates is dropped
        // (fail closed).
        function keepEvent(event) {
            try {
                if (!event) return true;
                var c = event.candidate;
                if (c === null || c === undefined) return true; // end-of-candidates
                var s = c.candidate;
                if (typeof s !== 'string') return false;        // unverifiable: drop
                if (s === '') return true;                      // modern end-of-candidates
                return s.indexOf('typ relay') !== -1;
            } catch (e) {
                return false; // fail closed
            }
        }

        // Rebuild an RTCStatsReport without the records that leak real
        // addresses: non-relay candidate records and the candidate pairs
        // referencing them. Everything else passes through untouched.
        function filterStatsReport(report) {
            try {
                if (!report || typeof report.forEach !== 'function') {
                    return report; // nothing we can inspect: honest report
                }
                var entries = [];
                report.forEach(function (value, key) {
                    entries.push([key, value]);
                });

                // Pass 1: candidate records with a non-relay candidateType.
                var dropped = new Map(); // stat id -> true
                var i, rec, key;
                for (i = 0; i < entries.length; i++) {
                    key = entries[i][0];
                    rec = entries[i][1];
                    if (rec && typeof rec === 'object') {
                        var t = rec.type;
                        if (t === 'local-candidate' || t === 'remote-candidate' || t === 'candidate') {
                            if (rec.candidateType !== 'relay') {
                                dropped.set(String(key), true);
                            }
                        }
                    }
                }
                // Pass 2: candidate pairs referencing a dropped candidate.
                for (i = 0; i < entries.length; i++) {
                    rec = entries[i][1];
                    if (rec && typeof rec === 'object' && rec.type === 'candidate-pair') {
                        var lid = (typeof rec.localCandidateId === 'string') ? rec.localCandidateId : '';
                        var rid = (typeof rec.remoteCandidateId === 'string') ? rec.remoteCandidateId : '';
                        if ((lid && dropped.get(lid)) || (rid && dropped.get(rid))) {
                            dropped.set(String(entries[i][0]), true);
                        }
                    }
                }

                var kept = [];
                for (i = 0; i < entries.length; i++) {
                    if (!dropped.get(String(entries[i][0]))) {
                        kept.push(entries[i]);
                    }
                }
                return makeFilteredReport(kept);
            } catch (e) {
                return report; // never break getStats for the page
            }
        }

        // A report-shaped object: RTCStatsReport.prototype in the chain (so
        // instanceof passes) with get/has/size/forEach/entries/keys/values
        // and iteration over the kept entries, exactly the surface page
        // code and fingerprint libraries read from a native report.
        function makeFilteredReport(kept) {
            var map = new Map();
            for (var i = 0; i < kept.length; i++) map.set(kept[i][0], kept[i][1]);

            var ReportCtor = window.RTCStatsReport;
            var ifaceProto = (ReportCtor && ReportCtor.prototype) ? ReportCtor.prototype : Object.prototype;

            var proto = Object.create(ifaceProto);
            if (ReportCtor) {
                try {
                    Object.defineProperty(proto, 'constructor', {
                        value: ReportCtor,
                        writable: true,
                        enumerable: false,
                        configurable: true
                    });
                } catch (e) { /* cosmetic */ }
            }

            var report = Object.create(proto);

            function defOwn(name, descriptor) {
                try {
                    Object.defineProperty(proto, name, descriptor);
                } catch (e) { /* partial report is still readable */ }
            }

            var sizeGetter = function size() { return map.size; };
            var getFn = function get(id) { return map.get(id); };
            var hasFn = function has(id) { return map.has(id); };
            var forEachFn = function forEach(callbackFn) {
                var thisArg = arguments.length > 1 ? arguments[1] : undefined;
                map.forEach(function (v, k) {
                    if (typeof callbackFn === 'function') {
                        callbackFn.call(thisArg, v, k, report);
                    }
                });
            };
            var entriesFn = function entries() { return map.entries(); };
            var keysFn = function keys() { return map.keys(); };
            var valuesFn = function values() { return map.values(); };

            track(sizeGetter, 'size');
            track(getFn, 'get');
            track(hasFn, 'has');
            track(forEachFn, 'forEach');
            track(entriesFn, 'entries');
            track(keysFn, 'keys');
            track(valuesFn, 'values');

            defOwn('size', { get: sizeGetter, set: undefined, enumerable: true, configurable: true });
            defOwn('get', { value: getFn, writable: true, enumerable: true, configurable: true });
            defOwn('has', { value: hasFn, writable: true, enumerable: true, configurable: true });
            defOwn('forEach', { value: forEachFn, writable: true, enumerable: true, configurable: true });
            defOwn('entries', { value: entriesFn, writable: true, enumerable: true, configurable: true });
            defOwn('keys', { value: keysFn, writable: true, enumerable: true, configurable: true });
            defOwn('values', { value: valuesFn, writable: true, enumerable: true, configurable: true });
            try {
                // Maplike iteration: for (const [id, stat] of report) walks
                // [key, value] pairs, so the iterator is the entries method
                // - exactly the native Map/RTCStatsReport shape.
                Object.defineProperty(proto, Symbol.iterator, {
                    value: entriesFn,
                    writable: true,
                    enumerable: false,
                    configurable: true
                });
            } catch (e) { /* iteration is optional sugar */ }

            return report;
        }

        function RTCPeerConnection() {
            if (!new.target) {
                // Native classes cannot be called without `new` - keep that.
                throw new TypeError("Failed to construct 'RTCPeerConnection': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
            }
            var rawArgs = arguments;
            var args = [];
            for (var i = 0; i < rawArgs.length; i++) args.push(rawArgs[i]);

            // Real instance with native behaviour (native TypeErrors for a
            // bad RTCConfiguration propagate unchanged).
            var pc = Reflect.construct(OrigPC, args, new.target || RTCPeerConnection);

            // ---- onicecandidate: instance-level get/set --------------------
            if (origOnIceSet) {
                try {
                    var userHandler = null;
                    var onIceGet = function () { return userHandler; };
                    var onIceSet = function (handler) {
                        if (typeof handler === 'function') {
                            userHandler = handler;
                            // Register the filter through the native
                            // prototype setter, so the native dispatch
                            // actually reaches it.
                            origOnIceSet.call(pc, function (event) {
                                if (keepEvent(event)) {
                                    return handler.call(this, event);
                                }
                                return undefined;
                            });
                        } else {
                            userHandler = null;
                            origOnIceSet.call(pc, handler); // null/cleanup: native path
                        }
                    };
                    track(onIceGet, 'onicecandidate');
                    track(onIceSet, 'onicecandidate');
                    Object.defineProperty(pc, 'onicecandidate', {
                        get: onIceGet,
                        set: onIceSet,
                        enumerable: false,
                        configurable: true
                    });
                } catch (e) {
                    // Native handler property stays; listeners still filter.
                }
            }

            // ---- addEventListener / removeEventListener for icecandidate --
            if (origAdd) {
                try {
                    var listenerMap = new Map(); // original listener -> wrapped
                    var addFn = function addEventListener(type, listener) {
                        try {
                            if (type === 'icecandidate') {
                                var isFn = typeof listener === 'function';
                                var isObj = listener && typeof listener === 'object' &&
                                    typeof listener.handleEvent === 'function';
                                if (isFn || isObj) {
                                    if (listenerMap.has(listener)) {
                                        return; // native dedupe on re-add
                                    }
                                    var wrapped;
                                    if (isFn) {
                                        wrapped = function (event) {
                                            if (keepEvent(event)) {
                                                return listener.call(this, event);
                                            }
                                            return undefined;
                                        };
                                    } else {
                                        var objListener = listener;
                                        wrapped = {
                                            handleEvent: function (event) {
                                                if (keepEvent(event)) {
                                                    return objListener.handleEvent(event);
                                                }
                                                return undefined;
                                            }
                                        };
                                    }
                                    listenerMap.set(listener, wrapped);
                                    var opts = arguments.length > 2 ? arguments[2] : undefined;
                                    return origAdd.call(pc, type, wrapped, opts);
                                }
                            }
                        } catch (e) {
                            // fall through to the native path
                        }
                        return origAdd.apply(pc, arguments);
                    };
                    track(addFn, 'addEventListener');
                    Object.defineProperty(pc, 'addEventListener', {
                        value: addFn,
                        writable: true,
                        enumerable: false,
                        configurable: true
                    });
                    if (origRemove) {
                        var removeFn = function removeEventListener(type, listener) {
                            try {
                                if (type === 'icecandidate') {
                                    var wrapped = listenerMap.get(listener);
                                    if (wrapped) {
                                        listenerMap.delete(listener);
                                        var opts = arguments.length > 2 ? arguments[2] : undefined;
                                        return origRemove.call(pc, type, wrapped, opts);
                                    }
                                }
                            } catch (e) {
                                // fall through to the native path
                            }
                            return origRemove.apply(pc, arguments);
                        };
                        track(removeFn, 'removeEventListener');
                        Object.defineProperty(pc, 'removeEventListener', {
                            value: removeFn,
                            writable: true,
                            enumerable: false,
                            configurable: true
                        });
                    }
                } catch (e) {
                    // Listeners stay native; onicecandidate still filters.
                }
            }

            // ---- getStats: instance-level report filter (B2) ----------------
            if (origGetStats) {
                try {
                    var getStatsFn = function getStats() {
                        var res = origGetStats.apply(this, arguments);
                        try {
                            if (res && typeof res.then === 'function') {
                                // Promise semantics preserved: the native
                                // promise, chained through the filter;
                                // rejections pass through untouched.
                                return res.then(filterStatsReport);
                            }
                        } catch (e) {
                            // honest result
                        }
                        return res;
                    };
                    track(getStatsFn, 'getStats');
                    Object.defineProperty(pc, 'getStats', {
                        value: getStatsFn,
                        writable: true,
                        enumerable: false,
                        configurable: true
                    });
                } catch (e) {
                    // Native getStats stays; onicecandidate/listeners filter.
                }
            }

            return pc;
        }

        try { RTCPeerConnection.prototype = PROTO; } catch (e) { /* non-standard engine */ }
        try {
            Object.defineProperty(RTCPeerConnection, 'name', {
                value: 'RTCPeerConnection',
                configurable: true
            });
        } catch (e) { /* name is cosmetic */ }
        try {
            if (typeof OrigPC.generateCertificate === 'function') {
                RTCPeerConnection.generateCertificate = OrigPC.generateCertificate;
            }
        } catch (e) { /* static is cosmetic */ }
        track(RTCPeerConnection, 'RTCPeerConnection');

        window.RTCPeerConnection = RTCPeerConnection;
        try {
            if (typeof window.webkitRTCPeerConnection === 'function') {
                window.webkitRTCPeerConnection = RTCPeerConnection;
            }
        } catch (e) { /* alias is best effort */ }
    }

    // ---- Function.prototype.toString: deliberately NOT patched ------------
    // Every page-level spoofed method/getter is now built with
    // __MIRAGE_RNG__.makeNative (a Proxy over an arrow), so the engine's OWN
    // Function.prototype.toString already answers "function () { [native
    // code] }" for every one of them - in every realm, with no patch needed.
    //
    // A central Function.prototype.toString patch used to exist here. Live
    // creepjs runs proved it is a net loss: the prototype lie detector tests
    // Function.prototype.toString itself and, because any JS replacement is
    // (rightly) not native, records it as a lie. That single lie flips the
    // detector's `detectProxies` gate ON for the whole page, which then runs
    // an extra proxy battery (`Object.setPrototypeOf(fn, Object.create(fn))
    // .toString()`, `fn.__proto__ = proxy; fn++`, `Reflect.setPrototypeOf`
    // ...) against EVERY function - and Proxy-backed wrappers cannot satisfy
    // those, so each one took three fresh lies. A plain-function mask fared
    // no better: it fails the eight native-shape checks instead. There is no
    // JS mask that passes both batteries; the proxies make the mask redundant,
    // so the honest native toString is kept. The registry (track /
    // nativeName) is retained for the worker snippet and diagnostics.
    //
    // Known consequence: the remaining PLAIN-function wrappers that must stay
    // constructable (Blob / Worker / SharedWorker / RTCPeerConnection) are not
    // masked and would show their source if a detector stringified them; none
    // of the calibration targets do.
    function installToStringLayer() {
        // Intentionally a no-op: keep Function.prototype.toString native.
    }

    try {
        runWebrtcMode();
    } catch (e) {
        // Contract: never leak anything to the page. The mode handling
        // failed - fall back to native WebRTC, but still run the mask.
    }
    try {
        installToStringLayer();
    } catch (e) {
        // Contract: never leak anything to the page.
    }
})();

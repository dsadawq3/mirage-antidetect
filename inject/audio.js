/*
 * Mirage inject layer - module "audio".
 *
 * Intercepts AnalyserNode.prototype.getFloatFrequencyData and
 * AnalyserNode.prototype.getByteFrequencyData. AnalyserNode is a common
 * readout point of AudioContext fingerprints, and the prototype is
 * shared by AudioContext and OfflineAudioContext (both create analyser
 * nodes through createAnalyser), so patching it covers both context
 * types. A tiny deterministic perturbation keyed by (audioSeed, bin
 * index) is added to every element:
 *   - float path: value += (noise - 0.5) * 0.0001  (invisible, bit-stable)
 *   - byte path:  +/-1 with probability 5%, clamped to [0, 255] (a raw
 *     Uint8Array store would silently wrap 255 + 1 into 0)
 *
 * Additionally covers the STANDARD offline audio fingerprint
 * (OfflineAudioContext + DynamicsCompressor + getChannelData), which the
 * analyser patch cannot see: OfflineAudioContext.prototype.startRendering
 * is wrapped and the rendered AudioBuffer's channel data gets a
 * deterministic Float32 perturbation of +/-1e-7, keyed by
 * (audioSeed, channelIndex, sampleIndex). NaN/Infinity samples are left
 * untouched. Both result forms are handled: the modern promise (the
 * resolved buffer is noised before the page sees it) and the legacy
 * callback style (an `oncomplete` event whose renderedBuffer is noised
 * before the page handler runs), should the runtime still have one.
 * The same sample always gets the same delta, so repeated renders of
 * the same graph are bit-identical, while the resulting hash differs
 * from every other profile.
 *
 * Every installed wrapper is built with window.__MIRAGE_RNG__.makeNative so
 * it is observationally native (own keys length,name; no own prototype;
 * non-constructable; "[native code]" from any realm's
 * Function.prototype.toString) - the prototype lie detectors scan
 * AnalyserNode (all members) and break every plain-function wrapper.
 *
 * Deliberately NOT intercepted (breaking these breaks real audio and is a
 * known detection vector - see inject/README.md):
 *   - DynamicsCompressorNode - the compressor must behave like the real
 *     device; a manipulated transfer curve is measurable by rendering the
 *     graph and comparing envelope shapes.
 *   - AudioBuffer.prototype.getChannelData (direct calls) - this is the
 *     data real playback and processing run on; perturbing every handed
 *     out array audibly corrupts legitimate audio. The offline-render
 *     readout is covered through startRendering instead.
 *   - AudioContext.prototype.sampleRate - the sample rate is expected to
 *     match the OS audio stack and is trivially cross-checked
 *     (OfflineAudioContext, MediaDevices); it must stay real.
 */
;(function () {
    'use strict';
    try {
        if (typeof window === 'undefined') return;

        if (typeof window.__MIRAGE_DONE__ === 'undefined') {
            window.__MIRAGE_DONE__ = {};
        }
        var done = window.__MIRAGE_DONE__;
        if (!done || typeof done !== 'object' || done['audio']) return;

        var fp = window.__MIRAGE_FP__;
        if (!fp || typeof fp !== 'object') return;
        if (typeof fp.audioSeed !== 'number') return;

        var rng = window.__MIRAGE_RNG__;
        if (!rng || typeof rng.noise !== 'function' || typeof rng.int !== 'function') return;

        var seed = fp.audioSeed | 0;
        var FLOAT_AMP = 0.0001;   // max |delta| on the float path is half of it
        var BYTE_RATE = 0.05;     // share of byte bins that get a +/-1 shift
        var RENDER_AMP = 2e-7;    // render path: delta in (-1e-7, +1e-7)

        var makeNative = (typeof rng.makeNative === 'function')
            ? rng.makeNative
            : function (name, length, impl) {
                var f = function () {
                    var a = [];
                    for (var i = 0; i < arguments.length; i++) a.push(arguments[i]);
                    return impl(this, a);
                };
                try { Object.defineProperty(f, 'name', { value: name, configurable: true }); } catch (e) { }
                return f;
            };

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

        // Replace a prototype method with a native-looking wrapper around its
        // current value. Only plain, configurable, function-valued data
        // properties are touched.
        function patchMethod(proto, prop, makeImpl) {
            try {
                var d = Object.getOwnPropertyDescriptor(proto, prop);
                if (!d) return false;
                if (d.get !== undefined || d.set !== undefined) return false;
                if (typeof d.value !== 'function') return false;
                if (d.configurable === false) return false;
                var orig = d.value;
                var w = makeNative(prop, orig.length, makeImpl(orig));
                if (typeof w !== 'function') return false;
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

        function makeGetFloatFrequencyData(orig) {
            return function (thisArg, args) {
                var array = args[0];
                orig.apply(thisArg, args);
                try {
                    if (array && typeof array.length === 'number') {
                        var n = array.length;
                        for (var i = 0; i < n; i++) {
                            array[i] = array[i] + (rng.noise(seed, i, 0) - 0.5) * FLOAT_AMP;
                        }
                    }
                } catch (e) {
                    // The noise step must never break the read.
                }
            };
        }

        function makeGetByteFrequencyData(orig) {
            return function (thisArg, args) {
                var array = args[0];
                orig.apply(thisArg, args);
                try {
                    if (array && typeof array.length === 'number') {
                        var n = array.length;
                        for (var i = 0; i < n; i++) {
                            if (rng.noise(seed, i, 0) < BYTE_RATE) {
                                var v = array[i] + (rng.int(seed, i, 0, 2) === 0 ? -1 : 1);
                                if (v < 0) v = 0;
                                else if (v > 255) v = 255;
                                array[i] = v;
                            }
                        }
                    }
                } catch (e) {
                    // The noise step must never break the read.
                }
            };
        }

        // ---- offline render noise (standard audio fingerprint) --------------

        function isAudioBufferLike(x) {
            try {
                return !!x && typeof x === 'object' &&
                    typeof x.numberOfChannels === 'number' &&
                    typeof x.getChannelData === 'function';
            } catch (e) {
                return false;
            }
        }

        // Deterministic per-sample perturbation of every channel of a
        // rendered AudioBuffer. NaN/Infinity samples are left as they are.
        function noiseAudioBuffer(buffer) {
            try {
                if (!isAudioBufferLike(buffer)) return buffer;
                var channels = buffer.numberOfChannels | 0;
                for (var c = 0; c < channels; c++) {
                    var data;
                    try { data = buffer.getChannelData(c); } catch (e) { continue; }
                    if (!data || typeof data.length !== 'number') continue;
                    var n = data.length;
                    for (var i = 0; i < n; i++) {
                        var v = data[i];
                        if (typeof v !== 'number' || !isFinite(v)) continue;
                        data[i] = v + (rng.noise(seed, c, i) - 0.5) * RENDER_AMP;
                    }
                }
            } catch (e) {
                // Never break the render result.
            }
            return buffer;
        }

        // Legacy callback-style runtimes deliver the buffer through the
        // `complete` event / oncomplete handler; wrap the instance handler
        // once so the renderedBuffer is noised before the page sees it.
        var legacyWrapped = new WeakSet();
        function wrapLegacyOnComplete(ctx) {
            try {
                if (!ctx || typeof ctx !== 'object' || legacyWrapped.has(ctx)) return;
                var Ctor = window.OfflineAudioContext;
                var proto = Ctor && Ctor.prototype;
                if (!proto) return;
                var desc = Object.getOwnPropertyDescriptor(proto, 'oncomplete');
                if (!desc || typeof desc.set !== 'function') return;
                legacyWrapped.add(ctx);
                var userHandler = null;
                Object.defineProperty(ctx, 'oncomplete', {
                    get: function () { return userHandler; },
                    set: function (handler) {
                        if (typeof handler === 'function') {
                            userHandler = handler;
                            desc.set.call(ctx, function (event) {
                                try { noiseAudioBuffer(event && event.renderedBuffer); } catch (e) { }
                                return handler.call(this, event);
                            });
                        } else {
                            userHandler = null;
                            desc.set.call(ctx, handler);
                        }
                    },
                    enumerable: false,
                    configurable: true
                });
            } catch (e) {
                // Best effort: legacy path only.
            }
        }

        function makeStartRendering(orig) {
            return function (thisArg, args) {
                var result = orig.apply(thisArg, args);
                try {
                    if (result && typeof result.then === 'function') {
                        // Modern form: promise resolving the AudioBuffer.
                        // Rejections pass through untouched.
                        return result.then(noiseAudioBuffer);
                    }
                    if (isAudioBufferLike(result)) {
                        // Ancient form: the buffer returned directly.
                        return noiseAudioBuffer(result);
                    }
                    if (typeof result === 'undefined') {
                        // Legacy callback form: buffer arrives via the
                        // `complete` event.
                        wrapLegacyOnComplete(thisArg);
                    }
                } catch (e) {
                    // The noise step must never break rendering.
                }
                return result;
            };
        }

        if (typeof AnalyserNode !== 'undefined' && AnalyserNode.prototype) {
            patchMethod(AnalyserNode.prototype, 'getFloatFrequencyData', makeGetFloatFrequencyData);
            patchMethod(AnalyserNode.prototype, 'getByteFrequencyData', makeGetByteFrequencyData);
        }

        if (typeof OfflineAudioContext !== 'undefined' && OfflineAudioContext.prototype) {
            patchMethod(OfflineAudioContext.prototype, 'startRendering', makeStartRendering);
        }

        done['audio'] = true;
    } catch (e) {
        // Contract: never leak anything to the page.
    }
})();

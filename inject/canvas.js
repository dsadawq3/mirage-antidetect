/*
 * Mirage inject layer - module "canvas".
 *
 * Intercepts CanvasRenderingContext2D.prototype.getImageData (and the
 * OffscreenCanvasRenderingContext2D variant when the runtime has it) and
 * deterministically perturbs a SPARSE, seed-distributed subset of the
 * returned pixels by one LSB. The decision, the channel and the sign are
 * keyed by (canvasSeed, absolute pixel x, absolute pixel y) through
 * window.__MIRAGE_RNG__, so:
 *   - reading the same region twice returns bit-identical bytes
 *     (stability: fingerprint pages re-read the canvas and must not see
 *     the noise flicker),
 *   - the same drawing under a different profile yields different bytes
 *     (uniqueness), while the perturbation stays invisible to the eye.
 *
 * Absolute canvas coordinates (sx + local x, sy + local y) are used as the
 * noise key so a pixel keeps the same noise decision whether it is read as
 * part of a full getImageData(0, 0, w, h) sweep or of a smaller rectangle.
 *
 * Virgin-pixel rule: a pixel whose RGBA quadruple is all 0x00 or all 0xFF
 * is NEVER noised. A blank canvas must read back as blank - native Chrome
 * returns all-zero bytes for a virgin 2D canvas, and non-zero noise there
 * is an immediate tell (noise on "nothing" cannot come from real
 * hardware). All other pixels keep the deterministic noise below.
 *
 * Quiet-corner rule (added after a live creepjs run): a "drawn vs read"
 * lie detector (creepjs `getPixelMods`) paints an 8x8 matrix of solid 1x1
 * fills and then reads every pixel back with `getImageData(x, y, 1, 1)`,
 * flagging the canvas whenever the read byte differs from the byte that was
 * drawn. ANY per-pixel perturbation of a freshly drawn flat fill is caught
 * by that comparison (it is drawn-value vs read-value, not a re-read), so
 * the ONLY JS-side way to stay clean there is to leave the exact pixels the
 * detector probes untouched. A pixel inside the top-left QUIET x QUIET
 * block is therefore never noised. The rule is a pure function of absolute
 * coordinates, so it keeps the full-read / sub-read consistency contract
 * (rule 2 below) intact: the corner is simply a fixed, noiseless region.
 * This defeats the specific detector but is a tactical trade-off - the
 * definitive fix is native (patches/), perturbing inside the renderer so
 * both the drawn and the read bytes agree.
 *
 * The original method is always called first with the original arguments,
 * so native exceptions (IndexSizeError, SecurityError on a tainted canvas,
 * TypeError on illegal receivers) propagate exactly as before; the noise
 * step itself can never throw out of the wrapper.
 *
 * Every installed wrapper is built with window.__MIRAGE_RNG__.makeNative,
 * so it is observationally native (own keys length,name; no prototype; not
 * constructable; "[native code]" from any realm's Function.prototype
 * .toString) and is additionally registered in the toString-mask registry.
 *
 * Deliberately NOT intercepted (project contract, see inject/README.md):
 *   - HTMLCanvasElement.prototype.toDataURL / toBlob - a JS wrapper cannot
 *     re-encode a still-valid PNG/JPEG without an async decode/re-encode,
 *     and editing the produced data: URL string breaks its validity. The
 *     native layer (patches/) must route the platform encoder through the
 *     same noised pixel path; the JS layer only covers the direct pixel
 *     readout (getImageData).
 *   - HTMLCanvasElement.prototype.getContext - returning a proxy context
 *     breaks instanceof and native calls; noise at the readout APIs covers
 *     the same fingerprint surface with far less exposure.
 */
;(function () {
    'use strict';
    try {
        if (typeof window === 'undefined') return;

        if (typeof window.__MIRAGE_DONE__ === 'undefined') {
            window.__MIRAGE_DONE__ = {};
        }
        var done = window.__MIRAGE_DONE__;
        if (!done || typeof done !== 'object' || done['canvas']) return;

        var fp = window.__MIRAGE_FP__;
        if (!fp || typeof fp !== 'object') return;
        if (typeof fp.canvasSeed !== 'number') return;

        var rng = window.__MIRAGE_RNG__;
        if (!rng || typeof rng.noise !== 'function' || typeof rng.int !== 'function') return;

        var seed = fp.canvasSeed | 0;
        var RATE = 0.015; // share of (non-quiet) pixels that get a one-bit flip
        var QUIET = 8;    // top-left QUIET x QUIET block is never perturbed

        // Native-looking wrapper factory; degrade to a plain wrapper when the
        // shared helper is missing (an older bundle) so the spoof still works.
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
        // current value. Descriptor checks: only plain, configurable,
        // function-valued data properties are touched, so we never wrap an
        // accessor, a sealed property, or (on a run without a marker) an
        // existing wrapper. `makeImpl(orig)` returns the wrapper body
        // (thisArg, args) -> result.
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

        // True for a pixel the noise must leave alone: a virgin quadruple
        // (all 0x00 or all 0xFF). Blank canvases read back blank.
        function isVirginPixel(d, o) {
            var v0 = d[o], v1 = d[o + 1], v2 = d[o + 2], v3 = d[o + 3];
            return (v0 === 0 && v1 === 0 && v2 === 0 && v3 === 0) ||
                   (v0 === 255 && v1 === 255 && v2 === 255 && v3 === 255);
        }

        function makeGetImageData(orig) {
            return function (thisArg, args) {
                var sx = args[0], sy = args[1];
                // All arguments are forwarded untouched (including the
                // optional ImageDataSettings fifth argument).
                var img = orig.apply(thisArg, args);
                try {
                    if (img && img.data) {
                        var d = img.data;
                        var width = img.width | 0;
                        var height = img.height | 0;
                        var baseX = (typeof sx === 'number') ? sx : 0;
                        var baseY = (typeof sy === 'number') ? sy : 0;
                        for (var yy = 0; yy < height; yy++) {
                            var row = yy * width;
                            var ay = baseY + yy;
                            for (var xx = 0; xx < width; xx++) {
                                var ax = baseX + xx;
                                // Quiet corner: keep detector probes bit-exact.
                                if (ax < QUIET && ay < QUIET) continue;
                                if (rng.noise(seed, ax, ay) < RATE) {
                                    var o = (row + xx) << 2;
                                    if (isVirginPixel(d, o)) continue;
                                    d[o + rng.int(seed, ax, ay, 4)] ^= 1;
                                }
                            }
                        }
                    }
                } catch (e) {
                    // The noise step must never break the read; the original
                    // result is returned as-is.
                }
                return img;
            };
        }

        if (typeof CanvasRenderingContext2D !== 'undefined' && CanvasRenderingContext2D.prototype) {
            patchMethod(CanvasRenderingContext2D.prototype, 'getImageData', makeGetImageData);
        }
        if (typeof OffscreenCanvasRenderingContext2D !== 'undefined' && OffscreenCanvasRenderingContext2D.prototype) {
            patchMethod(OffscreenCanvasRenderingContext2D.prototype, 'getImageData', makeGetImageData);
        }

        done['canvas'] = true;
    } catch (e) {
        // Contract: never leak anything to the page.
    }
})();

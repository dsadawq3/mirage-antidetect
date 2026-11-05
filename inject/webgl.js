/*
 * Mirage inject layer - module "webgl".
 *
 * Intercepts WebGLRenderingContext.prototype and
 * WebGL2RenderingContext.prototype (when present):
 *
 *   getParameter(pname) - fingerprint-consistent values:
 *       UNMASKED_VENDOR_WEBGL / UNMASKED_RENDERER_WEBGL from the profile,
 *       the masked VENDOR / RENDERER / VERSION / SHADING_LANGUAGE_VERSION
 *       strings a real Chrome reports, and MAX_TEXTURE_SIZE /
 *       MAX_RENDERBUFFER_SIZE / MAX_VIEWPORT_DIMS from the profile.
 *       Everything else (including all WebGL2-only parameters) falls
 *       through to the original via .call(this, pname), so no capability
 *       lie is introduced beyond the profile. The pname is matched after
 *       ToUint32 coercion (>>> 0), exactly like the WebIDL GLenum
 *       conversion, so string/float enum values cannot smuggle the real
 *       GPU strings out. Spoofed answers are only given to receivers that
 *       pass instanceof for the patched interface; anything else falls to
 *       the original call and reproduces the native "Illegal invocation"
 *       TypeError.
 *
 *   getExtension(name) - explicit pass-through. The
 *       "WEBGL_debug_renderer_info" object must come from the real browser
 *       (or be null when the GPU blocklist disables it); fabricating the
 *       extension object is a known detection vector, so it is never done.
 *
 *   readPixels(x, y, w, h, format, type, dstData, offset) - after the
 *       original call, RGBA/UNSIGNED_BYTE typed-array destinations get the
 *       same deterministic LSB noise as the 2D canvas path (~2% of pixels,
 *       one channel, keyed by (canvasSeed, pixel x, pixel y)). Virgin
 *       pixels (RGBA all 0x00 or all 0xFF) are never noised: an empty
 *       WebGL framebuffer must read back all-zero, like native Chrome.
 *       Other formats/types and WebGL2 PIXEL_PACK_BUFFER reads (numeric
 *       target argument) are left untouched.
 *
 * Every installed wrapper is built with window.__MIRAGE_RNG__.makeNative so
 * it is observationally native (own keys length,name; no own prototype;
 * non-constructable; "[native code]" from any realm's Function.prototype
 * .toString) - the prototype lie detectors walk WebGLRenderingContext /
 * WebGL2RenderingContext getParameter/readPixels/bufferData and break every
 * plain-function wrapper.
 *
 * Deliberately NOT intercepted: getSupportedExtensions (spoofing it adds
 * inconsistency risk for zero fingerprint value) and getError (calling it
 * from a wrapper would consume the page's GL error state).
 */
;(function () {
    'use strict';
    try {
        if (typeof window === 'undefined') return;

        if (typeof window.__MIRAGE_DONE__ === 'undefined') {
            window.__MIRAGE_DONE__ = {};
        }
        var done = window.__MIRAGE_DONE__;
        if (!done || typeof done !== 'object' || done['webgl']) return;

        var fp = window.__MIRAGE_FP__;
        if (!fp || typeof fp !== 'object') return;

        var glCfg = (fp.webgl && typeof fp.webgl === 'object') ? fp.webgl : null;
        var seed = (typeof fp.canvasSeed === 'number') ? (fp.canvasSeed | 0) : null;
        if (!glCfg && seed === null) return; // nothing to spoof at all

        var rng = window.__MIRAGE_RNG__;
        if (seed !== null && (!rng || typeof rng.noise !== 'function' || typeof rng.int !== 'function')) {
            seed = null; // degrade gracefully: parameter spoof still installs
        }

        var READ_RATE = 0.02; // share of readPixels pixels that get a flip

        var makeNative = (rng && typeof rng.makeNative === 'function')
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

        // Masked strings exactly as a real Chrome 156 reports them.
        var MASKED_VENDOR = 'WebKit';
        var MASKED_RENDERER = 'WebKit WebGL';
        var VERSION_WGL2 = 'WebGL 2.0 (OpenGL ES 3.0 Chromium)';
        var VERSION_WGL1 = 'WebGL 1.0 (OpenGL ES 2.0 Chromium)';
        var GLSL_WGL2 = 'WebGL GLSL ES 3.00 (OpenGL ES GLSL ES 3.0 Chromium)';
        var GLSL_WGL1 = 'WebGL GLSL ES 1.00 (OpenGL ES GLSL ES 1.0 Chromium)';

        // WebGL enum values used below.
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

        // ctor is the interface constructor: the instanceof receiver check
        // needs a constructor, while patchMethod needs the prototype.
        function makeGetParameter(ctor, isWebGL2, orig) {
            return function (thisArg, args) {
                var pname = args[0];
                try {
                    // ToUint32 coercion mirrors the WebIDL GLenum conversion.
                    var p = pname >>> 0;
                    if (p === UNMASKED_VENDOR_WEBGL || p === UNMASKED_RENDERER_WEBGL ||
                        p === GL_VENDOR || p === GL_RENDERER || p === GL_VERSION ||
                        p === GL_SHADING_LANGUAGE_VERSION || p === GL_MAX_TEXTURE_SIZE ||
                        p === GL_MAX_RENDERBUFFER_SIZE || p === GL_MAX_VIEWPORT_DIMS) {
                        // Only answer for a real receiver of this interface;
                        // everything else must keep native behavior.
                        if (thisArg instanceof ctor) {
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
                                if (glCfg && Array.isArray(glCfg.maxViewportDims) && glCfg.maxViewportDims.length >= 2) {
                                    // Fresh object per call, like the original.
                                    return new Int32Array([glCfg.maxViewportDims[0] | 0, glCfg.maxViewportDims[1] | 0]);
                                }
                            }
                        }
                    }
                } catch (e) {
                    // Fall through to the original call below.
                }
                return orig.apply(thisArg, args);
            };
        }

        function makeGetExtension(orig) {
            return function (thisArg, args) {
                // Pure pass-through: the real extension object (or null) is
                // always the correct answer - never fabricate one.
                return orig.apply(thisArg, args);
            };
        }

        function makeReadPixels(orig) {
            return function (thisArg, args) {
                var x = args[0], y = args[1], width = args[2], height = args[3];
                var format = args[4], type = args[5], dstData = args[6];
                orig.apply(thisArg, args);
                try {
                    if (seed === null) return;
                    // Coerce exactly like the WebIDL GLenum conversion.
                    if ((format >>> 0) !== GL_RGBA || (type >>> 0) !== GL_UNSIGNED_BYTE) return;
                    // Only typed-array destinations are noised; a numeric
                    // 7th argument means a WebGL2 PIXEL_PACK_BUFFER read.
                    if (!dstData || typeof dstData !== 'object' || dstData.BYTES_PER_ELEMENT !== 1) return;
                    var dstOffset = args.length > 7 ? args[7] : 0;
                    var base = (typeof dstOffset === 'number' && dstOffset > 0) ? dstOffset : 0;
                    var total = (width * height) * 4;
                    var lim = dstData.length;
                    for (var i = 0; i < total; i += 4) {
                        var px = x + ((i >> 2) % width);
                        var py = y + (((i >> 2) / width) | 0);
                        if (rng.noise(seed, px, py) < READ_RATE) {
                            var o = base + i;
                            // Virgin pixels (RGBA all 0x00 or all 0xFF) read
                            // back untouched - an empty framebuffer is empty
                            // in native Chrome.
                            var v0 = dstData[o], v1 = dstData[o + 1];
                            var v2 = dstData[o + 2], v3 = dstData[o + 3];
                            if ((v0 === 0 && v1 === 0 && v2 === 0 && v3 === 0) ||
                                (v0 === 255 && v1 === 255 && v2 === 255 && v3 === 255)) {
                                continue;
                            }
                            var di = o + rng.int(seed, px, py, 4);
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

        // ctor/proto are captured per call, so the receiver check and the
        // WebGL2 flag inside the getParameter closure are bound to the
        // right interface.
        function installOnPrototype(ctor, proto, isWebGL2) {
            patchMethod(proto, 'getParameter', function (orig) {
                return makeGetParameter(ctor, isWebGL2, orig);
            });
            patchMethod(proto, 'getExtension', makeGetExtension);
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

        done['webgl'] = true;
    } catch (e) {
        // Contract: never leak anything to the page.
    }
})();

# patches/ — Mirage native Chromium patch layer

Native source patches for Chromium. This is the **second, reinforcing spoofing
layer**; it exists only for the signals the JS inject layer (`inject/`, applied
over CDP) cannot close. Patches here require a custom Chromium build — see
[`scripts/build_chromium.md`](../scripts/build_chromium.md).

## Two-layer strategy

| Layer | Mechanism | Maintenance cost | Coverage |
|---|---|---|---|
| 1. JS inject (`inject/`) | `Page.addScriptToEvaluateOnNewDocument` before page scripts in every document | None — tracks the user's stock Chrome, updated with the repo | navigator, screen, time, plugins, client hints, canvas *read* (`getImageData`), WebGL parameters + `readPixels`, audio, WebRTC |
| 2. Native patches (this directory) | Source patches, own Chromium build | Rebuild + anchor refresh on every version bump | Only what JS physically cannot do (below) |

Rule: a signal moves from layer 1 to layer 2 only when JS spoofing is
structurally impossible or detectable. Every native patch is a permanent
maintenance liability, so this directory stays deliberately small.

## What the native layer closes

| Patch | Signal | Why JS cannot close it |
|---|---|---|
| `0001-fonts-whitelist.patch` | Font enumeration (measurement-based: `span`-width probing, `document.fonts.check`, `local()` in `@font-face`) | A probe measures *rendered glyph metrics* inside the layout engine. A JS-patched `measureText`/font list disagrees with real rendering on the first fallback measurement — the lie is the signal. Only the font resolution path itself can make a non-whitelisted font behave exactly like an absent one. |
| `0002-canvas-serialization-noise.patch` | `toDataURL` / `toBlob` / `OffscreenCanvas.convertToBlob` serialization | A JS wrapper cannot perturb the bytes *inside* the platform encoder: editing the produced data-URL string breaks its validity, and decode + re-encode from JS is async, slow, and detectable by timing. The native patch applies the same deterministic noise at the encoder input, so the encoded PNG is *valid* and *stable*, and worker contexts (which the JS inject never reaches) are covered too. |
| `0003-automation-flags-hardening.patch` | Automation internals: `navigator.webdriver` native getter, the `AutomationControlled` Blink runtime feature, the `--enable-automation` switch | The launcher already passes `--disable-blink-features=AutomationControlled` and `inject/navigator.js` overrides `navigator.webdriver`. The native patch is the backstop: nothing (embedder preferences, a harness re-enabling the feature, an external tool passing the switch) can light the automation indicators up while a Mirage profile is active. |

**Everything else stays in the JS layer** (UA, languages, timezone, locale,
screen, `hardwareConcurrency`, `deviceMemory`, WebGL vendor/renderer,
audio read noise, WebRTC modes, plugins, client hints) — see
[`docs/CONTRACT.md`](../docs/CONTRACT.md) and [`inject/README.md`](../inject/README.md).
Be careful adding more here: each new native patch must justify its rebuild cost.

## Target tag

**Chromium `156.0.8078.80`** — the line matching the Chrome installed on the
reference machine and the fingerprint calibration in `docs/CONTRACT.md`
(`CHROME_FULL = "156.0.8078.80"`). The engine version must match the legend:
a 131 binary behind a 156 fingerprint is itself a detectable contradiction.

The anchors and signatures were drafted against the 131 source layout; the
working target is the 156 line. On the first apply against a 156 checkout
expect signature refusals — that is by design (see "First application"
below); realign each refused anchor using the printed context dumps.

## MIRAGE_FP_CONFIG

The patches are driven by one environment variable:

```
MIRAGE_FP_CONFIG = <absolute path to a file containing the Fingerprint JSON>
```

- The JSON schema is **identical to `window.__MIRAGE_FP__`** (`docs/CONTRACT.md`,
  "Схема Fingerprint", camelCase). The same object the launcher injects for the
  JS layer is written to a file and pointed to by this variable.
- Read **once per Chromium process** at first use (cached in function-local
  statics) — before any page can trigger the gated paths.
- The file must be machine-generated canonical JSON (launcher output): the
  parsers inside the patches are minimal scanners (`"fonts"` array,
  `"canvasSeed"` number), by design — Blink code must not grow a JSON library
  dependency for this. No nested objects inside the `fonts` array.
- Field consumption by the native layer:
  - `fonts` — **only** by patch 0001 (feeds the Blink font whitelist);
  - `canvasSeed` — by patch 0002 (seeds the serialization noise; the per-pixel
    decisions are a bit-exact C++ port of `inject/rng.js`, so a PNG decoded
    from `toDataURL` matches a `getImageData(0, 0, w, h)` read of the same
    canvas — the "same noised pixel path" required by `inject/README.md`);
  - mere *presence* of the variable — by patch 0003 (activation switch).
- **Integration requirement for `mirage-launcher`:** when spawning the patched
  build, export `MIRAGE_FP_CONFIG` in the child environment. The stock-Chrome
  path (no patched binary) ignores the variable entirely.
- Without the variable every patch is inert — the patched binary behaves
  exactly like stock Chromium. Delete the variable to A/B-test a profile.

## Patch file format (v1)

A `.patch` file is a Mirage patch template: a header plus a sequence of
anchored replacement (or manual) blocks. `#` comment lines are ignored outside
text captures.

```
# comment
MIRAGE-PATCH: 0001                    # required, first directive
MIRAGE-TITLE: short title             # required
MIRAGE-TARGET: 156.0.8078.80          # required: Chromium tag the patch targets
MIRAGE-DESCRIPTION: one line          # optional, repeatable

MIRAGE-BLOCK                          # start of a replacement block
MIRAGE-FILE: third_party/blink/...    # optional pinned file (forward slashes, relative to the Chromium src/ root)
MIRAGE-SIGNATURE: ^SomeFunc\(         # required: Python regex, must match exactly once
MIRAGE-VERIFY: note for the operator  # optional, repeatable, echoed in the report
MIRAGE-REPLACE-FROM:                  # exact existing text (verbatim, incl. blank lines)
<existing Chromium lines>
MIRAGE-REPLACE-TO:                    # replacement text (empty allowed = deletion)
<replacement lines>
MIRAGE-ENDBLOCK

MIRAGE-MANUAL                         # start of a manual-action block
MIRAGE-FILE: third_party/blink/...    # file the action concerns (informational)
MIRAGE-VERIFY: ...                    # optional
MIRAGE-INSTRUCTIONS:                  # human instructions, echoed in the report
<lines>
MIRAGE-ENDBLOCK
```

Semantics, as implemented by `scripts/apply_patches.py`:

1. **Signature resolution.** If `MIRAGE-FILE` is pinned and exists, the
   signature regex must match **exactly once** in that file. Otherwise (or if
   the pinned file is missing / has zero matches) the whole `src/` tree is
   grepped and the signature must match **exactly once overall** — exactly one
   file, one position. Zero or multiple matches → the block is **refused**
   with full diagnostics (which files, which counts, code context around the
   match). The applier never guesses.
2. **Replacement.** The `MIRAGE-REPLACE-FROM` text must occur **exactly once**
   in the resolved file (LF, no-EOL and CRLF line endings are tried in that
   order). It is replaced verbatim by `MIRAGE-REPLACE-TO`. If `FROM` is absent
   but `TO` is present → `already-applied` (idempotent re-runs are safe).
3. **Backups.** The first time a file is modified its pristine copy is saved as
   `<path>.mirage-orig`. `--revert` restores every `*.mirage-orig` under the
   Chromium root.
4. **Manual blocks** require a human (anchor too uncertain to template
   honestly). They appear in the report as `manual` with their instructions.
   Once the real anchor is known, convert the block to `MIRAGE-BLOCK` so
   future version bumps are automated.
5. Patch files are processed in lexicographic order; blocks inside a file in
   written order. A failing block does not stop other blocks; the final report
   lists applied / already-applied / manual / errors and the exit code is 1 if
   anything errored.

Inserted C++ code is wrapped in `// <<<MIRAGE(...)` … `// >>>MIRAGE(...)`
markers, so a patched tree greps cleanly (`grep -rn "<<<MIRAGE" src/`).

### Honesty policy

Templates carry anchors we could not verify against a real checkout only when
they are marked for verification: every uncertain anchor carries a
`MIRAGE-VERIFY` line naming what to check and how. A block that fails to
resolve is a **refusal**, not a best-effort guess — the same rule this README
applies to fingerprints applies to patches: an honest template beats a pretty
lie.

## Applying

From the Mirage repository root (Python 3.8+, stdlib only):

```
# always dry-run first
python scripts\apply_patches.py --chromium-dir C:\chromium\src --patches-dir patches --dry-run

# apply for real (writes .mirage-orig backups)
python scripts\apply_patches.py --chromium-dir C:\chromium\src --patches-dir patches

# restore every patched file from its backup
python scripts\apply_patches.py --chromium-dir C:\chromium\src --revert
```

`--chromium-dir` points at the `src/` checkout (the directory containing
`third_party/`, `content/`, …), not at the `out/` build directory.

### First application: verifying anchors

On the first run against `156.0.8078.80` expect some blocks to refuse — that
is by design. For each refused block:

1. Read the diagnostics (files/counts, context dump around the signature
   match).
2. Run the `MIRAGE-VERIFY` greps listed in the block.
3. Correct the `MIRAGE-REPLACE-FROM` text (and, if needed, the signature) in
   the `.patch` file to the real code of the tag.
4. Re-run. After the first successful application the templates are stable
   until the next version bump.

## Updating on a Chromium version bump

1. `cd src && git fetch --tags origin && git checkout <new-tag> && gclient sync -D`
   (details in [`scripts/build_chromium.md`](../scripts/build_chromium.md)).
2. `--dry-run` the applier. Blocks that resolve cleanly need nothing.
3. For refused blocks: grep the new tree for the signature, refresh the
   `MIRAGE-REPLACE-FROM` anchors to the new code, re-run.
4. `autoninja -C out\Release chrome` — incremental, typically well under a
   full build.
5. Update `MIRAGE-TARGET:` in each `.patch` header and this README once
   verified.

## Residuals and known limits

- **Per-character font fallback** (`FontCache::PlatformFallbackFontForCharacter`)
  is not gated in 0001: blocking it would break CJK/emoji rendering on
  realistic pages. Residual: a page can infer that a non-whitelisted font
  exists if a glyph missing from all whitelisted fonts renders instead of
  showing tofu. Mitigation: profile font lists are realistic full OS sets
  (`fp-db/fonts.json`), so the whitelist already contains the fallback fonts a
  real machine of the legend would use.
- **`local()` in `@font-face`** naming a non-whitelisted family fails through
  the same gate — consistent with "that font is not installed on this machine".
- **0002** covers 2D-canvas serialization paths (`ImageDataBuffer` /
  `CanvasAsyncBlobCreator`). WebGL canvas `toDataURL` goes through the same
  `ImageDataBuffer` snapshot path; `readPixels` noise stays in the JS layer.
- **0003** renames the content-level `--enable-automation` switch constant;
  tooling that *requires* that switch (classic Selenium/ChromeDriver combos)
  will not get automation mode inside a Mirage session — intended.
- The `.mirage-orig` backups are ignored by the build; do not ship them.

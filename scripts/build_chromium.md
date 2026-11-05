# Building the patched Chromium 156 (Windows)

Full build cycle for the Mirage native patch layer: from a clean machine to
`out\Release\chrome.exe` with `patches\` applied. The patches target tag
**`156.0.8078.80`** — see [`patches/README.md`](../patches/README.md) for what
they change and how they are applied.

## 1. Host requirements

| Resource | Minimum | Comfortable |
|---|---|---|
| OS | Windows 10/11 x64 | Windows 11 x64 |
| CPU | 8 cores | 12–16+ cores |
| RAM | 16 GB (expect 12–20 h builds) | 32 GB (**6–12 h** full build) |
| Disk | 150 GB free HDD | 200+ GB free NVMe SSD |
| Power | — | Plugged in; disable sleep during builds |

Keep the checkout on a short path (e.g. `C:\chromium`) — the tree contains
paths that exceed the default 260-character `MAX_PATH` unless long paths are
enabled (`git config --system core.longpaths true`, or enable Win32 long path
support in the registry/group policy).

## 2. Visual Studio 2022 and Windows SDK

1. Install **Visual Studio 2022** (Community is fine) with the workload
   **"Desktop development with C++"**.
2. Inside the VS Installer, make sure these components are present:
   - **MSVC v143 — VS 2022 C++ x64/x86 build tools** (latest),
   - **Windows 11 SDK** (10.0.22621.0 or newer),
   - **C++ ATL** for the same toolset (Chromium still pulls it in).
3. Chromium pins its exact toolchain versions via `DEPS` (clang, GN, ninja are
   fetched automatically by `gclient sync`) — but the local VS/SDK install
   must exist for non-Google builders.

## 3. depot_tools

```bat
git clone https://chromium.googlesource.com/chromium/tools/depot_tools.git C:\depot_tools
setx PATH "C:\depot_tools;%PATH%"
```

Open a **new** terminal (so PATH applies), then:

```bat
set DEPOT_TOOLS_WIN_TOOLCHAIN=0
gclient
```

- `DEPOT_TOOLS_WIN_TOOLCHAIN=0` tells depot_tools to use your locally
  installed VS 2022 instead of trying to download Google's internal
  toolchain (it is required for non-Google accounts; consider setting it
  permanently with `setx DEPOT_TOOLS_WIN_TOOLCHAIN 0`).
- The bare `gclient` call bootstraps the bundled python/git/ninja. Do not use
  a system git for the steps below — depot_tools' own tools must win in PATH.

## 4. Fetch the source

```bat
mkdir C:\chromium
cd C:\chromium
fetch --no-history chromium
```

- Creates `C:\chromium\src` (~35–45 GB, 1–2 h depending on disk/network).
- `--no-history` = shallow clone: smaller and faster; you can still fetch
  tags later (step 10). Full history adds ~25 GB and is not needed for
  building tags.

## 5. Check out the Mirage target tag

```bat
cd C:\chromium\src
git fetch --tags origin
git checkout 156.0.8078.80
gclient sync -D
```

- `gclient sync -D` aligns all dependencies (`DEPS`) to that tag and runs the
  hooks, which download clang, GN and the rest of the build toolchain via
  CIPD (1–3 h on a good connection).
- Verify: `git describe --tags` → `156.0.8078.80`.

## 6. Apply the Mirage patches

From the Mirage repository root:

```bat
python scripts\apply_patches.py --chromium-dir C:\chromium\src --patches-dir patches --dry-run
```

- **First application against a real tree: expect some blocks to refuse.**
  That is by design — every uncertain anchor carries a `MIRAGE-VERIFY` note;
  correct the `MIRAGE-REPLACE-FROM` text in the `.patch` file to the real
  code of the tag and re-run. Procedure and refusal diagnostics:
  [`patches/README.md`](../patches/README.md).
- Apply for real (creates `.mirage-orig` backups):

```bat
python scripts\apply_patches.py --chromium-dir C:\chromium\src --patches-dir patches
```

- Roll back everything: add `--revert`.

## 7. Configure the build (gn args)

```bat
cd C:\chromium\src
gn gen out\Release --args="is_debug=false is_component_build=false symbol_level=0 blink_symbol_level=0 proprietary_codecs=true ffmpeg_branding=\"Chrome\""
```

What each argument does:

| Argument | Why |
|---|---|
| `is_debug=false` | Release build: no DCHECKs (DCHECKs change timing and behavior), optimized. |
| `is_component_build=false` | Monolithic `chrome.exe`-style build like stock Chrome (component builds differ in DLL layout and crash signature). |
| `symbol_level=0` | No symbols in the output: ~10× smaller link time and disk use. |
| `blink_symbol_level=0` | Same for the (huge) Blink target. |
| `proprietary_codecs=true` + `ffmpeg_branding="Chrome"` | H.264/AAC support, matching stock Chrome. Without these, `MediaSource.isTypeSupported()` and playback behavior differ from every real Chrome — a codec-level fingerprint. |
| (not set) `is_official_build` | Official builds add PGO/LTO: +10–20 h build time for marginal gains; skip it. Note stock Chrome is an official build — if some edge check ever requires `chrome://version` to read "Official Build", revisit this. |

`target_cpu` defaults to `"x64"` — that is what you want.

## 8. Build

```bat
autoninja -C out\Release chrome
```

- **Full build: 6–12 hours** on a 32 GB / modern 12–16-core machine
  (longer on 16 GB RAM or fewer cores; link phase alone can take 30+ min).
- Incremental rebuilds after a patch refresh: typically 30–90 min.
- Output: `C:\chromium\src\out\Release\chrome.exe` plus the supporting files
  in the same directory — keep the whole directory together.

Performance tips:

- Add Defender exclusions for `C:\depot_tools` and `C:\chromium` (real-world
  difference: up to 2× on file-heavy steps).
- Build on NVMe; keep `out\` on the same physical drive as `src\`.
- `autoninja` scales automatically; leave the machine alone during the link.

## 9. Post-build sanity checks

1. Version: run `out\Release\chrome.exe --version` or open `chrome://version`
   → must read **156.0.8078.80**.
2. Patches active (from PowerShell):

```powershell
$fp = Get-Content <path-to-fingerprint.json> -Raw
Set-Content -Path $env:TEMP\mirage_fp.json -Value $fp -NoNewline
$env:MIRAGE_FP_CONFIG = "$env:TEMP\mirage_fp.json"
& C:\chromium\src\out\Release\chrome.exe --user-data-dir=$env:TEMP\mirage-udd --no-first-run
```

   Then verify per [`docs/DETECTION.md`](../docs/DETECTION.md):
   - browserleaks.com/fonts → the profile's font set (0001),
   - canvas signature via toDataURL → stable across reloads, different from
     the host machine and from other profiles (0002),
   - `navigator.webdriver` → `false` under any circumstances (0003),
   - **without** `MIRAGE_FP_CONFIG` the same binary behaves exactly like
     stock Chromium (patches are inert) — good A/B baseline.
3. Ship: distribute `out\Release` **without** `*.mirage-orig` files and point
   `mirage-launcher` at this binary (the launcher must also export
   `MIRAGE_FP_CONFIG` — see `patches/README.md`).

## 10. Updating to a new Chrome release

```bat
cd C:\chromium\src
git fetch --tags origin
git checkout <new-tag>          :: e.g. 132.0.6834.57
gclient sync -D
python <mirage-repo>\scripts\apply_patches.py --chromium-dir C:\chromium\src --patches-dir <mirage-repo>\patches --dry-run
:: fix refused anchors (grep recipes are in each .patch), then:
python <mirage-repo>\scripts\apply_patches.py --chromium-dir C:\chromium\src --patches-dir <mirage-repo>\patches
autoninja -C out\Release chrome
```

- `git checkout <new-tag>` on a `--no-history` checkout works — tags are
  fetched explicitly in step 5/10.
- Expect anchor churn in the Blink files (fonts/canvas) release over release;
  the `content/` anchors are usually stable. Refused blocks refuse loudly —
  nothing ambiguous is ever applied.
- Update `MIRAGE-TARGET:` in the `.patch` headers once verified.
- The fp-db and the JS inject layer must move to the new Chrome major at the
  same time (UA Reduction constants, brands) — see `docs/CONTRACT.md`.

## 11. Troubleshooting

| Symptom | Fix |
|---|---|
| `fetch`/`gclient` complains about toolchain | `set DEPOT_TOOLS_WIN_TOOLCHAIN=0`; make sure `C:\depot_tools` is first in PATH. |
| Long-path errors (`Filename too long`) | Short checkout path (`C:\chromium`), `git config --system core.longpaths true`. |
| gn cannot find VS 2022 | Run the build from a VS 2022 "Developer Command Prompt", or `set vs2022_install_path=C:\Program Files\Microsoft Visual Studio\2022\Community`. |
| Out of disk | `out\Release` is 60–100 GB with `symbol_level=0`; prune old output dirs (`gn clean out\Release`), never delete `src\.git` on a no-history checkout. |
| Linker OOM / thrashing | Close memory hogs; 16 GB machines should add `use_lld=true` (default on) and accept longer links; do not run builds in parallel. |
| Build fails after applying patches | Read the compile error — it names the file and the `<<<MIRAGE` marker; the `MIRAGE-VERIFY` notes in the corresponding `.patch` block describe the expected adjustment (e.g. the pixel-buffer accessor). Revert the file (`--revert`), fix, re-apply. |
| `chrome.exe` won't start after copy | Copy the whole `out\Release` directory, not just the exe; keep DLL/resources next to it. |

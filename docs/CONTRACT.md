# Mirage — контракт данных и модулей (v1, волна 1)

Антидетект-браузер: менеджер профилей, движок фингерпринтов, спуф-слой поверх Chromium через CDP.
Данные калиброваны под Chrome 156 (реальная версия установленного у пользователя Chrome; UA Reduction включена: минорные версии в UA заморожены в `.0.0`).
Каждый модуль волны 1 обязан соблюдать этот контракт буква-в-букву: имена полей, типы, пути, порядок.

## Карта модулей

| Модуль | Путь | Зона ответственности |
|---|---|---|
| mirage-core | crates/mirage-core | структуры Fingerprint/Profile, генератор, валидатор консистентности, загрузка fp-db |
| mirage-vault | crates/mirage-vault | шифрованное хранилище профилей (Argon2id + AES-256-GCM), файлы .mprof |
| mirage-proxy | crates/mirage-proxy | клиент SOCKS5 (RFC 1928/1929) и HTTP CONNECT, проверка exit-IP |
| mirage-launcher | crates/mirage-launcher | поиск Chrome, сборка аргументов, spawn, минимальный CDP-клиент, инъекция спуф-слоя |
| mirage-api | crates/mirage-api | локальный REST API (волна 2): профили, запуск/стоп, прокси, cookies, импорт, AdsPower-алиасы; HTTP-поверхность — docs/API.md |
| mirage-cli | crates/mirage-cli | CLI `mirage`: профили, запуск/статус/стоп, прокси, cookies, импорт, фингерпринты |
| mirage-cookies | crates/mirage-cookies | импорт/экспорт cookies из cookie-store Chromium (Netscape/JSON) |
| mirage-import | crates/mirage-import | импорт профилей из AdsPower, Dolphin Anty, GoLogin |
| mirage-sync | crates/mirage-sync | self-hosted sync-сервер: SQLite, API-ключи с ролями `admin`/`member`, ревизии профилей |
| mirage-e2e | crates/mirage-e2e | offline e2e-чекер спуф-слоя; живой прогон под `MIRAGE_E2E=1` |
| fp-db | fp-db/ | JSON-базы реальных значений (GPU, шрифты, экраны, локали) |
| inject | inject/ | JS-спуф слой (исполняется до скриптов страницы) |
| patches | patches/ | нативные патчи Chromium (второй, усиливающий слой) |
| sdk/python | sdk/python/ | Python SDK для локального API (только stdlib) |
| ui | ui/ | десктоп-панель на Tauri 2 (статический фронтенд + Rust-обёртка) |
| docs, scripts, .github | — | документация, smoke-скрипты, CI |

Общие правила кода: Rust edition 2021; полный production-код без TODO/заглушек; явная обработка
ошибок; без `unwrap`/`expect` в библиотечном коде (только в тестах); юнит-тесты для нетривиальной
логики; комментарии на английском, по делу.

## Схема Fingerprint (serde camelCase; это же — содержимое window.__MIRAGE_FP__ в браузере)

```json
{
  "os": "windows",
  "osVersion": "11",
  "platform": "Win32",
  "ua": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36",
  "chromeVersion": "156.0.8078.80",
  "languages": ["en-US", "en"],
  "timezone": "Europe/London",
  "geo": { "lat": 51.5074, "lon": -0.1278 },
  "screen": {
    "width": 1920, "height": 1080,
    "availWidth": 1920, "availHeight": 1040,
    "dpr": 1, "colorDepth": 24
  },
  "hardwareConcurrency": 8,
  "deviceMemory": 8,
  "webgl": {
    "vendor": "Google Inc. (NVIDIA)",
    "renderer": "ANGLE (NVIDIA, NVIDIA GeForce GTX 1660 SUPER Direct3D11 vs_5_0 ps_5_0, D3D11)",
    "maxTextureSize": 16384,
    "maxViewportDims": [32767, 32767],
    "maxRenderbufferSize": 16384
  },
  "canvasSeed": 1234567890,
  "audioSeed": 987654321,
  "fonts": ["Arial", "Calibri"],
  "webRTC": "real",
  "clientHints": {
    "brands": [
      { "brand": "Not:A-Brand", "version": "8" },
      { "brand": "Chromium", "version": "156" },
      { "brand": "Google Chrome", "version": "156" }
    ],
    "mobile": false,
    "platform": "Windows",
    "platformVersion": "15.0.0",
    "architecture": "x86",
    "bitness": "64",
    "uaFullVersion": "156.0.8078.80"
  }
}
```

Значения полей:

- `os`: `"windows" | "macos" | "linux"`
- `osVersion`: windows `"10" | "11"`; macos `"10.15" | "11" | "12" | "13" | "14"`; linux `"5.15" | "6.1" | "6.5"`
- `platform`: windows `"Win32"`, macos `"MacIntel"`, linux `"Linux x86_64"`
- `clientHints.platform`: `"Windows" | "macOS" | "Linux"`
- `clientHints.platformVersion`: windows10 → `"10.0.0"`, windows11 → `"13.0.0" | "15.0.0"`, macos → версия Darwin вида `"10.15.7"`, linux → `"5.15.0"` и т.п.
- `deviceMemory`: только `{2, 4, 8}` (значения, которые реально отдаёт Chrome)
- `hardwareConcurrency`: из `{2, 4, 6, 8, 12, 16, 20, 24}`
- `dpr`: из `{1, 1.25, 1.5, 2}`
- `webRTC`: `"real" | "relay" | "off"`
- `canvasSeed`, `audioSeed`: любые u32, детерминированно выводятся из seed профиля

## Схема Profile (шифруется целиком в vault)

```json
{
  "id": "6f9619ff-8b86-d011-b42d-00cf4fc964ff",
  "name": "Shop-01",
  "tags": ["work"],
  "seed": 20261103,
  "fingerprint": { },
  "proxy": { "scheme": "socks5", "host": "1.2.3.4", "port": 1080, "username": null, "password": null },
  "notes": "",
  "createdAt": "2026-11-03T12:00:00Z",
  "updatedAt": "2026-11-03T12:00:00Z"
}
```

- `id`: uuid v4 строкой (нижний регистр, дефисы)
- `seed`: `u64`, seed генерации фингерпринта; `0` = неизвестен (легаси-профили).
  Ненулевой seed позволяет пересобрать тот же фингерпринт на любой машине
  (`generate(seed, os)`); поле пишется в профиль при создании/импорте/пересборке.
- `proxy`: `null` = прямое соединение. `scheme`: `"http" | "socks5"`. `username`/`password`: `null` или строка
- даты: RFC 3339, UTC

## UA-шаблоны (UA Reduction, Chrome 156)

- windows: `Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36`
- macos: `Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36` (версия ОС в UA всегда заморожена `10_15_7`)
- linux: `Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36`

Константы: `CHROME_MAJOR = 156`, `CHROME_FULL = "156.0.8078.80"`.

GREASE-калибровка (снята с живого Chrome 156.0.8078.80, инструмент — `crates/mirage-launcher/examples/dump_native.rs`):
`GREASE_BRAND = "Not:A-Brand"`, `GREASE_VERSION = "8"`, `GREASE_FULL_VERSION = "8.0.0.0"`.
Порядок brands: `[GREASE, Chromium, Google Chrome]`. В `fullVersionList` GREASE-бренд получает
`GREASE_FULL_VERSION`, остальные — `uaFullVersion`. GREASE меняется с мажором Chrome:
при обновлении калибровки — снять дамп заново.

## fp-db — схемы файлов

- `fp-db/gpus.json`: `[{ "os": "windows", "vendor": "...", "renderer": "...", "maxTextureSize": 16384, "maxViewportDims": [32767, 32767], "maxRenderbufferSize": 16384 }]`
- `fp-db/fonts.json`: `{ "windows": ["Arial"], "macos": ["Helvetica"], "linux": ["DejaVu Sans"] }`
- `fp-db/screens.json`: `[{ "width": 1920, "height": 1080, "dpr": 1, "weight": 30 }]` — `availHeight` вычисляется, не хранится: windows `height - 40`, macos `height - 25`, linux `height - 24`; `availWidth = width`
- `fp-db/locales.json`: `[{ "languages": ["en-US", "en"], "timezone": "Europe/London", "lat": 51.5, "lon": -0.12, "weight": 10 }]` — timezone в формате IANA

## Правила консистентности (валидатор mirage-core)

1. os=windows → platform=`"Win32"`, clientHints.platform=`"Windows"`, UA по windows-шаблону, renderer содержит `Direct3D11` или `D3D11`, fonts из fonts.json:windows, availHeight = height − 40
2. os=macos → platform=`"MacIntel"`, clientHints.platform=`"macOS"`, renderer содержит `ANGLE Metal`, fonts из macos, availHeight = height − 25
3. os=linux → platform=`"Linux x86_64"`, renderer содержит `Mesa`, `llvmpipe` или `OpenGL ES 3.2 NVIDIA` (проприетарный драйвер NVIDIA на Linux легитимен и есть в fp-db), fonts из linux, availHeight = height − 24
4. deviceMemory ∈ {2,4,8}; hardwareConcurrency ∈ {2,4,6,8,12,16,20,24}
5. dpr ∈ {1, 1.25, 1.5, 2}; размеры экрана существуют в screens.json
6. languages непустой; timezone непустая (IANA-имя)
7. мажор chromeVersion = версия в каждом элементе brands; uaFullVersion = chromeVersion
8. canvasSeed и audioSeed детерминированы seed'ом генерации (не случайны при повторе)

## Спуф-слой inject/

Исполняется через `Page.addScriptToEvaluateOnNewDocument` ДО скриптов страницы в каждом документе.
Лаунчер склеивает source: `window.__MIRAGE_FP__ = <JSON Fingerprint>;\n` + файлы строго в порядке:

1. rng.js
2. navigator.js
3. screen.js
4. time.js
5. plugins.js
6. clienthints.js
7. canvas.js
8. webgl.js
9. audio.js
10. webrtc.js

Требования к каждому файлу: самодостаточный IIFE; `'use strict'`; ES2017; идемпотентность через
маркер `window.__MIRAGE_DONE__["<имя модуля>"] = true`; ноль вывода в console; ноль исключений
наружу (тело в try/catch); подмена свойств через `Object.defineProperty` на прототипах, не на
инстансах (исключение — webrtc.js в режиме relay: перехваты onicecandidate/getStats обязаны жить
на инстансе, нативный RTCStatsReport иммутабелен и через прототип не пересобирается).

Особые правила:

- time.js: сначала сравнить фактическую зону `new Date()` с конфигом (CDP `Emulation.setTimezoneOverride`
  уже применён лаунчером нативно). Если совпадает — не патчить ничего: нативная эмуляция лучше
  JS-подмены, JS-подмену Date детектят.
- fonts: JS-слой НЕ подменяет перечисление шрифтов (детектится measurement-проверками). Шрифты
  живут в нативном слое patches. Поле fonts в конфиге — для нативного слоя.
- plugins.js: только правдоподобный набор современного Chrome (5 записей PDF-семейства).
- webrtc.js: режим из fp.webRTC: `"real"` — ничего не делать; `"relay"` — оставить только
  relay-кандидаты (JS-фильтр onicecandidate и getStats + флаг лаунчера); `"off"` —
  `delete window.RTCPeerConnection` и webkit-алиаса: свойство отсутствует целиком, как при
  реально выключенном WebRTC.
- navigator.js: `navigator.webdriver` → false; перехват на `Navigator.prototype`.

## Аргументы Chromium (mirage-launcher)

Обязательно: `--user-data-dir=<dir>`, `--remote-debugging-port=<port>`, `--no-first-run`,
`--no-default-browser-check`, `--disable-blink-features=AutomationControlled`,
`--password-store=basic`, `--hide-crash-restore-bubble`, `--lang=<languages[0]>`,
`--start-maximized` (геометрия окна обязана соответствовать легенде экрана: у максимизированного
окна `outerWidth == availWidth`, `outerHeight == availHeight`).

Порт отладки: значение `0` в конфиге — лаунчер сам выбирает свободный порт.

При наличии proxy: `--proxy-server=<scheme>://<host>:<port>`.
При webRTC=`"relay"`: `--force-webrtc-ip-handling-policy=disable_non_proxied_udp`
(значение `relay_only` в Chrome 156 не применяется — проверено живым прогоном).

Запрещено добавлять: `--headless`, `--enable-automation`, `--disable-gpu` (ломает WebGL-рендерер),
`--no-sandbox`.

## CDP-команды (mirage-launcher, сразу после подключения)

1. `Page.enable`, затем `Network.enable`
2. `Network.setUserAgentOverride`: `{ userAgent, acceptLanguage: languages.join(","), platform, userAgentMetadata: { brands, fullVersionList (те же brands с полными версиями: GREASE-бренд → GREASE_FULL_VERSION, остальные → uaFullVersion), fullVersion: uaFullVersion, platform: clientHints.platform, platformVersion, architecture, model: "", mobile, bitness, wow64: false } }`
3. `Emulation.setTimezoneOverride`: `{ timezoneId: timezone }`
4. `Emulation.setLocaleOverride`: `{ locale: languages[0] }`
5. `Emulation.setDeviceMetricsOverride`: `{ width: screen.width, height: screen.height, screenWidth: screen.width, screenHeight: screen.height, deviceScaleFactor: screen.dpr, mobile: false }` — нативная подмена геометрии: width/height (вьюпорт) и screenWidth/screenHeight (эмулируемый экран) берутся ИЗ ЛЕГЕНДЫ (не 0), иначе CSS media queries (`min-width`, `device-width`, `resolution`, DPR) отвечали реальным монитором, а JS `screen.width/height` — легендой (mismatch main-vs-CSS); `deviceScaleFactor` согласует media queries с `window.devicePixelRatio` (media queries из JS не патчатся)
6. `Page.addScriptToEvaluateOnNewDocument`: `{ source: <склейка из раздела inject/> }`

## Пути (Windows) и формат vault

```
%LOCALAPPDATA%\Mirage\
  vault\<id>.mprof        # MAGIC "MGPF" || u32 LE version=1 || nonce[12] || AES-256-GCM(json Profile)
  vault\.vault.salt       # 16 случайных байт, создаётся при первом открытии
  profiles\<id>\          # user-data-dir Chromium
```

Vault принимает каталог явно (путь `%LOCALAPPDATA%` подставляет вызывающий, не vault).

## Версии зависимостей (Rust)

serde 1, serde_json 1, rand 0.8, thiserror 1, anyhow 1, aes-gcm 0.10, argon2 0.5,
tokio 1 (features: full), tokio-tungstenite 0.21, tempfile 3 (dev-зависимость).

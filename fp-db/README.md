# fp-db — калибровочная база реальных значений фингерпринта

Это **данные, а не код**: пул правдоподобных реальных значений, из которых
`mirage-core` собирает консистентный `Fingerprint` профиля. Схемы файлов описаны
в `docs/CONTRACT.md` (раздел «fp-db — схемы файлов») и должны соблюдаться
буква-в-букву: имена полей, типы, порядок. Загрузчик `mirage-core` валидирует
каждый файл при старте; невалидный JSON = отказ запуска.

База откалибрована под **Chrome 156** (`CHROME_MAJOR = 156`) с включённой
UA Reduction. Строки GPU — реальные дампы WebGL/ANGLE соответствующей эры.

## Файлы

### `gpus.json`

Массив записей GPU per-OS. Поля: `os` (`windows | macos | linux`), `vendor`,
`renderer`, `maxTextureSize`, `maxViewportDims: [w, h]`, `maxRenderbufferSize`.

Форматы renderer-строк (реальная анатомия ANGLE, проверять каждое новое значение
по живому дампу — устаревший или искажённый формат сам по себе детектится):

- **windows** (D3D11-бэкенд):
  `ANGLE (<vendor>, <DXGI adapter name> (0x<PCI device id>) Direct3D11 vs_5_0 ps_5_0, D3D11)`,
  vendor `Google Inc. (<NVIDIA|AMD|Intel>)`.
  PCI device id в скобках — обязателен начиная с Chrome 90; ANGLE берёт его из
  `DXGI_ADAPTER_DESC.DeviceId` и печатает как 8 hex-цифр (`(0x00002504)`).
  Лимиты на Windows одинаковы для всех вендоров на FL11+: 16384 / [32767, 32767] / 16384
  (это константы D3D11 feature level, а не свойства карты — «уникальные» лимиты
  на Windows = артефакт подделки).
- **macos** (Metal-бэкенд):
  `ANGLE (<Apple|AMD|Intel>, ANGLE Metal Renderer: <Metal device name>, Unspecified Version)`,
  vendor `Google Inc. (<Apple|AMD|Intel>)`. Apple Silicon: `Apple M1`, `Apple M1 Pro`,
  `Apple M2`, `Apple M2 Max`; Intel-маки: `AMD Radeon Pro 5300M/5500M`,
  `Intel(R) Iris(TM) Plus Graphics 645`. Лимиты: 16384 / [16384, 16384] / 16384.
- **linux** (ANGLE GL/GLES-бэкенд):
  `ANGLE (<vendor>, <GL_RENDERER> (0x<PCI device id>) OpenGL ES 3.2 <driver>)`,
  vendor `Google Inc. (<NVIDIA|Intel|AMD|Mesa>)`.
  NVIDIA: `NVIDIA GeForce RTX 3060 (0x00002503) OpenGL ES 3.2 NVIDIA 550.107.02`.
  Mesa: `Mesa Intel(R) UHD Graphics 620 (Kaby Lake GT2) (0x00005917) OpenGL ES 3.2 Mesa 23.2.1`,
  radeonsi: `AMD Radeon RX 6600 (radeonsi navi23 LLVM 15.0.7) (0x000073FF) ...`.
  Софтверный рендер: `ANGLE (Mesa, llvmpipe (LLVM 15.0.7, 256 bits))` — лимиты
  8192 / [8192, 8192] / 8192, vendor `Google Inc. (Mesa)`.

### `fonts.json`

Объект `{ "windows": [...], "macos": [...], "linux": [...] }` — имена шрифтовых
семейств, видимых перечислению шрифтов (DirectWrite / CoreText / fontconfig).
Поле `fonts` из Fingerprint потребляет нативный слой `patches/`; JS-слой
перечисление шрифтов не подменяет (см. CONTRACT). Списки — общесистемные наборы
по умолчанию (Windows 10/11, macOS, mainstream-дистрибутивы Linux), без
Office/Adobe/экзотики, которой на целевой машине быть не обязано.

### `screens.json`

Массив `{ "width", "height", "dpr", "weight", "os"? }`. `dpr` строго из `{1, 1.25, 1.5, 2}`.

Необязательное поле `os` привязывает запись к одной платформе и закрывает класс
багов «невозможная комбинация ОС+экран» (M13):

- `"os": "macos"` — Retina-логические размеры, которые в дикой природе встречаются
  только на маках: `1440x900@2` (MBP 13" Retina), `1512x982@2` (MBP 14"),
  `1728x1117@2` (MBP 16"), `2560x1600@2`, `1280x800@2` (MB 13" физические
  2560x1600). Панели с такими физическими разрешениями вне Apple не выпускаются.
- `"os": "windows"` — дробный скейл как специфика Windows: `1536x864 dpr 1.25`
  (физические 1920x1080 @ 125%) и `2560x1440 dpr 1.5` (4K-панель @ 150%).
  macOS такие dpr не отдаёт (валидатор требует для macos dpr ∈ {1, 2}).
- Отсутствие поля = кросс-платформенная запись (например, `1920x1080@1`,
  `1366x768@1`) — доступна любой легенде.

Генератор предлагает легенде только записи без тега или с тегом её ОС;
валидатор консистентности бракует легенду os=X с экраном, тегированным os=Y,
и macos-легенду с dpr ∉ {1, 2}. `availHeight` по-прежнему не хранится, а
вычисляется по ОС профиля (windows −40, macos −25, linux −24), `availWidth = width`.
`weight` — относительная частота (ориентир — статистика десктопов).

### `locales.json`

Массив `{ "languages", "timezone", "lat", "lon", "weight" }`. `languages` —
список с резервными («главный» + базовый + `en`), `timezone` — IANA-имя,
`lat`/`lon` — центры крупных городов (4 знака), привязанные к зоне. Вес —
ориентировочная частота связки «язык + зона» в веб-аудиториях.

## Как расширять

1. Новая запись GPU: возьми **реальный дамп** с целевой ОС и Chrome (не выдумывай):
   скопируй `UNMASKED_VENDOR_WEBGL` / `UNMASKED_RENDERER_WEBGL` и лимиты
   (`MAX_TEXTURE_SIZE`, `MAX_VIEWPORT_DIMS`, `MAX_RENDERBUFFER_SIZE`) буква-в-букву.
   Сверь PCI device id с базой PCI ID. Убедись, что запись проходит валидатор
   консистентности CONTRACT (D3D11 / ANGLE Metal / Mesa-llvmpipe в renderer).
2. Новые экраны/локали: только реально встречающиеся комбинации; dpr из
   `{1, 1.25, 1.5, 2}`; timezone — валидное IANA-имя; координаты — в той же зоне.
   Экран, встречающийся только на одной платформе (Retina-логический размер,
   дробный скейл Windows), обязан нести `"os"`-тег — иначе генератор выдаст
   невозможную комбинацию, а валидатор её забракует.
3. Все JSON — валидный JSON (UTF-8, без комментариев, без trailing commas),
   поля строго по CONTRACT. После правок прогони загрузку `mirage-core`.

## Откуда брать свежие значения

- **Реальные дампы WebGL**: webglreport.com, gpu-check сканеры (например,
  checkproxy.org GPU library — живая база renderer-строк с лимитами),
  собственный Chrome на целевых машинах (`chrome://gpu`, DevTools:
  `gl.getParameter(gl.RENDERER)` + `WEBGL_debug_renderer_info`).
- **Исходники ANGLE/Chromium**: формат renderer-строк задаётся бэкендом
  (`Renderer11::getRendererDescription`, `DisplayGL`, Metal-бэкенд) — при смене
  мажорной версии Chrome формат надо перепроверять.
- **Веса/популярность железа**: Steam Hardware Survey (доли GPU и разрешений),
  Amazon bestsellers (что реально у людей), StatCounter (доли OS/разрешений).
- **PCI device id**: The PCI ID Repository (pci-ids.sourceforge.net), TechPowerUp
  GPU Database.
- **Шрифты**: чистая установка целевой ОС + перечисление через DevTools;
  списки стандартных шрифтов Microsoft/Apple документированы вендорами.

## ⚠ Предупреждение

Это **калибровочные данные**. Они не «истина навсегда»: со временем меняются
драйверы, строки ANGLE, доли разрешений и версия целевого Chrome. Неверно
актуализированная база (например, Windows-строки без PCI id — формат до Chrome
90, или устаревшие версии драйверов NVIDIA) сама по себе является детект-сигналом
для антифрод-систем. Пересматривать базу при каждом изменении `CHROME_MAJOR` и
не реже раза в квартал по весам.

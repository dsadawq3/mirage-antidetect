# inject/ — JS-спуф-слой Mirage

Файлы этого каталога склеиваются лаунчером и исполняются через
`Page.addScriptToEvaluateOnNewDocument` **до скриптов страницы** в каждом документе.
Порядок склейки и общие требования — `docs/CONTRACT.md`, раздел «Спуф-слой inject/».
Каждый файл: самодостаточный IIFE, `'use strict'`, ES2017, без зависимостей и сборки;
идемпотентность через `window.__MIRAGE_DONE__["<модуль>"]`; ноль вывода в console;
все ошибки глушатся внутри модуля — наружу не летит ни одного исключения.

`worker.js` в этот бандл **не входит** (в `INJECT_MODULES` его нет): это отдельный
воркер-сниппет, который лаунчер внедряет прямо в воркер-таргеты через CDP до старта
воркера — см. раздел «Воркер-скоуп: два механизма».

Конфигурация приходит в `window.__MIRAGE_FP__` (схема Fingerprint из CONTRACT.md).
Если конфига нет или не хватает нужных полей — модуль молча не устанавливается.

## Модули

Порядок = порядок склейки (важен: `rng.js` первым, `webrtc.js` последним). `worker.js`
стоит последней строкой только как справка — в бандл страницы он не входит.

| Файл | Маркер | Назначение |
|---|---|---|
| `rng.js` | `rng` | Детерминированный PRNG `window.__MIRAGE_RNG__`: `hash32(a,b,c)`, `noise(seed,x,y)` → `[0,1)`, `int(seed,x,y,max)`; `noise`/`int` считаются от разных солей хеша; `Math.random` не используется нигде. Плюс **фабрика нативных обёрток** `makeNative(name, length, impl)` (см. ниже) — ею построены все методы и геттеры слоя. Плюс реестр `track(fn, имя)` / `nativeName(fn)` (Map функция→имя), сохранённый для воркер-сниппета и диагностики; центральной toString-маски в бандле больше нет. |
| `navigator.js` | `navigator` | Геттеры идентичности на `Navigator.prototype`: platform, hardwareConcurrency, deviceMemory, languages/language (один замороженный массив, `[SameObject]`), userAgent, webdriver→false, maxTouchPoints, vendor. **oscpu не определяется вообще** (см. ниже). Плюс спуф списка голосов `SpeechSynthesis` (см. ниже) и переписывание page-created воркеров (см. ниже). |
| `screen.js` | `screen` | Геометрия `Screen.prototype` (width/height/avail*/colorDepth/pixelDepth) и window-метрики: `devicePixelRatio`, `outerWidth`/`outerHeight` = **avail** (геометрия максимизированного окна), innerWidth/innerHeight не трогаем (реальный вьюпорт). |
| `time.js` | `time` | Fallback таймзоны за CDP-оверрайдом: если фактическая зона != fp.timezone — патчит `getTimezoneOffset`, `Date#toString/toTimeString` (переписывается только GMT±HHMM-метка), обёртка `Intl.DateTimeFormat` + `resolvedOptions`. Если зона совпала — модуль не патчит ничего (нативная эмуляция CDP лучше). |
| `plugins.js` | `plugins` | Модель плагинов Chrome 156: 5 PDF-плагинов, у **каждого** `length === 2` (application/pdf + text/pdf); `navigator.mimeTypes` — ровно **2** записи (дедуп по типу), инстансы `MimeType` общие для одинаковых типов (`plugins[i]['application/pdf'] === mimeTypes['application/pdf']`, `enabledPlugin` → первый плагин). Own-гигиена: поля и методы — на фейковых прототипах (`Object.create(<реальный прототип>)`, instanceof сохраняется), у инстансов own только индексы и имена, у `MimeType` — ноль own-свойств. |
| `clienthints.js` | `clienthints` | `navigator.userAgentData`. `[SecureContext]`: на не-secure странице **не определяется вовсе** (натив не имеет). `[SameObject]`: один кэшированный инстанс; поля (brands/mobile/platform — геттеры), `getHighEntropyValues` и `toJSON` — на прототип-объекте `Object.create(NavigatorUAData.prototype)`; own у инстанса — ноль. `toJSON` возвращает `{brands, mobile, platform}` — `JSON.stringify(userAgentData)` работает как натив. `getHighEntropyValues` **всегда** содержит `brands`+`mobile`+`platform` плюс запрошенное; промис никогда не реджектится. |
| `canvas.js` | `canvas` | Шум в `getImageData` (2D + OffscreenCanvas): **~1.5%** пикселей, LSB одного канала, ключ `(canvasSeed, абсолютные X, Y)`. **Виргинное правило**: пиксели RGBA all-0 / all-255 не шумятся — пустой canvas читается нулями, как натив. **Тихий угол**: блок 8×8 в начале координат (`x < 8 && y < 8`) не шумится никогда (обход live-детектора creepjs, см. ниже). |
| `webgl.js` | `webgl` | `getParameter` (UNMASKED_*, маскированные строки Chrome 156, MAX_*), pass-through `getExtension`, шум `readPixels` (~2%, то же виргинное правило). |
| `audio.js` | `audio` | Шум анализатора (`getFloatFrequencyData`/`getByteFrequencyData`, ключ `(audioSeed, bin)`). Плюс **стандартный оффлайн-фингерпринт**: `OfflineAudioContext.prototype.startRendering` — на результат (AudioBuffer) накладывается детерминированный Float32-шум ±1e-7 от `(audioSeed, channelIndex, sampleIndex)`; NaN/±Infinity не трогаются; обе формы — promise и legacy-колбэк (`oncomplete`). DynamicsCompressor не трогаем. |
| `webrtc.js` | `webrtc` | Режимы `fp.webRTC`: `real` — ничего; **`off` — `delete window.RTCPeerConnection` и webkit-алиаса** (`typeof RTCPeerConnection === 'undefined'`, как у реального Chrome без WebRTC); `relay` — обёртка конструктора + инстанс-фильтры: onicecandidate, add/removeEventListener('icecandidate') и **getStats** (см. ниже). `Function.prototype.toString` модуль **не** патчит (см. ниже). |
| `worker.js` | — | **Вне бандла.** Воркер-сниппет для воркер-скоупа: `WorkerNavigator`, `userAgentData`, WebGL `getParameter`/`readPixels`, `OffscreenCanvas` и своя мини-маска toString. Внедряется лаунчером в воркер-таргеты через CDP до старта воркера (см. ниже). |

## Нативные обёртки (`makeNative`) и почему центральной toString-маски больше нет

Каждый метод и геттер слоя собирается фабрикой `window.__MIRAGE_RNG__.makeNative(name, length, impl)`.
Фабрика возвращает `Proxy` над **стрелочной** функцией (target); `impl(thisArg, args)` — тело
обёртки, получающее настоящий получатель и аргументы. Такой объект неотличим от нативного
WebIDL-метода/геттера по всему набору, которым его щупают prototype-lie детекторы (creepjs и
родственные):

- **не конструируется**: `new fn()` и `class X extends fn {}` бросают нативный `TypeError`
  (стрелка не конструктор, own `prototype` отсутствует);
- own-ключи ровно `['length', 'name']`, нет own `prototype` / `arguments` / `caller` / `toString`;
  `'prototype' in fn === false`;
- `Function.prototype.toString` — **нативный, не перехваченный** — для `Proxy` отвечает
  `"function () { [native code] }"` в любом realm, в т.ч. через чистый кросс-реалмный toString;
- ловушки воспроизводят нативные формы ошибок: `setPrototypeOf` проигрывает
  `TypeError("Cyclic __proto__ value")` для цикла, а `get` воспроизводит точную ошибку
  `Function.prototype.toString` для не-функции-получателя (её стек читается как
  `at Function.toString`, а не `at Object.toString`).

`length` обёртки берётся из исходного нативного дескриптора, поэтому сигнатура остаётся честной.
Если в рантайме нет `Proxy` (на Blink не бывает), фабрика деградирует к простой функции-обёртке.

**Центральная маска `Function.prototype.toString` удалена.** Раньше `webrtc.js` (последний в
бандле) подменял `Function.prototype.toString` общим whitelist'ом по реестру `track`. Живые
прогоны creepjs показали, что это чистый проигрыш: детектор тестирует сам
`Function.prototype.toString`, видит, что замена — не натив, и записывает это как ложь. Одна
ложь переключает его гейт `detectProxies` и запускает расширенную battery против **каждой**
функции (`Object.setPrototypeOf(fn, Object.create(fn))`, `fn.__proto__ = proxy; fn++`,
`Reflect.setPrototypeOf` и т.п.), на которой Proxy-обёртки дают ещё пачку ложей. Никакая
JS-маска не проходит обе battery, а Proxy-обёртки делают маску ненужной — поэтому
`Function.prototype.toString` теперь честно нативный. Реестр `track(fn, имя)` / `nativeName(fn)`
в `rng.js` сохранён: им пользуется воркер-сниппет (в воркер-скоупе нет `window.__MIRAGE_RNG__`)
и диагностика. Каждый модуль по-прежнему отмечает свои функции через `track`, но на страницу
это уже не влияет.

Геттеры идентичности (`navigator`/`screen`/`userAgentData`) дополнительно несут нативный
brand-check: чтение геттера с самого прототипа бросает `TypeError('Illegal invocation')`, как у
реального WebIDL-аксессора.

Следствие (известный gap): оставшиеся **простые функции, которые обязаны быть конструируемыми**
(`Blob` / `Worker` / `SharedWorker` / `RTCPeerConnection`), в `makeNative` не заворачиваются и не
маскируются — детектор, который их `toString()`-ит, увидит исходник; калибровочные цели этого
не делают.

## Воркер-скоуп: два механизма

Воркеры патчатся с двух сторон, потому что ни одна не покрывает всё.

### 1. Переписывание page-created воркеров (navigator.js)

Геттеры на `Navigator.prototype` не работают в `new Worker(...)` — воркеры читали реальные
hardwareConcurrency/deviceMemory/languages/platform/userAgent. Механизм:

- обёртка `Blob`-конструктора: `WeakMap<Blob, {parts, type}>` — части
  снапшотятся синхронно в момент конструирования;
- обёртка `URL.createObjectURL`/`revokeObjectURL`: маппинг blob-URL→Blob
  (revoke чистит маппинг — отозванный URL ведёт себя нативно);
- `Worker(url)` / `SharedWorker(url)`:
  - blob-URL нашего Blob → реальный воркер от нового Blob
    `[сниппет, ...исходные parts]` тем же type — работает и для classic,
    и для **module**-воркеров (сниппет — обычные statement до импортов);
  - data: URL → полезная нагрузка декодируется (percent/base64) и
    prepend'ится; для classic при неудаче декодирования — fallback на
    importScripts-бутстрап;
  - http(s)/относительный URL + classic-воркер → blob-URL бутстрапа
    «сниппет + importScripts(<абсолютизированный url>)»;
  - опции (name/type/credentials/...) прокидываются как есть;
  - bootstrap-URL кэшируется по исходному URL → два
    `SharedWorker(sameUrl)` разделяют ОДИН инстанс, как натив;
- сниппет: самодостаточная строка с инлайн-конфигом
  (hardwareConcurrency, deviceMemory, languages, language, platform, ua),
  внутри воркера патчит `WorkerNavigator.prototype` (только `self`, без
  window) и ставит свою мини-маску toString (в воркер-реалме нет
  `__MIRAGE_RNG__`, поэтому сниппет несёт собственный PRNG-минимум и
  локальный реестр).

**Ограничения этой ветки (by design, любой сбой = тихий honest-воркер):**
- module-воркер с внешним http(s) URL — **не патчится** (в module-скоупе
  нет importScripts): легенда внутри такого воркера не действует;
- blob-URL'ы Blob'ов, созданных не через наш конструктор
  (например `response.blob()`) — источник недоступен синхронно;
- ServiceWorker через эту ветку не регистрируется (JS страницы до его
  global scope не достаёт) — его закрывает ветка 2;
- внутри бутстрапнутого http(s)-воркера `self.location` — blob-URL
  бутстрапа, а не оригинального скрипта (importScripts грузит
  оригинальный абсолютный URL);
- источник blob-воркера, начинающийся с `#!` (shebang), не патчится —
  prepend сломал бы синтаксис.

### 2. Инъекция в воркер-таргеты через CDP (worker.js)

`worker.js` — **НЕ в бандле страницы**. Лаунчер держит
`Target.setAutoAttach { autoAttach, waitForDebuggerOnStart, flatten }` (фильтр по типам
`worker` / `service_worker` / `shared_worker`) и на каждый attach к такому таргету шлёт
`Runtime.evaluate` с исходником `self.__MIRAGE_FP__ = <JSON Fingerprint>;\n` + `worker.js`,
**пока воркер держится на паузе** (`waitForDebuggerOnStart`), т.е. до старта его собственного
скрипта. Это единственный путь дотянуться до global scope **service-worker'а** и до воркеров,
которых страница не создавала через обёрнутые конструкторы. `allowUnsafeEvalBlockedByCSP`
пропускает одноразовый сетап даже на воркере со строгой CSP. После инъекции на сессии воркера
ставят вложенный `setAutoAttach` — покрытие **вложенных** воркеров (best effort).

Что спуфит (в синхроне с page-модулями):
- `WorkerNavigator.prototype`: hardwareConcurrency, deviceMemory, platform, userAgent,
  languages/language (один замороженный массив, `[SameObject]`). `webdriver` / `vendor` /
  `maxTouchPoints` **не добавляются** — в воркер-скоупе их нет, инвентарь был бы палевом;
- `userAgentData` (brands/mobile/platform-геттеры, `getHighEntropyValues`, `toJSON`),
  `[SecureContext]` гейтится по **нативному дескриптору**, а не по `isSecureContext`, и
  `[SameObject]`;
- WebGL `getParameter` (UNMASKED vendor/renderer, маскированные строки, MAX_*) и `readPixels`
  (LSB-шум) на `WebGLRenderingContext` / `WebGL2RenderingContext.prototype` — иначе
  `OffscreenCanvas.getContext('webgl')` отдаёт реальный GPU, отличный от легенды;
- OffscreenCanvas 2D `getImageData` (LSB-шум) и `convertToBlob`;
- свою **локальную** маску `Function.prototype.toString`.

`convertToBlob` идёт через тот же зашумлённый пиксельный путь: JS не может править уже
закодированный PNG, поэтому враппер декодирует blob (`createImageBitmap`), рисует на
scratch-`OffscreenCanvas`, читает обратно зашумлённым `getImageData` и перекодирует **исходным**
`convertToBlob`. При любой ошибке (декод, размер, неподдерживаемый тип) возвращается
неизменённый исходный blob — воркер этим патчем не ломается.

**Ограничения:**
- `worker.js` идёт отдельным от бандла файлом: страница его не содержит, он живёт только в
  воркер-реалме; PRNG продублирован внутри (полная копия `rng.js`);
- покрыты только таргеты `worker` / `service_worker` / `shared_worker` с web-content origin
  (http/https/blob/data); воркеры бандлов и внутренностей браузера
  (`chrome-extension://`, `chrome://`, `devtools://`) освобождаются, но **никогда** не патчатся;
- воркер, стартовавший раньше, чем лаунчер приаттачился и внедрил сниппет, остаётся честным.

## Спуф синтеза речи (navigator.js)

Список голосов ОС — жёсткий device-tell: легенда fr-FR / Windows, отдающая
`Microsoft Irina - Russian` (или любой голос хоста), палит реальную машину, а единственный
не-легендный голос по умолчанию заставляет детекторы ругаться на связку timezone/locale.
`SpeechSynthesis.prototype.getVoices` заменяется детерминированным каталожным набором,
согласованным с языками легенды:

- каталог строится по первичному языку `fp.languages[0]`; есть таблицы под en/fr/de/es/it/nl/pl/
  sv/ru/tr/pt/ja/ko/zh и региональные варианты (fr-CA, en-GB, es-MX) — набор берётся из них,
  ничего не выдумывается вне легенды;
- **ровно один** голос помечен `default` — локальный (SAPI), как в реальном Chrome/Windows;
- остальные — оставшиеся локальные из детерминированного подмножества (pick от `canvasSeed`/
  `audioSeed`) плюс built-in remote-голос Chrome для языка легенды; для не-английской легенды
  добавляется английский «хвост»;
- результат стабилен между вызовами и **никогда** не откатывается к нативному списку, даже пока
  нативный ещё грузится (пуст);
- `onvoiceschanged` перехвачен (геттер/сеттер на прототипе) так, что обработчик страницы остаётся
  достижим через нативную событийную машину — поздний нативный `voiceschanged` не отдаёт странице
  полузагруженный список.

## WebRTC

- `real` — натив, ничего не трогаем.
- `off` — `delete window.RTCPeerConnection` / `window.webkitRTCPeerConnection`.
  Реально выключенный WebRTC = свойства нет; конструктор с TypeError —
  изобретение, которого нет ни в одном нативном билде.
- `relay` — на инстансе (как onicecandidate):
  - `onicecandidate` (get/set) и `addEventListener`/`removeEventListener`
    для `icecandidate`: до страницы доходят только `typ relay` и
    end-of-candidates (null/пустой candidate); всё непроверяемое
    гасится (fail closed);
  - `getStats()`: нативный промис цепляется через фильтр — из отчёта
    удаляются записи-кандидаты (`local-candidate`/`remote-candidate`/
    legacy `candidate`) с `candidateType !== 'relay'` и связанные с ними
    `candidate-pair`. Остальные записи не трогаются. Отчёт
    пересобирается: цепочка прототипов через `RTCStatsReport.prototype`
    (instanceof сохраняется), интерфейс maplike — `get`/`has`/`size`/
    `forEach`/`entries`/`keys`/`values`/итерация. Promise-семантика
    сохранена (реджекты проходят насквозь).

## Правило виргинных пикселей и тихий угол (canvas + webgl)

Пиксель, у которого все четыре канала RGBA = 0x00 или все четыре = 0xFF,
**никогда не шумится**. Нативный Chrome читает пустой canvas 64×64
нулями и пустой WebGL-readPixels нулями — шум «на пустоте» мгновенный
детект. Для остальных пикселей шум детерминирован как раньше.

**Тихий угол (canvas.js).** «Drawn vs read» детектор creepjs (`getPixelMods`) рисует матрицу
сплошных заливок 8×8 и читает каждый пиксель обратно как `getImageData(x, y, 1, 1)`, помечая
canvas, если прочитанный байт отличается от нарисованного. Любое по-пиксельное возмущение только
что нарисованной плоской заливки ловится этим сравнением (это сравнение «нарисовано vs прочитано»,
а не повторное чтение). Единственный JS-способ остаться чистым — не трогать ровно те пиксели,
которые щупает детектор, поэтому блок 8×8 в начале координат (`x < 8 && y < 8`) не шумится
никогда. Правило — чистая функция абсолютных координат, поэтому контракт стабильности (полное
чтение / чтение под-области) не ломается: угол просто фиксированная нешумная область. Обход
тактический; окончательное решение — нативный слой (`patches/`), где шум живёт в рендерере и
нарисованный байт совпадает с прочитанным.

## Правило стабильности

Весь шум детерминирован от seed профиля и координат: одинаковый вход →
одинаковый вывод, всегда. Обязательные следствия (проверяются харнессом
`%TEMP%\rhamz\mirage_inject_fix_test.js`):

1. два подряд чтения одного холста/буфера/рендера → побитово одинаковый результат;
2. `getImageData` полного холста и под-области → одинаковые решения для общих пикселей;
3. перезагрузка страницы, новый iframe, перезапуск браузера → тот же хеш;
4. другой профиль (другой `canvasSeed`/`audioSeed`) → другой хеш;
5. виргинный холст → нули при любом seed.

## Что сознательно НЕ перехватывается и почему

| API | Причина |
|---|---|
| `HTMLCanvasElement.prototype.toDataURL`, `toBlob` | JS-обёртка не может «шумить» закодированный PNG/JPEG: нельзя изменить строку data-URL без нарушения её валидности, а декод + перекодирование из JS — асинхронно, медленно и палится таймингами. Кодировщик обязан пройти через тот же зашумлённый пиксельный путь — это делает **нативный слой `patches/`**. Пока его нет, canvas-фингерпринты через toDataURL видят незашумленный результат — известный gap (контракт проекта). |
| `HTMLCanvasElement.prototype.getContext` | Возврат прокси-контекста ломает `instanceof` и нативные вызовы. Шум на уровне `getImageData`/`readPixels` покрывает те же точки чтения. |
| `Function.prototype.toString` | Центральная маска убрана (см. выше): она сама детектится и запускает каскад ложных срабатываний. Нативные обёртки `makeNative` дают нужную строку без патча. |
| Перечисление шрифтов | JS-подмена детектится measurement-проверками. Шрифты спуфит нативный слой; `fp.fonts` в конфиге — для него. |
| `AudioBuffer.prototype.getChannelData` (прямые вызовы) | Данные реального воспроизведения; возмущение искажает звук. Оффлайн-рендер покрыт через `startRendering`. |
| `DynamicsCompressorNode` | Компрессор обязан вести себя как реальное устройство — подменённая передаточная функция измерима рендер-сравнением. Уникальность даёт шум в точках считывания. |
| `AudioContext.prototype.sampleRate` | Частота дискретизации обязана совпадать с аудиостеком ОС; расхождение тривиально перекрёстно проверяется. |
| `getSupportedExtensions`, `getError` | Спуф не даёт ценности; вызов `getError` из обёртки **съедал бы** GL-ошибку страницы. |
| `innerWidth`/`innerHeight` | Должны совпадать с реальным вьюпортом, иначе layout-детект мгновенный. |

## Как проверять вручную

- <https://browserleaks.com/canvas> —
  Signature-хеш должен быть **стабилен между перезагрузками** и **отличен
  от реального Chrome** на той же машине;
- <https://browserleaks.com/webgl> — Vendor/Renderer из профиля. Строки в `fp-db/gpus.json`
  держатся в живом формате ANGLE: `ANGLE (NVIDIA, NVIDIA GeForce … Direct3D11 vs_5_0 ps_5_0,
  D3D11)`, `ANGLE (Intel, Intel(R) … )`, Apple `ANGLE (Apple, ANGLE Metal Renderer: …, Unspecified
  Version)`, AMD `ANGLE (AMD, Radeon … / AMD Radeon …, Direct3D11 …)`; vendor — `Google Inc.
  (NVIDIA/AMD/Intel/Apple)`;
- <https://browserleaks.com/audio> — хеш стабилен между перезагрузками и
  отличен от хост-машины (теперь покрывает и OfflineAudioContext-рендер);
- воркеры: в консоли страницы —

```js
var b = new Blob(['postMessage([navigator.hardwareConcurrency, navigator.deviceMemory, navigator.platform])']);
var w = new Worker(URL.createObjectURL(b));
w.onmessage = function (e) { console.log(e.data); }; // легенда, не хост-значения
```

  Тот же результат должен приходить и из воркера, которого страница не создавала через обёрнутый
  конструктор (например из service-worker'а): их закрывает `worker.js` через CDP.

- нативные обёртки:

```js
Object.getOwnPropertyDescriptor(Navigator.prototype, 'platform').get.toString()
// "function platform() { [native code] }" — исходника и __MIRAGE_* нет
Object.getOwnPropertyNames(Object.getOwnPropertyDescriptor(Navigator.prototype, 'platform').get)
// ["length","name"] — own-ключи как у натива
Object.getOwnPropertyDescriptor(Navigator.prototype, 'platform').get.call(Navigator.prototype)
// TypeError: Illegal invocation — нативный brand-check геттера
(function honest() { return 1; }).toString() // честные функции вообще не тронуты
```

- WebRTC: режим `off` → `typeof RTCPeerConnection` === `'undefined'`;
  режим `relay` → в `getStats()` нет кандидатов с `candidateType !== 'relay'`.

## Известные ограничения

- toDataURL/toBlob — незашумленный результат, закрывается нативным слоем
  `patches/` (см. таблицу выше);
- module-воркеры с внешним URL не спуфятся через page-ветку (легенда внутри не действует);
  воркеры и service-worker'ы, до которых дотягивается лаунчер по CDP, закрывает `worker.js`
  (см. раздел про воркеры);
- «тихий угол» canvas 8×8 — тактический обход одного детектора; окончательное решение —
  нативный слой `patches/`;
- пересобранный getStats-отчёт — не родной `RTCStatsReport`-инстанс
  (нативный отчёт иммутабелен, удалить записи из него нельзя): instanceof
  и maplike-интерфейс сохранены, но тайпинг через `Object.prototype.toString`
  покажет `Object`. Риск принят: альтернатива — утечка реальных адресов;
- глобалы `__MIRAGE_FP__` / `__MIRAGE_RNG__` / `__MIRAGE_DONE__` видны
  странице — контракт волны 1. Скрытие/очистка — задача лаунчера или
  нативного слоя после исполнения всей цепочки модулей;
- геометрия outer* описывает **максимизированное** окно: не-максимизированное
  окно течёт реальными внешними границами (принято волной 1,
  `--start-maximized` ставит лаунчер).

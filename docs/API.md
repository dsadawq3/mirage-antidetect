# Mirage Local API (v1) — замороженный контракт

Единый локальный HTTP-интерфейс ко всему ядру Mirage. Потребители: UI (Tauri),
CLI, Python SDK, внешние скрипты. Отдельный слой совместимости с AdsPower —
чтобы их скрипты переезжали без переписывания.

- Адрес: `127.0.0.1`, порт по умолчанию **35500** (`MIRAGE_API_PORT`).
- Авторизация: если задан `MIRAGE_API_TOKEN` — все `/api/v1/*` требуют
  `Authorization: Bearer <token>`, **кроме `GET /api/v1/health`** (он всегда
  без авторизации); не задан — без авторизации (только localhost).
- Хранилище: `mirage-vault`. Пароль через `MIRAGE_VAULT_PASSWORD` (при старте)
  либо `POST /api/v1/vault/unlock` (сессионно, хранится в памяти).
- CORS: `Access-Control-Allow-Origin` зеркалит origin только из аллоулиста
  (`MIRAGE_ALLOWED_ORIGINS`, дефолт — origin'ы Tauri: `http://tauri.localhost`,
  `https://tauri.localhost`, `tauri://localhost`); чужой origin ACAO не получает
  (preflight такого origin → 403). Запросы без `Origin` (curl, SDK) работают без
  CORS-заголовков. `Access-Control-Allow-Headers: authorization, content-type`.
- Тела профилей — схема `Profile` из `docs/CONTRACT.md`, без изменений.

## Ошибки

Нативные эндпоинты: HTTP-статус + тело
`{ "error": { "code": "<code>", "message": "<текст>" } }`.

Коды: `invalid_request`, `not_found`, `already_running`, `not_running`,
`vault_locked`, `io`, `internal`.

AdsPower-алиасы: всегда HTTP 200 + `{ "code": <0|не 0>, "msg": "<...>", "data": ... }`
(их API так устроено; исключение — неверный токен → 401).

## Эндпоинты (нативные)

| Метод | Путь | Тело | Ответ |
|---|---|---|---|
| GET | `/api/v1/health` | — | `{ "ok": true, "version": "0.1.0", "vault": "locked"\|"unlocked", "running": 0 }` |
| POST | `/api/v1/vault/unlock` | `{ "password": "..." }` | `{}` (401 при неверном пароле) |
| GET | `/api/v1/profiles` | — | `{ "profiles": [ProfileSummary] }` |
| POST | `/api/v1/profiles` | `CreateProfileRequest` | 201 `Profile` |
| GET | `/api/v1/profiles/:id` | — | `Profile` |
| PUT | `/api/v1/profiles/:id` | `UpdateProfileRequest` | `Profile` |
| DELETE | `/api/v1/profiles/:id` | — | 204 |
| POST | `/api/v1/profiles/:id/launch` | — | `{ "target_id", "ws_url", "debug_port", "profile_dir" }` (409 если уже запущен) |
| POST | `/api/v1/profiles/:id/stop` | — | `{}` |
| GET | `/api/v1/status` | — | `{ "running": [{ "profile_id", "target_id", "debug_port", "started_at" }] }` |
| POST | `/api/v1/proxy/check` | `{ "scheme", "host", "port", "username"?, "password"? }` | `{ "ok": bool, "ip"?, "latency_ms"?, "error"? }` |
| POST | `/api/v1/profiles/:id/cookies/import` | `{ "format": "netscape"\|"json", "data": "<строка>" }` | `{ "imported": N, "skipped": N }` |
| GET | `/api/v1/profiles/:id/cookies/export?format=netscape\|json` | — | `{ "format", "data" }` |
| POST | `/api/v1/import` | `{ "source": "adspower"\|"dolphin"\|"gologin", "data": "<строка>" }` | `{ "imported": [Profile], "warnings": [ { "field": "<строка>", "kind": "mapped"\|"defaulted"\|"ignored" } ] }` |

Элемент `warnings[]` у `/api/v1/import` — объект `{ "field": "<логическое поле>", "kind": "mapped"|"defaulted"|"ignored" }` (не строка), по одному на каждое решение маппера по полю.

### Модели запросов

`ProfileSummary`: `{ "id", "name", "tags": [], "running": bool, "proxy": { "scheme", "host", "port" } \| null, "createdAt", "updatedAt" }`

`CreateProfileRequest`:
```json
{
  "name": "Shop-01",
  "tags": ["work"],
  "seed": 20261103,
  "os": "windows",
  "proxy": { "scheme": "socks5", "host": "1.2.3.4", "port": 1080, "username": null, "password": null },
  "overrides": { "timezone": "Europe/Paris", "languages": ["fr-FR", "fr"] }
}
```
- `seed` нет → генерируется случайный; фингерпринт = `generate(seed, os)`.
- `overrides` — частичный `Fingerprint`; мержится поверх сгенерированного и
  перепроверяется валидатором консистентности (несовместимое → 400).

`UpdateProfileRequest`: `{ "name"?, "tags"?, "proxy"?, "seed"?, "overrides"? }`. Если
`seed` присутствует, фингерпринт пересобирается с нуля: `generate(seed, os профиля)`,
поверх накладывается `overrides`, результат проверяется валидатором (несовместимое → 400),
и `seed` профиля обновляется. Без `seed` фингерпринт пересчитывается только от `overrides`.

## AdsPower-совместимые алиасы

`{ "code": 0, "msg": "success", "data": ... }`, code 0 = успех.

| Метод | Путь | Параметры | data |
|---|---|---|---|
| GET | `/api/v1/browser/start` | `user_id` | `{ "ws": { "puppeteer": "<ws_url>" }, "debug_port", "target_id" }` |
| GET | `/api/v1/browser/stop` | `user_id` | `{}` |
| GET | `/api/v1/browser/list` | `page`, `page_size` | `{ "list": [{ "user_id", "name", "group_id": "0", "status": "Active"\|"" }], "page", "page_size", "total" }` |
| POST | `/api/v1/user/create` | AdsPower-тело профиля | `{ "id": "<profile_id>", "warnings": [] }` |
| POST | `/api/v1/user/update` | AdsPower-тело + `user_id` | `{}` |
| POST | `/api/v1/user/delete` | `user_id` или `{ "user_ids": [] }` | `{}` |

Маппинг AdsPower → Mirage делает `mirage-import`; не смапленное попадает в
`data.warnings`, не отбрасывается молча.

## Примеры

```bash
curl -s http://127.0.0.1:35500/api/v1/health
curl -s -X POST http://127.0.0.1:35500/api/v1/profiles \
  -H 'content-type: application/json' \
  -d '{"name":"Shop-01","os":"windows","seed":20261103}'
curl -s http://127.0.0.1:35500/api/v1/browser/start?user_id=<id>
```

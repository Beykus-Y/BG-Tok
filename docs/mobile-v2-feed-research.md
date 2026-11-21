# Deep Research: Мобильный API TikTok `/aweme/v2/feed/`

Дата: 2026-11-21  
Анализируемый артефакт: `TikTok_MC_1.0.4-Beta(47.5.3)_clone_Fix.apk` (`com.ss.android.ugc.trill`, `versionCode=470503`, `versionName=47.5.3`, 52 DEX, переподписан RSA-2048 с пустым DN). Подлинная немодифицированная сборка Play Store в локальной среде отсутствовала, поэтому все выводы опираются на декомпилят данной сборки и прямые HTTP-проверки.

---

## 1. Архитектурная карта протокола `/aweme/v2/feed/`

| Этап | Класс / Метод (реальное имя) | DEX / Путь | Назначение | Уверенность |
| :--- | :--- | :--- | :--- | :--- |
| 1. Точка входа API | `com.ss.android.ugc.aweme.feed.api.FeedApi.LIZ(06Ak)` / `LIZIZ(06Ak)` | `classes28.dex` (`FeedApi.java`) | Инициирует загрузку ленты, инициализирует `RetrofitApi` и вызывает `C091006Bp.LIZJ(06Ak)` | Подтверждено кодом |
| 2. Сборка запроса | `X.C091006Bp.LIZJ(06Ak)` → `LIZLLL(C091706Bw, 06Ak)` → `LJI(...)` | `classes42.dex` (`C091006Bp.java`) | Формирует Protobuf-запрос `C090906Bo` через пайплайн процессоров `InterfaceC091806Bx`, ставит `Content-Type: application/x-protobuf` | Подтверждено кодом |
| 3. Транспортный интерфейс | `FeedApi.RetrofitApi`: `fetchInitialFeedStream`, `postFetchRecommendFeedV2Call`, `postFetchRecommendFeedV2CallNew` | `classes28.dex` (`FeedApi.java:147-162`) | Вызовы `/aweme/v2/feed/`: потоковый старт (`TypedInput`) и POST-запросы (`ComposePbAndJson<C117806gs, FeedItemList>` / `FeedItemList`) | Подтверждено кодом |
| 4. Модель запроса | `X.C090906Bo` (Wire `Message`) | `classes42.dex` (`C090906Bo.java`) | Protobuf-тело запроса с тегами пагинации, типа ленты, кэша и персонализации | Подтверждено кодом |
| 5. Модель ответа (wire) | `X.C117806gs` (Wire `Message`, `aweme_v2_feed_response`) | `classes14.dex` (`C117806gs.java`) | Protobuf-ответ сервера: `status_code`, курсоры, `has_more`, `aweme_list`, `rid`, реклама и карточки | Подтверждено кодом |
| 6. Конвертация в домен | `ConvertHelper.com$ss$ugc$tiktok$proto$aweme_v2_feed_response$$com$ss$android$ugc$aweme$feed$model$FeedItemList` и `X.1V8S` | `classes28.dex` (`FeedApi.java:86, 209`) | Преобразует `C117806gs` (или JSON fallback) в `FeedItemList` | Подтверждено кодом |
| 7. Мониторинг ошибок | `FeedApi.LJ` (`C06BO`) + `07Pb.LIZ` | `classes28.dex` (`FeedApi.java:42-145`), `classes42.dex` (`C091006Bp.java:1051-1073`) | Логирует `foru_data_parse_monitor` (`normal`, `error`, `server_empty`) и ошибки `postFetchRecommendFeedV2 failed` | Подтверждено кодом |

### Различие методов `RetrofitApi` (`FeedApi.java:148-161`)
- `fetchInitialFeedStream`: аннотирован `@05zt` (streaming), возвращает `04Na<TypedInput>` для первичной потоковой загрузки.
- `postFetchRecommendFeedV2Call`: возвращает `04Na<ComposePbAndJson<C117806gs, FeedItemList>>` (Protobuf-ответ `C117806gs`, конвертируемый в `FeedItemList`).
- `postFetchRecommendFeedV2CallNew`: возвращает `04Na<FeedItemList>` напрямую через адаптер `1V8S`.

---

## 2. Подтверждённая схема запроса (`X.C090906Bo`, `classes42.dex`)

| Tag | Поле в `C090906Bo` | Тип | Подтверждённое назначение |
| :--- | :--- | :--- | :--- |
| `1` | `type` | `INT32` | Тип ленты (`0` = For You, `1` = Following) |
| `2` | `max_cursor` | `INT64` | Верхний курсор пагинации |
| `3` | `min_cursor` | `INT64` | Нижний курсор пагинации |
| `4` | `count` | `INT32` | Размер запрашиваемой порции |
| `5` | `feed_style` | `INT32` | Режим отображения ленты |
| `6` | `aweme_id` | `INT64` | Целевой ID ролика (при переходе к конкретному видео) |
| `7` | `pull_type` | `INT32` | Тип запроса (`0` — старт, `1` — refresh, `2` — load more; см. `C091006Bp.java:1113`) |
| `13` | `content_language` | `STRING` | Языковые предпочтения контента |
| `16` | `preload_aweme_ids` | `STRING` | Предзагруженные ID |
| `18` | `cached_item_num` | `INT32` | Количество элементов в локальном кэше |
| `25` | `show_ids` | `STRING` | Уже показанные ID |
| `26` | `cache_ids` | `STRING` | ID в кэше |
| `28` | `disable_personalization` | `BOOL` | Флаг отключения персонализации |
| `40` | `batch_number` | `INT32` / `INT64` | Номер батча сессии |
| `56` | `is_cold_start` | `BOOL` | Холодный старт приложения |
| `66` | `feed_source` | `INT32` | Источник ленты |
| `68` | `is_non_personalized` | `BOOL` | Неперсонализированный режим |
| `71` | `external_feed_type` | `STRING` | Внешний тип ленты |
| `72` | `cmpl_enc` | `STRING` | Зашифрованный маркер соответствия/согласий |

---

## 3. Подтверждённая схема ответа (`X.C117806gs` и `X.C05LG`, `classes14.dex`)

### Ответ `C117806gs` (`aweme_v2_feed_response`)
- `1`: `status_code` (`INT32`) — код статуса (`0` = успех).
- `2`: `min_cursor` (`INT64`) и `3`: `max_cursor` (`INT64`) — курсоры пагинации.
- `4`: `has_more` (`INT32`) — признак наличия следующих страниц.
- `5`: `aweme_list` (`repeated X.C05LG`) — упорядоченный список рекомендаций; **порядок элементов в массиве задаёт порядок показа**.
- `6`: `rid` (`STRING`) — `requestId` порции.
- `8`: `refresh_clear` (`INT32`) — очистка локального списка при обновлении.
- `13`: `preload_ads` (`repeated X.C05LG`) и `16`: `preload_awemes` (`repeated X.C05LG`) — рекламные и предзагруженные элементы.
- `14`: `status_msg` (`STRING`), `15`: `block_code` (`INT32`).
- `24`: `card_insert_results` (`repeated 0Ccv`) — специальные карточки (Lynx/опросы/Live-карты).
- `25`: `is_non_personalized` (`BOOL`) — признак неперсонализированного ответа.

### Элемент `C05LG` (`aweme_struct`, `C05LG.java:1203-1234`)
- `1`: `aweme_id` (`STRING`), `2`: `desc` (`STRING`), `3`: `create_time` (`INT64`), `4`: `author`, `5`: `music`, `7`: `video` (`05PE`), `9`: `user_digged` (`INT32`), `10`: `statistics` (`05Q4`), `11`: `status` (`05Qs`), `25`: `is_vr` (`BOOL`).

---

## 4. Уровни защиты, транспорта и авторизации

1. **Транспорт:** `libsscronet.so`, `libttboringssl.so`, `libttquic.so` (кастомный Cronet/TTNet стек с собственным BoringSSL/QUIC).
2. **Подпись и целостность запросов (`LoginAssuranceObject.java:9` + `libmetasec_ov.so`):**
   - Обязательные заголовки подписи/анти-тампера: `X-Gorgon`, `X-Khronos`, `X-Argus`, `X-Ladon`, `X-Ss-Stub`, `X-Ss-Req-Ticket`, `X-Vc-Bdturing-Sdk-Version`.
   - Аппаратная и сессионная защита: `tt-ticket-guard-iteration-version`, `tt-ticket-guard-public-key`, `tt-ticket-guard-version`, `tt-ticket-guard-server-data`, `tt-ticket-guard-client-data`, `tt-device-guard-client-data`, `tt-device-guard-iteration-version`.
   - Идентификаторы клиента: `device_id`, `aid`, `iid`, `app_id`, `version_code`, `ttnet_version`.
3. **Пользовательская сессия:** `passport_csrf_token`, `X-Tt-Passport-Csrf-Token`, `cmpl_token`, `tt-target-idc`, `tt-target-idc-sign`.
4. **Официальная сторонняя авторизация:** Login Kit v2 и Display API v2 работают через `open.tiktokapis.com` и **не** дают доступа к `/aweme/v2/feed/` или `/aweme/v1/aweme/stats/`.

---

## 5. Практический эксперимент (2026-11-21)

- **Запрос:** `GET https://api16-normal-c-useast1a.tiktokv.com/aweme/v2/feed/?feed_type=0&count=6` (без подделки `X-Gorgon`/`X-Argus`/`X-Ladon` и без приватных ключей).
- **Наблюдаемый ответ:** `HTTP/1.1 200 OK`, `Content-Type: application/json`, **`Content-Length: 0`**, `tt_orcas_res: 1`, `x-tt-logid: 2026112117352636067CBD861E30FC962D`.
- **Вывод:** Шлюз TikTok принимает TCP/TLS-соединение, но молча отбрасывает тело ответа (`Content-Length: 0`), если отсутствуют валидные клиентские подписи `libmetasec_ov.so` и зарегистрированные параметры устройства/сессии. Эксперимент остановлен на этом шаге, так как воспроизведение `X-Gorgon`/`X-Argus`/`X-Ladon` требует извлечения проприетарных алгоритмов и ключей из `libmetasec_ov.so`.

---

## 6. Обратная связь для рекомендаций (`/aweme/v1/aweme/stats/`, `/aweme/v1/commit/item/digg/`)

- `C90400z4p.java:216-229` (`classes46.dex`) подтверждает отправку статистики просмотра на `/aweme/v1/aweme/stats/` с полями `action_time`, `aweme_type`, `enter_from`, `item_id`, `play_delta`.
- `VideoDiggApi.java:11` (`classes46.dex`) подтверждает отправку лайков на `/aweme/v1/commit/item/digg/`.
- Оба интерфейса являются приватными клиентскими RPC, защищёнными тем же стеком `libmetasec_ov.so`. Разрешённого публичного API для отправки событий просмотра из стороннего плеера (`BG Media`) не существует, поэтому обучение ленты TikTok на просмотрах внутри BeyGram невозможно.

---

## 7. Итоговый ответ на главный инженерный вопрос

**Можем ли мы создать самостоятельный TikTok For You Provider на Rust (без Chromium, WebView, LSPosed и официального TikTok)?**

**Нет — на текущий момент это невозможно законным и технически поддерживаемым способом.**

**Что именно препятствует:**
1. Отсутствие официального API TikTok для персональной ленты `For You` (Login Kit v2 / Display API v2 / oEmbed не предоставляют её).
2. Мобильный `/aweme/v2/feed/` и веб-эндпоинт `/api/recommend/item_list/` защищены нативными и JS-подписями целостности (`libmetasec_ov.so`: `X-Gorgon`, `X-Khronos`, `X-Argus`, `X-Ladon`, `tt-ticket-guard-*`, `tt-device-guard-*`; в вебе — `X-Bogus`/`X-Gnarly`), без которых сервер возвращает пустой ответ (`Content-Length: 0` / `{"status_code": 0}`).
3. Отсутствие разрешённого API для отправки реальных событий просмотра (`/aweme/v1/aweme/stats/`) и лайков (`/aweme/v1/commit/item/digg/`).

**Что остаётся рабочим и подтверждённым в BG Tok:**
- Публичное разрешение видео по ID/ссылке, потоковая передача MP4 через `MediaDelivery` (с `ByteRange`), публичный Explore, комментарии, oEmbed, Login Kit v2 и Display API v2 (PR #5).
- Экспериментальный `CompanionFeedAdapter` за feature-флагом `experimental-companion` (PR #7) для приёма упорядоченных ID от внешнего наблюдателя на тестовом Android-устройстве.
- Встроенный WebView (`https://www.tiktok.com/foryou`) как отдельная Web-интеграция вне `BG Media`.

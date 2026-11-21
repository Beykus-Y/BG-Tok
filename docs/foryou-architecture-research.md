# Исследование персональной ленты TikTok For You (APK 47.5.3, Toki, HushFeed, Metra)

Дата: 2026-11-21

## 1. Объект анализа (APK)

- **Файл:** `TikTok_MC_1.0.4-Beta(47.5.3)_clone_Fix.apk` (537,468,067 байт)
- **Пакет:** `com.ss.android.ugc.trill`
- **Версия:** `versionName='47.5.3'`, `versionCode='470503'`, `minSdkVersion='24'`, `targetSdkVersion='36'`
- **Подпись (`apksigner`):** v2=true, v3=true, v1=false; 1 подпись RSA-2048 с пустым DN (`CN=, OU=, O=, L=, ST=, C=`), SHA-256 сертификата `2509dde1335913f2b1461311277b6303b33dac390ef984f30892cfb332cbbe75` (переподписанная/модифицированная сборка на базе официального `com.ss.android.ugc.trill` 47.5.3).
- **Комплектность:** 26,693 записей ZIP, 52 DEX-файла (`classes.dex`..`classes52.dex`), 595 нативных библиотек (`arm64-v8a`, `armeabi-v7a`, по 300 на ABI).

---

## 2. Архитектурная схема получения For You в официальном Android-клиенте

```text
[1] Запрос ленты (Retrofit / TTNet / Cronet)
    com.ss.android.ugc.aweme.feed.api.FeedApi.RetrofitApi
      ├── @POST/@GET "/aweme/v2/feed/" -> postFetchRecommendFeedV2Call(...)
      ├── @... "/aweme/v2/feed/" -> postFetchRecommendFeedV2CallNew(...)
      └── @... "/aweme/v2/feed/" -> fetchInitialFeedStream(...)
    Слой подписи и транспорта (нативные .so):
      ├── libsscronet.so + libttboringssl.so + libttquic.so (TLS/QUIC)
      └── libmetasec_ov.so (X-Gorgon, X-Khronos, X-Argus, X-Ladon, device/ticket guard)
          │
          ▼
[2] Десериализация ответа (Protobuf + JSON)
    ComposePbAndJson<C117806gs, FeedItemList>
    ConvertHelper.com$ss$ugc$tiktok$proto$aweme_v2_feed_response$$com$ss$android$ugc$aweme$feed$model$FeedItemList
          │
          ▼
[3] Модель порции рекомендаций: com.ss.android.ugc.aweme.feed.model.FeedItemList
    ├── items / getItems() : List<Aweme>  (порядок элементов от сервера)
    ├── preloadAds         : List<Aweme>  (TopView / рекламные вставки)
    ├── cursor, hasMore, fetchType, requestId
    └── dataUserId (null/пусто для For You; заполнен для списков профиля)
          │
          ├──► Кэш / восстановление (Toki & HushFeed):
          │      COLD_FEED (0), FEED_UNCONSUMED (1), GOLDEN_HOUSE (2), OFFLINE_MODE (3), MERGE_CACHE (4)
          │
          ▼
[4] Модель элемента: com.ss.android.ugc.aweme.feed.model.Aweme
    ├── getAid()                    -> TikTok Video ID
    ├── getItemDistributeSource()   -> пул рекомендаций (напр. for_you_page_999)
    ├── getAwemeType(), isAd(), isSoftAd()
    ├── getVideo() -> getDuration(), getPilotLength(), getPlayAddr()
    ├── getStatistics() -> getPlayCount(), getDiggCount()
    └── getAuthor() -> getUniqueId(), getNickname()
          │
          ▼
[5] Репозиторий и Adapter UI
    RECOMMEND_MODEL (setItems, append, update, insertItemList, batch, deleteItems)
    FEED_ADAPTER / RECOMMEND_ADAPTER (BaseFeedPageParams.getEventType() == "homepage_hot")
          │
          ▼
[6] Обратная связь и события просмотра (подписанные клиентские RPC)
    ├── Просмотр/досмотр: /aweme/v1/aweme/stats/ (item_id, play_delta, action_time, aweme_type, enter_from)
    └── Лайк: VideoDiggApi -> /aweme/v1/commit/item/digg/
```

---

## 3. Результаты изучения HushFeed, Toki и Metra

| Проект | Лицензия | Точки перехвата | Ключевые наблюдения |
| :--- | :--- | :--- | :--- |
| **Toki** (`MeiYongAI/Toki`) | **MIT** | `FeedFilterHook.kt`, `FeedModelReader.kt`, `FeedFilterPolicy.kt`, `HostSymbols.kt` | Перехват через LSPosed/libxposed: `FeedApi.LIZ` (`feedType == 0` → `FeedScope.FOR_YOU`, `1` → Following), `COLD_FEED`, `PRELOADED_FEED`, `RECOMMEND_MODEL`, `FEED_ADAPTER` (`BaseFeedPageParams.getEventType() == "homepage_hot"`). Читает `FeedItemList.getItems()`, `Aweme`. Символы резолвятся по отпечаткам DEX (`HostSymbols`). |
| **HushFeed** (`SysAdminDoc/hushfeed`) | **GPLv3** *(код не переносился)* | `FeedItemsFilter.java`, `FeedCapture.java` | Bytecode-патчи: `FeedItemsFilter.filter(FeedItemList)` после `fetchFeedList` и `FeedItemsFilter.filterOnRead(FeedItemList)` внутри `FeedItemList.getItems()` (ловит cold cache, golden house, merge cache, offline). Отличает For You от профилей через `WeakHashMap` + `FeedItemList.dataUserId`. Читает `Aweme.getAid()`, `Aweme.getItemDistributeSource()`, `AwemeBizExtKt.getCacheSourceType()`. |
| **Metra** (`icysymmetra/tiktok-patches-for-morphe`) | **GPLv3** *(код не переносился)* | `docs/feed-controls-and-debugging.md`, feed-патчи | Аналогичная Morphe-архитектура перехвата `FeedItemList` и отладочного дампа; фиксирует источник `for_you_page_999` и ограничения на непомеченные маршруты (mid-roll/profile ads). |

---

## 4. Подтверждённые факты vs Гипотезы и препятствия

### Подтверждённые факты (из декомпиляции APK 47.5.3 и живых HTTP-проверок)
1. **Независимый серверный протокол (Категория A — невозможен без проприетарной подписи):**
   - `LoginAssuranceObject` (в декомпиляте APK) и `FeedApi` показывают, что запросы мобильного клиента защищены заголовками `X-Gorgon`, `X-Khronos`, `X-Argus`, `X-Ladon`, `X-Ss-Stub`, `X-Ss-Req-Ticket`, `tt-ticket-guard-*`, `tt-device-guard-*`, `X-Tt-Passport-Csrf-Token` и нативной библиотекой `libmetasec_ov.so` поверх `libsscronet.so` (`libttboringssl.so`).
   - Веб-эндпоинт `/api/recommend/item_list/` без клиентских JS-подписей (`X-Bogus`/`X-Gnarly`) и пользовательской сессии возвращает `{"status_code": 0}` без видео.
   - Официальный Login Kit v2 / Display API v2 **не** предоставляют персональную ленту For You и **не** предоставляют API отправки событий просмотра (`/aweme/v1/aweme/stats/`) или лайков (`/aweme/v1/commit/item/digg/`).
2. **Android Companion (Категория B — технически осуществим на тестовом Android с ограничениями):**
   - На устройстве с установленным официальным TikTok и модулем наблюдения (уровень LSPosed/Toki или Morphe-патч) можно считывать `FeedItemList.getItems()` и `Aweme.getAid()` с сохранением серверного порядка, `hasMore`, `cursor` и привязкой к `FeedScope.FOR_YOU` (`feedType == 0` / `homepage_hot`), не экспортируя cookies/токены/device_id.
   - В текущем окружении физическое Android-устройство/эмулятор не подключено (`f:\platform-tools\adb.exe devices` вернул пустой список), поэтому этап живой проверки на Android-устройстве (Этап 3–4) не выполнялся и остаётся **непроверенным вживую**.
3. **События просмотра (Этап 6):**
   - В официальном APK отправка просмотров идёт через `/aweme/v1/aweme/stats/` (с параметрами `item_id`, `play_delta`, `action_time`, `aweme_type`, `enter_from`), а лайки — через `/aweme/v1/commit/item/digg/`. Оба запроса подписываются клиентским стеком (`libmetasec_ov.so`).
   - **Следствие:** при воспроизведении видео в `BG Media` внутри BeyGram без подписанного клиента TikTok алгоритм рекомендаций TikTok **не обучается** на этих просмотрах. Фиктивные просмотры генерировать запрещено.

### Неподтверждённые гипотезы
- Гипотеза о том, что длительный экспорт ID через Android Companion не вызовет расхождений с отображаемой лентой при агрессивной предзагрузке (`GOLDEN_HOUSE` / `MERGE_CACHE`), требует живой проверки на тестовом Android-устройстве.

---

## 5. Альтернатива: WebView / Web-интеграция (Этап 5)

Если нативный `BG Media`-провайдер не может получать персональную `For You` напрямую (Категория A невозможна):
- **Механизм:** открытие `https://www.tiktok.com/foryou` внутри встроенного WebView (`WebView` на Android, `WKWebView` на iOS, `WebView2` на Windows) с явной авторизацией пользователя в веб-сессии TikTok.
- **Классификация:** это **WebView/Web-интеграция**, а **не** нативный `BG Media`-провайдер (видео декодирует и рендерит браузерный движок, события просмотра отправляет JS-рантайм самого TikTok).
- **Ограничения:**
  - Не используется единый нативный плеер/кэш `BG Media` и жестовая навигация BeyGram.
  - На части мобильных/десктопных User-Agent веб-версия TikTok ограничивает фоновое воспроизведение, навязывает баннеры «Открыть в приложении» или требует прохождения веб-капчи при входе.
  - Условия использования TikTok запрещают несанкционированное извлечение потоков/cookies из WebView.

---

## 6. Ограничения для Android, Windows и iOS (Этап 7)

| Платформа | Категория | Статус и ограничения |
| :--- | :--- | :--- |
| **Android (тестовое устройство с LSPosed / патчем)** | **B (Android Companion)** | Технически возможно наблюдение `FeedItemList`/`Aweme.getAid()` в экспериментальном адаптере `CompanionFeedAdapter` (`feature = "experimental-companion"`). Требует установленный TikTok и модуль наблюдения; не подходит для обычной дистрибуции в Google Play / stock-устройств. |
| **Android (обычное устройство без root/патча)** | **C (Невозможно подтвердить)** | Без модификации APK/LSPosed межпроцессное чтение моделей TikTok заблокировано песочницей Android. Доступна только WebView-интеграция или публичные/OAuth-методы PR #5. |
| **Windows** | **C (Невозможно подтвердить)** | Нет официального Windows-клиента с открытым модельным API и нет разрешённого безподписного API For You. Доступны только публичные видео/Explore/oEmbed/Display API v2 (PR #5) либо WebView2 (`tiktok.com/foryou`). |
| **iOS** | **C (Невозможно подтвердить)** | Жёсткая изоляция приложений (App Sandbox) и отсутствие разрешённого API персональной ленты исключают Companion-наблюдение. Доступны только PR #5 и `WKWebView`. |

---

## 7. Экспериментальный адаптер в `bg-tok-tiktok`

В `crates/bg-tok-tiktok/src/companion.rs` (строго под feature-флагом `experimental-companion`, **выключен по умолчанию**) добавлен `CompanionFeedAdapter`:
- Принимает упорядоченные батчи `CompanionBatch` (только `video_id`, `position`, `observed_at_ms`, `batch_seq`, `has_more`, `account_id` — без cookies и токенов).
- Сохраняет серверный порядок, дедуплицирует ID, поддерживает догрузку новых порций через `Feed::load_next`.
- При смене аккаунта и `logout()` очищает буфер и отменяет устаревшие операции.
- Разрешает каждый ID через `TikTokProvider::resolve_video` и отдаёт байты через `MediaDelivery`.

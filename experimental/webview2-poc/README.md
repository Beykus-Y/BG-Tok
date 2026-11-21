# BG Tok — Independent Windows WebView2 For You PoC

Полностью автономный исследовательский Windows PoC для проверки Microsoft Edge WebView2 в качестве видимого и скрытого источника персональной ленты TikTok `For You`.

**Изоляция:** Не зависит от BeyGram Windows, BG Media, FFI или основного workspace `bg-tok`.

## Обоснование стека (C++20 + Win32 + WebView2LoaderStatic.lib)
- Использует уже установленный в системе **Microsoft Edge WebView2 Runtime** (без поставки Chromium).
- Статическая линковка `WebView2LoaderStatic.lib` даёт один компактный `.exe` (~200–350 КБ) без `.NET`, Node/Python или Playwright.
- Прямой доступ к `ICoreWebView2Controller::put_IsVisible`, `AddScriptToExecuteOnDocumentCreated`, `add_WebMessageReceived`, `get_BrowserProcessId` и Win32 API для замера RAM/CPU всего дерева процессов WebView2.

## Сборка и запуск
Из **Developer PowerShell for VS 2022**:
```powershell
cd experimental\webview2-poc
.\build.ps1
.\build\bg-tok-webview2-poc.exe
```

## Возможности диагностического окна
- **TikTok Login / Open For You**: загрузка `https://www.tiktok.com/` и `https://www.tiktok.com/foryou` с сохранением профиля в `%LOCALAPPDATA%\BGTokWebView2PoC\Profile` (cookies и пароли не извлекаются в хост).
- **Hide/Show WV2**: переключение `ICoreWebView2Controller::put_IsVisible(FALSE/TRUE)` при сохранении контроллера и диагностического окна.
- **Minimize**: сворачивание окна с продолжением мониторинга JS-heartbeat.
- **Метрики в реальном времени**: версия WebView2 Runtime, видимость (`VISIBLE` / `HIDDEN` / `MINIMIZED`), `document.visibilityState`, счётчик JS-heartbeat (включая в скрытом состоянии), количество наблюдаемых `video_id`, RAM хоста и дочерних процессов WebView2, CPU %, размер профиля.
## Безопасность и изоляция
- Приём `WebMessage` разрешён только с origin `https://www.tiktok.com`, с лимитом 16 КБ на сообщение и строгим разбором полей.

## Замеры на живом запуске (Windows 11, WebView2 Runtime `157.0.4341.60`)
- **Размер EXE (`bg-tok-webview2-poc.exe`)**: `318,464` байт (~311 КБ), без поставки Chromium.
- **RAM хоста**: `32.09 МБ`.
- **Дерево процессов WebView2 (`msedgewebview2.exe`)**: `7` дочерних процессов, суммарно `488.14 МБ` RAM при загрузке `https://www.tiktok.com/`.
- **Размер профиля (`%LOCALAPPDATA%\BGTokWebView2PoC\Profile`)**: `17.81 МБ` после первого запуска.

## Оценка применимости к BG Tok (без реализации интеграции)
- **Плюсы**: не требует поставки Chromium (~311 КБ бинарника), использует системный WebView2 Runtime, сохраняет сессию и позволяет пользователю войти в `https://www.tiktok.com/` штатно, а при открытом `/foryou` наблюдать DOM-элементы.
- **Ограничения и риски**:
  1. **Память**: даже один экземпляр WebView2 поднимает ~7 процессов и ~488 МБ RAM.
  2. **Скрытый режим и throttling**: при `put_IsVisible(FALSE)` или сворачивании окна Chromium переводит страницу в `document.visibilityState = "hidden"` и применяет фоновый throttling таймеров/медиа; для интерактивного входа и прокрутки пользователю всё равно нужен видимый экран.
  3. **Персонализация и ToS**: если воспроизведение идёт в `BG Media`, а не в веб-плеере WebView2, TikTok Web не получает реальных событий досмотра/взаимодействия; кроме того, автоматическое извлечение ленты из DOM/XHR веб-клиента для стороннего нативного плеера может нарушать Условия использования TikTok.
  4. **Вывод**: WebView2 подходит как **честная WebView-интеграция** (отдельная вкладка с веб-плеером TikTok), но **не** как лёгкий невидимый backend для нативного `BG Media`.

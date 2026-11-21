// Standalone Windows WebView2 PoC for BG Tok For You research.
// Uses system-installed Microsoft Edge WebView2 Runtime (no Chromium bundle).
// Does NOT read cookies/tokens, does NOT connect to BG Media or BeyGram.

#ifndef NOMINMAX
#define NOMINMAX
#endif
#ifndef UNICODE
#define UNICODE
#endif
#ifndef _UNICODE
#define _UNICODE
#endif

#include "feed_queue.h"

#include <windows.h>
#include <commctrl.h>
#include <psapi.h>
#include <shlobj.h>
#include <tlhelp32.h>
#include <wrl.h>
#include <WebView2.h>

#include <algorithm>
#include <cstdint>
#include <filesystem>
#include <set>
#include <sstream>
#include <string>
#include <vector>

#pragma comment(lib, "user32.lib")
#pragma comment(lib, "gdi32.lib")
#pragma comment(lib, "comctl32.lib")
#pragma comment(lib, "ole32.lib")
#pragma comment(lib, "shell32.lib")
#pragma comment(lib, "psapi.lib")
#pragma comment(lib, "advapi32.lib")

using Microsoft::WRL::Callback;
using Microsoft::WRL::ComPtr;

namespace {

constexpr int IDC_BTN_LOGIN = 101;
constexpr int IDC_BTN_FORYOU = 102;
constexpr int IDC_BTN_TOGGLE_VIS = 103;
constexpr int IDC_BTN_MINIMIZE = 104;
constexpr int IDC_BTN_RELOAD = 105;
constexpr int IDC_BTN_CLEAR = 106;
constexpr int IDC_BTN_DIAG = 109;
constexpr int IDC_BTN_PREV = 110;
constexpr int IDC_BTN_NEXT = 111;
constexpr int IDC_CHK_SYNC = 112;
constexpr int IDC_BTN_PAUSE_MEDIA = 113;
constexpr int IDC_STATUS_TEXT = 107;
constexpr int IDC_LOG_LIST = 108;
constexpr UINT_PTR TIMER_METRICS = 1;
constexpr size_t MAX_WEB_MESSAGE_CHARS = 16 * 1024;

struct AppState {
    HWND main_hwnd = nullptr;
    HWND status_hwnd = nullptr;
    HWND list_hwnd = nullptr;
    HWND wv_host_hwnd = nullptr;
    HWND chk_sync_hwnd = nullptr;

    ComPtr<ICoreWebView2Environment> env;
    ComPtr<ICoreWebView2Controller> controller;
    ComPtr<ICoreWebView2> webview;

    std::wstring profile_dir;
    std::wstring runtime_version = L"checking...";
    std::wstring current_url = L"(none)";
    std::wstring auth_state = L"UNKNOWN";
    std::wstring js_visibility = L"unknown";

    bool wv_visible = true;
    bool media_paused = false;
    bool sync_wv2_on_nav = false;
    uint64_t heartbeat_count = 0;
    uint64_t hidden_heartbeats = 0;
    uint64_t dom_mutations = 0;
    uint64_t links_found = 0;
    uint64_t current_dom_cards = 0;
    uint64_t accumulated_cards = 0;
    uint64_t expected_aborts = 0;
    uint64_t nav_errors = 0;
    uint64_t dom_errors = 0;
    uint64_t wv2_errors = 0;

    bgtok::FeedQueue queue{3};

    // CPU calculation state
    ULONGLONG prev_sys_time = 0;
    ULONGLONG prev_proc_time = 0;
    double last_cpu_pct = 0.0;
};

AppState g_state;

void AddLog(const std::wstring& line) {
    if (!g_state.list_hwnd) return;
    LRESULT count = SendMessageW(g_state.list_hwnd, LB_GETCOUNT, 0, 0);
    if (count > 300) {
        SendMessageW(g_state.list_hwnd, LB_DELETESTRING, 0, 0);
    }
    LRESULT idx = SendMessageW(g_state.list_hwnd, LB_ADDSTRING, 0, reinterpret_cast<LPARAM>(line.c_str()));
    if (idx != LB_ERR) {
        SendMessageW(g_state.list_hwnd, LB_SETTOPINDEX, static_cast<WPARAM>(idx), 0);
    }
}

std::wstring GetProfileDir() {
    PWSTR local_app_data = nullptr;
    std::wstring base = L".";
    if (SUCCEEDED(SHGetKnownFolderPath(FOLDERID_LocalAppData, 0, nullptr, &local_app_data)) && local_app_data) {
        base = local_app_data;
        CoTaskMemFree(local_app_data);
    }
    std::filesystem::path dir = std::filesystem::path(base) / L"BGTokWebView2PoC" / L"Profile";
    std::error_code ec;
    std::filesystem::create_directories(dir, ec);
    return dir.wstring();
}

double DirSizeMb(const std::wstring& path) {
    std::error_code ec;
    uint64_t total = 0;
    for (const auto& entry : std::filesystem::recursive_directory_iterator(
             path, std::filesystem::directory_options::skip_permission_denied, ec)) {
        if (entry.is_regular_file(ec)) {
            total += entry.file_size(ec);
        }
    }
    return static_cast<double>(total) / (1024.0 * 1024.0);
}

std::vector<DWORD> CollectDescendantPids(DWORD root_pid) {
    std::vector<DWORD> pids = {root_pid};
    HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
    if (snap == INVALID_HANDLE_VALUE) return pids;

    std::vector<std::pair<DWORD, DWORD>> procs;
    PROCESSENTRY32W pe{};
    pe.dwSize = sizeof(pe);
    if (Process32FirstW(snap, &pe)) {
        do {
            procs.emplace_back(pe.th32ProcessID, pe.th32ParentProcessID);
        } while (Process32NextW(snap, &pe));
    }
    CloseHandle(snap);

    bool added = true;
    while (added) {
        added = false;
        for (const auto& [pid, ppid] : procs) {
            if (std::find(pids.begin(), pids.end(), ppid) != pids.end() &&
                std::find(pids.begin(), pids.end(), pid) == pids.end()) {
                pids.push_back(pid);
                added = true;
            }
        }
    }
    return pids;
}

ULONGLONG FileTimeToUll(const FILETIME& ft) {
    ULARGE_INTEGER uli{};
    uli.LowPart = ft.dwLowDateTime;
    uli.HighPart = ft.dwHighDateTime;
    return uli.QuadPart;
}

void UpdateMetrics() {
    DWORD self_pid = GetCurrentProcessId();
    std::vector<DWORD> all_pids = CollectDescendantPids(self_pid);

    UINT32 browser_pid = 0;
    if (g_state.webview && SUCCEEDED(g_state.webview->get_BrowserProcessId(&browser_pid)) && browser_pid != 0) {
        for (DWORD p : CollectDescendantPids(browser_pid)) {
            if (std::find(all_pids.begin(), all_pids.end(), p) == all_pids.end()) {
                all_pids.push_back(p);
            }
        }
    }

    SIZE_T host_ws = 0;
    SIZE_T host_priv = 0;
    SIZE_T wv_ws = 0;
    SIZE_T wv_priv = 0;
    ULONGLONG total_proc_time = 0;

    for (DWORD pid : all_pids) {
        HANDLE h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, FALSE, pid);
        if (!h) continue;
        PROCESS_MEMORY_COUNTERS_EX pmc{};
        if (GetProcessMemoryInfo(h, reinterpret_cast<PROCESS_MEMORY_COUNTERS*>(&pmc), sizeof(pmc))) {
            if (pid == self_pid) {
                host_ws += pmc.WorkingSetSize;
                host_priv += pmc.PrivateUsage;
            } else {
                wv_ws += pmc.WorkingSetSize;
                wv_priv += pmc.PrivateUsage;
            }
        }
        FILETIME ct{}, et{}, kt{}, ut{};
        if (GetProcessTimes(h, &ct, &et, &kt, &ut)) {
            total_proc_time += FileTimeToUll(kt) + FileTimeToUll(ut);
        }
        CloseHandle(h);
    }

    FILETIME idle{}, sys_k{}, sys_u{};
    if (GetSystemTimes(&idle, &sys_k, &sys_u)) {
        ULONGLONG sys_total = FileTimeToUll(sys_k) + FileTimeToUll(sys_u);
        if (g_state.prev_sys_time != 0 && sys_total > g_state.prev_sys_time) {
            ULONGLONG d_sys = sys_total - g_state.prev_sys_time;
            ULONGLONG d_proc = (total_proc_time >= g_state.prev_proc_time) ? (total_proc_time - g_state.prev_proc_time) : 0;
            g_state.last_cpu_pct = (static_cast<double>(d_proc) * 100.0) / static_cast<double>(d_sys);
        }
        g_state.prev_sys_time = sys_total;
        g_state.prev_proc_time = total_proc_time;
    }

    double profile_mb = DirSizeMb(g_state.profile_dir);
    BOOL minimized = IsIconic(g_state.main_hwnd);
    const bgtok::FeedItem* cur = g_state.queue.Current();
    std::wstring cur_id = cur ? cur->video_id : L"-";
    std::wstring cur_idx = g_state.queue.CurrentIndex().has_value()
        ? std::to_wstring(*g_state.queue.CurrentIndex() + 1)
        : L"0";

    std::wostringstream ss;
    ss.precision(1);
    ss << std::fixed;
    ss << L"WV2: " << (minimized ? L"MINIMIZED" : (g_state.wv_visible ? L"VISIBLE" : L"HIDDEN"))
       << L" | Media: " << (g_state.media_paused ? L"PAUSED" : L"ACTIVE")
       << L" | Auth: " << g_state.auth_state
       << L" | JS vis: " << g_state.js_visibility
       << L"\r\nFeed: " << bgtok::FeedStateToString(g_state.queue.State())
       << L" | Cur: " << cur_id << L" (" << cur_idx << L"/" << g_state.queue.Length()
       << L", rem: " << g_state.queue.Remaining() << L")"
       << L"\r\nLast ID: " << (g_state.queue.LastVideoId().empty() ? L"-" : g_state.queue.LastVideoId())
       << L" | Dup: " << g_state.queue.DuplicatesCount()
       << L" | Skip(ad/live): " << g_state.queue.SkippedCount()
       << L" | Gaps: " << g_state.queue.GapsCount()
       << L"\r\nDOM Cards: " << g_state.current_dom_cards
       << L" (acc: " << g_state.accumulated_cards << L")"
       << L" | Links: " << g_state.links_found
       << L" | Mut: " << g_state.dom_mutations
       << L"\r\nHB: " << g_state.heartbeat_count << L" (hid: " << g_state.hidden_heartbeats << L")"
       << L" | Aborts: " << g_state.expected_aborts
       << L" | Err(nav/dom/wv2): " << g_state.nav_errors << L"/" << g_state.dom_errors << L"/" << g_state.wv2_errors
       << L"\r\nHost WS/Priv: " << (static_cast<double>(host_ws) / (1024.0 * 1024.0))
       << L"/" << (static_cast<double>(host_priv) / (1024.0 * 1024.0)) << L" MB"
       << L" | WV2(" << (all_pids.size() > 0 ? all_pids.size() - 1 : 0) << L"): "
       << (static_cast<double>(wv_ws) / (1024.0 * 1024.0))
       << L"/" << (static_cast<double>(wv_priv) / (1024.0 * 1024.0)) << L" MB"
       << L"\r\nCPU: " << g_state.last_cpu_pct << L"% | Profile: " << profile_mb << L" MB";

    SetWindowTextW(g_state.status_hwnd, ss.str().c_str());
}

// Simple JSON field extraction for trusted origin messages (no external JSON dependency).
std::wstring ExtractJsonString(const std::wstring& json, const std::wstring& key) {
    std::wstring needle = L"\"" + key + L"\":\"";
    size_t pos = json.find(needle);
    if (pos == std::wstring::npos) return L"";
    pos += needle.size();
    std::wstring out;
    while (pos < json.size() && json[pos] != L'"') {
        if (json[pos] == L'\\' && pos + 1 < json.size()) {
            pos++;
        }
        out.push_back(json[pos++]);
    }
    return out;
}

bool ExtractJsonBool(const std::wstring& json, const std::wstring& key, bool default_val = false) {
    std::wstring needle = L"\"" + key + L"\":";
    size_t pos = json.find(needle);
    if (pos == std::wstring::npos) return default_val;
    pos += needle.size();
    if (json.compare(pos, 4, L"true") == 0) return true;
    if (json.compare(pos, 5, L"false") == 0) return false;
    return default_val;
}

uint64_t ExtractJsonUint(const std::wstring& json, const std::wstring& key) {
    std::wstring needle = L"\"" + key + L"\":";
    size_t pos = json.find(needle);
    if (pos == std::wstring::npos) return 0;
    pos += needle.size();
    uint64_t val = 0;
    while (pos < json.size() && iswdigit(json[pos])) {
        val = val * 10 + static_cast<uint64_t>(json[pos] - L'0');
        pos++;
    }
    return val;
}

bool IsTrustedTikTokOrigin(const std::wstring& source) {
    // Exact origin check: must be "https://www.tiktok.com" or "https://www.tiktok.com/..."
    // Rejects lookalike hosts like "https://www.tiktok.com.evil.com/".
    if (source == L"https://www.tiktok.com") return true;
    const std::wstring prefix = L"https://www.tiktok.com/";
    return source.size() >= prefix.size() && source.compare(0, prefix.size(), prefix) == 0;
}

void HandlePageMessage(const std::wstring& source, const std::wstring& raw_json) {
    if (!IsTrustedTikTokOrigin(source)) {
        AddLog(L"[BLOCKED] Untrusted origin: " + source);
        return;
    }
    if (raw_json.size() > MAX_WEB_MESSAGE_CHARS) {
        AddLog(L"[REJECTED] Oversized web message");
        return;
    }

    std::wstring kind = ExtractJsonString(raw_json, L"kind");
    if (kind == L"heartbeat") {
        g_state.heartbeat_count++;
        if (!g_state.wv_visible || IsIconic(g_state.main_hwnd)) {
            g_state.hidden_heartbeats++;
        }
        g_state.js_visibility = ExtractJsonString(raw_json, L"vis");
        bool logged_in = ExtractJsonBool(raw_json, L"loggedIn", false);
        g_state.auth_state = logged_in ? L"LOGGED_IN" : L"GUEST";
    } else if (kind == L"scan_stats") {
        g_state.dom_mutations = ExtractJsonUint(raw_json, L"mutations");
        g_state.links_found = ExtractJsonUint(raw_json, L"links");
        g_state.current_dom_cards = ExtractJsonUint(raw_json, L"cards");
        g_state.accumulated_cards += ExtractJsonUint(raw_json, L"processed");
    } else if (kind == L"feed_item") {
        bgtok::FeedItem item{};
        item.video_id = ExtractJsonString(raw_json, L"id");
        item.author = ExtractJsonString(raw_json, L"author");
        item.title = ExtractJsonString(raw_json, L"title");
        item.item_type = ExtractJsonString(raw_json, L"itemType");
        item.src_kind = ExtractJsonString(raw_json, L"srcKind");
        item.dom_pos = ExtractJsonUint(raw_json, L"pos");
        item.observed_at_ms = GetTickCount64();

        if (!item.video_id.empty() && std::all_of(item.video_id.begin(), item.video_id.end(), iswdigit)) {
            auto r = g_state.queue.Push(item, g_state.queue.generation());
            if (r.accepted) {
                std::wstring line = L"#" + std::to_wstring(g_state.queue.Length()) +
                                    L" (dom@" + std::to_wstring(item.dom_pos) + L"," + item.src_kind +
                                    L") [" + item.video_id + L"] @" + item.author + L" - " + item.title.substr(0, 44);
                AddLog(line);
            }
        }
    } else if (kind == L"abort") {
        g_state.expected_aborts++;
    } else if (kind == L"dom_err") {
        g_state.dom_errors++;
        std::wstring msg = ExtractJsonString(raw_json, L"msg");
        if (!msg.empty()) AddLog(L"[DOM-ERR] " + msg);
    } else if (kind == L"diag") {
        std::wstring msg = ExtractJsonString(raw_json, L"msg");
        if (!msg.empty()) {
            AddLog(L"[DIAG] " + msg);
        }
    }
}

const wchar_t* kObserverScript = LR"JS(
(() => {
  if (window.__bgTokObserverInstalled) return;

  function post(obj) {
    try {
      if (window.chrome && window.chrome.webview) {
        window.chrome.webview.postMessage(JSON.stringify(obj));
      }
    } catch (_) {}
  }

  try {
    let mutations = 0;
    let scanTimer = null;
    let lastScanMs = 0;
    let observerStarted = false;
    const loggedDiagKeys = new Set();

    let mediaPaused = false;

    function isAbortLike(err) {
      const s = String(err && err.name ? err.name : '') + ' ' + String(err && err.message ? err.message : err);
      return /AbortError|aborted|interrupted by a call to pause|The play\(\) request was interrupted/i.test(s);
    }

    window.addEventListener('error', (e) => {
      if (isAbortLike(e && (e.error || e.message))) { post({ kind: 'abort' }); return; }
      post({ kind: 'dom_err', msg: 'JS error: ' + String(e && e.message ? e.message : e) });
    });
    window.addEventListener('unhandledrejection', (e) => {
      if (isAbortLike(e && e.reason)) { post({ kind: 'abort' }); return; }
      post({ kind: 'dom_err', msg: 'JS rejection: ' + String(e && e.reason ? e.reason : e) });
    });

    function detectLoggedIn() {
      if (!document || !document.querySelector) return false;
      const loginBtn = document.querySelector('[data-e2e="top-login-button"]');
      const profile = document.querySelector('[data-e2e="profile-icon"], [data-e2e="nav-profile"]');
      return !!profile && !loginBtn;
    }

    function classifyCard(card) {
      if (card.querySelector('[data-e2e="ad-badge"], [data-e2e="sponsored-tag"], a[href*="/ad/"]')) {
        return 'ad';
      }
      if (card.querySelector('[data-e2e="live-badge"], [data-e2e="live-room"], a[href*="/live"]')) {
        return 'live';
      }
      return 'video';
    }

    function extractFromCard(card) {
      const link = card.querySelector('a[href*="/video/"], a[href*="/photo/"]');
      if (link) {
        const m = (link.getAttribute('href') || '').match(/\/@([^/]+)\/(?:video|photo)\/(\d+)/);
        if (m) {
          return { id: m[2], author: m[1], srcKind: 'href' };
        }
      }
      const xg = card.id && card.id.startsWith('xgwrapper-') ? card : card.querySelector('[id^="xgwrapper-"]');
      if (xg) {
        const m = xg.id.match(/^xgwrapper-\d+-(\d{10,})$/);
        if (m) {
          const authorEl = card.querySelector('[data-e2e="video-author-uniqueid"], a[href^="/@"]');
          let author = authorEl ? (authorEl.textContent || '').replace(/^@/, '').trim() : '';
          if (!author && authorEl && authorEl.getAttribute('href')) {
            const am = authorEl.getAttribute('href').match(/^\/@([^/?]+)/);
            if (am) author = am[1];
          }
          return { id: m[1], author: author || 'unknown', srcKind: 'xgwrapper' };
        }
      }
      return null;
    }

    function scanFeed() {
      try {
        lastScanMs = Date.now();
        if (!location.pathname.startsWith('/foryou')) return;
        if (!document || !document.querySelectorAll) return;

        const links = document.querySelectorAll('a[href*="/video/"], a[href*="/photo/"]');
        const cards = Array.from(document.querySelectorAll('article, [data-e2e="recommend-list-item-container"]'));

        let processed = 0;
        let ads = 0;
        let live = 0;
        let unidentified = 0;

        if (cards.length > 0) {
          cards.sort((a, b) => a.getBoundingClientRect().top - b.getBoundingClientRect().top);
          cards.forEach((card, idx) => {
            processed++;
            const rawPos = card.getAttribute('data-scroll-index');
            const pos = rawPos !== null && /^\d+$/.test(rawPos) ? Number(rawPos) : idx;
            const itemType = classifyCard(card);
            if (itemType === 'ad') ads++;
            if (itemType === 'live') live++;

            const info = extractFromCard(card);
            if (info) {
              const desc = card.querySelector('[data-e2e="video-desc"]')?.textContent || '';
              post({
                kind: 'feed_item',
                id: info.id,
                author: info.author,
                title: desc.slice(0, 80),
                itemType,
                srcKind: info.srcKind,
                pos
              });
            } else {
              unidentified++;
              const key = `card:${pos}:${itemType}:${card.tagName}:${card.getAttribute('data-e2e') || ''}`;
              if (!loggedDiagKeys.has(key) && loggedDiagKeys.size < 40) {
                loggedDiagKeys.add(key);
                const hasVideo = !!card.querySelector('video');
                const xgId = card.querySelector('[id^="xgwrapper-"]')?.id || '';
                post({
                  kind: 'diag',
                  msg: `Unidentified pos=${pos} type=${itemType} tag=${card.tagName} e2e=${card.getAttribute('data-e2e') || '-'} video=${hasVideo} xg=${xgId}`
                });
              }
            }
          });
        } else {
          links.forEach((a, idx) => {
            processed++;
            const m = (a.getAttribute('href') || '').match(/\/@([^/]+)\/(?:video|photo)\/(\d+)/);
            if (m) {
              post({
                kind: 'feed_item',
                id: m[2],
                author: m[1],
                title: '',
                itemType: 'video',
                srcKind: 'href-fallback',
                pos: idx
              });
            } else {
              unidentified++;
            }
          });
        }

        post({
          kind: 'scan_stats',
          mutations,
          links: links.length,
          cards: cards.length,
          processed,
          ads,
          live,
          unidentified
        });
      } catch (err) {
        post({ kind: 'diag', msg: 'scanFeed error: ' + String(err && err.message ? err.message : err) });
      }
    }

    function scheduleScan() {
      const elapsed = Date.now() - lastScanMs;
      if (elapsed >= 250) {
        if (scanTimer) { clearTimeout(scanTimer); scanTimer = null; }
        scanFeed();
      } else if (!scanTimer) {
        scanTimer = setTimeout(() => {
          scanTimer = null;
          scanFeed();
        }, 250 - elapsed);
      }
    }

    // Register Diag DOM and heartbeat BEFORE DOM observer so they work regardless of DOM readiness.
)JS" LR"JS(
    window.__bgTokDiagDump = () => {
      try {
        const articles = document.querySelectorAll('article').length;
        const rec = document.querySelectorAll('[data-e2e="recommend-list-item-container"]').length;
        const vids = document.querySelectorAll('video').length;
        const xgs = document.querySelectorAll('[id^="xgwrapper-"]').length;
        const links = document.querySelectorAll('a[href*="/video/"]').length;
        const idxs = Array.from(document.querySelectorAll('[data-scroll-index]')).map(e => e.getAttribute('data-scroll-index')).join(',');
        post({
          kind: 'diag',
          msg: `DOM path=${location.pathname} obs=${observerStarted} articles=${articles} rec=${rec} videos=${vids} xg=${xgs} links=${links} scrollIdx=[${idxs}]`
        });
        scanFeed();
        return 'ok';
      } catch (err) {
        post({ kind: 'diag', msg: 'DiagDump error: ' + String(err && err.message ? err.message : err) });
        return 'error';
      }
    };

    // Single-step user-initiated navigation for Sync WV2 test (never auto-scrolls).
    window.__bgTokStepNav = (dir) => {
      try {
        const key = dir > 0 ? 'ArrowDown' : 'ArrowUp';
        const code = dir > 0 ? 40 : 38;
        const ev = new KeyboardEvent('keydown', { key, code: key, keyCode: code, which: code, bubbles: true, cancelable: true });
        document.dispatchEvent(ev);
        if (document.activeElement) document.activeElement.dispatchEvent(ev);
        scheduleScan();
        return 'ok';
      } catch (err) {
        post({ kind: 'dom_err', msg: 'StepNav error: ' + String(err && err.message ? err.message : err) });
        return 'error';
      }
    };

    window.__bgTokSetMediaPaused = (paused) => {
      try {
        mediaPaused = !!paused;
        for (const v of document.querySelectorAll('video, audio')) {
          if (mediaPaused) v.pause();
        }
        return 'ok';
      } catch (err) {
        post({ kind: 'dom_err', msg: 'MediaPause error: ' + String(err && err.message ? err.message : err) });
        return 'error';
      }
    };

    setInterval(() => {
      if (mediaPaused) {
        for (const v of document.querySelectorAll('video, audio')) {
          if (!v.paused) v.pause();
        }
      }
      post({
        kind: 'heartbeat',
        vis: document.visibilityState || 'unknown',
        loggedIn: detectLoggedIn()
      });
      scheduleScan();
    }, 1000);

    function startObserver() {
      if (observerStarted) return;
      const target = document.documentElement || document.body;
      if (!target) return;
      try {
        const mo = new MutationObserver((list) => {
          mutations += list.length;
          scheduleScan();
        });
        mo.observe(target, { childList: true, subtree: true });
        window.addEventListener('scroll', scheduleScan, { passive: true, capture: true });
        observerStarted = true;
        scheduleScan();
      } catch (err) {
        post({ kind: 'diag', msg: 'Observer start error: ' + String(err && err.message ? err.message : err) });
      }
    }

    if (document.documentElement) {
      startObserver();
    } else {
      document.addEventListener('DOMContentLoaded', startObserver, { once: true });
      document.addEventListener('readystatechange', startObserver);
    }

    window.__bgTokObserverInstalled = true;
  } catch (err) {
    post({ kind: 'diag', msg: 'Observer fatal init error: ' + String(err && err.message ? err.message : err) });
  }
})();
)JS";

void ResizeWebView() {
    if (!g_state.controller || !g_state.wv_host_hwnd) return;
    RECT rc{};
    GetClientRect(g_state.wv_host_hwnd, &rc);
    g_state.controller->put_Bounds(rc);
}

void InitWebView2() {
    g_state.profile_dir = GetProfileDir();
    CreateCoreWebView2EnvironmentWithOptions(
        nullptr, g_state.profile_dir.c_str(), nullptr,
        Callback<ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler>(
            [](HRESULT result, ICoreWebView2Environment* env) -> HRESULT {
                if (FAILED(result) || !env) {
                    AddLog(L"[ERROR] WebView2 Runtime init failed");
                    return result;
                }
                g_state.env = env;
                LPWSTR ver = nullptr;
                if (SUCCEEDED(env->get_BrowserVersionString(&ver)) && ver) {
                    g_state.runtime_version = ver;
                    CoTaskMemFree(ver);
                }
                AddLog(L"[OK] WebView2 Runtime: " + g_state.runtime_version);

                return env->CreateCoreWebView2Controller(
                    g_state.wv_host_hwnd,
                    Callback<ICoreWebView2CreateCoreWebView2ControllerCompletedHandler>(
                        [](HRESULT res, ICoreWebView2Controller* controller) -> HRESULT {
                            if (FAILED(res) || !controller) {
                                AddLog(L"[ERROR] CreateCoreWebView2Controller failed");
                                return res;
                            }
                            g_state.controller = controller;
                            controller->get_CoreWebView2(&g_state.webview);
                            ResizeWebView();

                            if (g_state.webview) {
                                EventRegistrationToken tok{};
                                g_state.webview->add_SourceChanged(
                                    Callback<ICoreWebView2SourceChangedEventHandler>(
                                        [](ICoreWebView2* sender, ICoreWebView2SourceChangedEventArgs*) -> HRESULT {
                                            LPWSTR uri = nullptr;
                                            if (SUCCEEDED(sender->get_Source(&uri)) && uri) {
                                                g_state.current_url = uri;
                                                CoTaskMemFree(uri);
                                            }
                                            return S_OK;
                                        })
                                        .Get(),
                                    &tok);

                                g_state.webview->add_WebMessageReceived(
                                    Callback<ICoreWebView2WebMessageReceivedEventHandler>(
                                        [](ICoreWebView2*, ICoreWebView2WebMessageReceivedEventArgs* args) -> HRESULT {
                                            LPWSTR src = nullptr;
                                            LPWSTR msg = nullptr;
                                            args->get_Source(&src);
                                            args->TryGetWebMessageAsString(&msg);
                                            if (src && msg) {
                                                HandlePageMessage(src, msg);
                                            }
                                            if (src) CoTaskMemFree(src);
                                            if (msg) CoTaskMemFree(msg);
                                            return S_OK;
                                        })
                                        .Get(),
                                    &tok);

                                // Wait for AddScriptToExecuteOnDocumentCreated before initial Navigate.
                                g_state.webview->AddScriptToExecuteOnDocumentCreated(
                                    kObserverScript,
                                    Callback<ICoreWebView2AddScriptToExecuteOnDocumentCreatedCompletedHandler>(
                                        [](HRESULT script_hr, LPCWSTR) -> HRESULT {
                                            if (FAILED(script_hr)) {
                                                AddLog(L"[ERROR] AddScriptToExecuteOnDocumentCreated failed");
                                            } else {
                                                AddLog(L"[OK] Observer script registered");
                                            }
                                            if (g_state.webview) {
                                                g_state.webview->Navigate(L"https://www.tiktok.com/");
                                            }
                                            return S_OK;
                                        })
                                        .Get());
                            }
                            return S_OK;
                        })
                        .Get());
            })
            .Get());
}

void LayoutControls(HWND hwnd) {
    RECT rc{};
    GetClientRect(hwnd, &rc);
    int w = rc.right - rc.left;
    int h = rc.bottom - rc.top;
    int left_w = 500;
    if (w < 860) left_w = w / 2;

    if (g_state.status_hwnd) MoveWindow(g_state.status_hwnd, 10, 118, left_w - 20, 148, TRUE);
    if (g_state.list_hwnd) MoveWindow(g_state.list_hwnd, 10, 272, left_w - 20, std::max(100, h - 282), TRUE);
    if (g_state.wv_host_hwnd) MoveWindow(g_state.wv_host_hwnd, left_w, 0, std::max(100, w - left_w), h, TRUE);
    ResizeWebView();
}

LRESULT CALLBACK WndProc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
    switch (msg) {
        case WM_CREATE: {
            auto btn_id = [](int id) { return reinterpret_cast<HMENU>(static_cast<INT_PTR>(id)); };
            CreateWindowW(L"BUTTON", L"TikTok Login", WS_CHILD | WS_VISIBLE, 10, 10, 105, 28, hwnd, btn_id(IDC_BTN_LOGIN), nullptr, nullptr);
            CreateWindowW(L"BUTTON", L"Open For You", WS_CHILD | WS_VISIBLE, 120, 10, 105, 28, hwnd, btn_id(IDC_BTN_FORYOU), nullptr, nullptr);
            CreateWindowW(L"BUTTON", L"Hide/Show WV2", WS_CHILD | WS_VISIBLE, 230, 10, 115, 28, hwnd, btn_id(IDC_BTN_TOGGLE_VIS), nullptr, nullptr);
            CreateWindowW(L"BUTTON", L"Minimize", WS_CHILD | WS_VISIBLE, 350, 10, 95, 28, hwnd, btn_id(IDC_BTN_MINIMIZE), nullptr, nullptr);

            CreateWindowW(L"BUTTON", L"Reload", WS_CHILD | WS_VISIBLE, 10, 44, 80, 28, hwnd, btn_id(IDC_BTN_RELOAD), nullptr, nullptr);
            CreateWindowW(L"BUTTON", L"Reset Queue", WS_CHILD | WS_VISIBLE, 95, 44, 105, 28, hwnd, btn_id(IDC_BTN_CLEAR), nullptr, nullptr);
            CreateWindowW(L"BUTTON", L"Diag DOM", WS_CHILD | WS_VISIBLE, 205, 44, 95, 28, hwnd, btn_id(IDC_BTN_DIAG), nullptr, nullptr);
            CreateWindowW(L"BUTTON", L"Pause Media", WS_CHILD | WS_VISIBLE, 305, 44, 110, 28, hwnd, btn_id(IDC_BTN_PAUSE_MEDIA), nullptr, nullptr);

            CreateWindowW(L"BUTTON", L"< Prev", WS_CHILD | WS_VISIBLE, 10, 78, 85, 28, hwnd, btn_id(IDC_BTN_PREV), nullptr, nullptr);
            CreateWindowW(L"BUTTON", L"Next >", WS_CHILD | WS_VISIBLE, 100, 78, 85, 28, hwnd, btn_id(IDC_BTN_NEXT), nullptr, nullptr);
            g_state.chk_sync_hwnd = CreateWindowW(L"BUTTON", L"Sync WV2 on Prev/Next", WS_CHILD | WS_VISIBLE | BS_AUTOCHECKBOX, 195, 78, 190, 28, hwnd, btn_id(IDC_CHK_SYNC), nullptr, nullptr);

            g_state.status_hwnd = CreateWindowW(L"STATIC", L"Initializing WebView2...", WS_CHILD | WS_VISIBLE | SS_LEFT, 10, 118, 480, 148, hwnd, btn_id(IDC_STATUS_TEXT), nullptr, nullptr);
            g_state.list_hwnd = CreateWindowW(L"LISTBOX", nullptr, WS_CHILD | WS_VISIBLE | WS_BORDER | WS_VSCROLL | LBS_NOINTEGRALHEIGHT, 10, 272, 480, 400, hwnd, btn_id(IDC_LOG_LIST), nullptr, nullptr);
            g_state.wv_host_hwnd = CreateWindowW(L"STATIC", nullptr, WS_CHILD | WS_VISIBLE, 500, 0, 600, 700, hwnd, nullptr, nullptr, nullptr);

            SetTimer(hwnd, TIMER_METRICS, 1000, nullptr);
            InitWebView2();
            return 0;
        }
        case WM_SIZE:
            LayoutControls(hwnd);
            return 0;
        case WM_TIMER:
            if (wp == TIMER_METRICS) UpdateMetrics();
            return 0;
        case WM_COMMAND:
            switch (LOWORD(wp)) {
                case IDC_BTN_LOGIN:
                    if (g_state.webview && FAILED(g_state.webview->Navigate(L"https://www.tiktok.com/"))) {
                        g_state.nav_errors++;
                    }
                    AddLog(L"[NAV] https://www.tiktok.com/");
                    break;
                case IDC_BTN_FORYOU:
                    if (g_state.webview && FAILED(g_state.webview->Navigate(L"https://www.tiktok.com/foryou"))) {
                        g_state.nav_errors++;
                    }
                    AddLog(L"[NAV] https://www.tiktok.com/foryou");
                    break;
                case IDC_BTN_TOGGLE_VIS:
                    if (g_state.controller) {
                        g_state.wv_visible = !g_state.wv_visible;
                        if (FAILED(g_state.controller->put_IsVisible(g_state.wv_visible ? TRUE : FALSE))) {
                            g_state.wv2_errors++;
                        }
                        AddLog(g_state.wv_visible ? L"[MODE] WebView2 VISIBLE" : L"[MODE] WebView2 HIDDEN (controller alive)");
                    }
                    break;
                case IDC_BTN_MINIMIZE:
                    ShowWindow(hwnd, SW_MINIMIZE);
                    AddLog(L"[MODE] Window MINIMIZED");
                    break;
                case IDC_BTN_RELOAD:
                    if (g_state.webview && FAILED(g_state.webview->Reload())) {
                        g_state.nav_errors++;
                    }
                    AddLog(L"[NAV] Reload");
                    break;
                case IDC_BTN_CLEAR:
                    g_state.queue.ResetForAccountSwitch();
                    SendMessageW(g_state.list_hwnd, LB_RESETCONTENT, 0, 0);
                    AddLog(L"[QUEUE] Reset (account switch / clear), gen=" + std::to_wstring(g_state.queue.generation()));
                    break;
                case IDC_BTN_PREV:
                    if (g_state.queue.Previous()) {
                        const auto* cur = g_state.queue.Current();
                        AddLog(L"[QUEUE] Prev -> " + (cur ? cur->video_id : L"-"));
                    } else {
                        AddLog(L"[QUEUE] At start of queue");
                    }
                    if (g_state.sync_wv2_on_nav && g_state.webview) {
                        g_state.webview->ExecuteScript(L"window.__bgTokStepNav && window.__bgTokStepNav(-1)", nullptr);
                    }
                    break;
                case IDC_BTN_NEXT:
                    if (g_state.queue.Next()) {
                        const auto* cur = g_state.queue.Current();
                        AddLog(L"[QUEUE] Next -> " + (cur ? cur->video_id : L"-"));
                    } else {
                        AddLog(L"[QUEUE] End of queue (NEEDS_MORE_ITEMS)");
                    }
                    if (g_state.sync_wv2_on_nav && g_state.webview) {
                        g_state.webview->ExecuteScript(L"window.__bgTokStepNav && window.__bgTokStepNav(1)", nullptr);
                    }
                    break;
                case IDC_CHK_SYNC:
                    g_state.sync_wv2_on_nav = (SendMessageW(g_state.chk_sync_hwnd, BM_GETCHECK, 0, 0) == BST_CHECKED);
                    AddLog(g_state.sync_wv2_on_nav ? L"[SYNC] WV2 step-sync ON" : L"[SYNC] WV2 step-sync OFF");
                    break;
                case IDC_BTN_PAUSE_MEDIA:
                    g_state.media_paused = !g_state.media_paused;
                    if (g_state.webview) {
                        g_state.webview->ExecuteScript(
                            g_state.media_paused
                                ? L"window.__bgTokSetMediaPaused && window.__bgTokSetMediaPaused(true)"
                                : L"window.__bgTokSetMediaPaused && window.__bgTokSetMediaPaused(false)",
                            nullptr);
                    }
                    AddLog(g_state.media_paused ? L"[MEDIA] Paused in WebView2" : L"[MEDIA] Resumed in WebView2");
                    break;
                case IDC_BTN_DIAG:
                    if (g_state.webview) {
                        g_state.webview->ExecuteScript(
                            L"(function(){ return window.__bgTokDiagDump ? window.__bgTokDiagDump() : 'missing'; })()",
                            Callback<ICoreWebView2ExecuteScriptCompletedHandler>(
                                [](HRESULT hr, LPCWSTR result_json) -> HRESULT {
                                    if (FAILED(hr)) {
                                        AddLog(L"[DIAG-ERR] ExecuteScript hr=" + std::to_wstring(hr));
                                    } else if (result_json && std::wstring(result_json) != L"\"ok\"") {
                                        AddLog(L"[DIAG-RES] " + std::wstring(result_json));
                                    }
                                    return S_OK;
                                })
                                .Get());
                    }
                    break;
            }
            return 0;
        case WM_DESTROY:
            KillTimer(hwnd, TIMER_METRICS);
            if (g_state.controller) g_state.controller->Close();
            PostQuitMessage(0);
            return 0;
    }
    return DefWindowProcW(hwnd, msg, wp, lp);
}

}  // namespace

int WINAPI wWinMain(HINSTANCE hInstance, HINSTANCE, PWSTR, int nCmdShow) {
    CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
    InitCommonControls();

    WNDCLASSW wc{};
    wc.lpfnWndProc = WndProc;
    wc.hInstance = hInstance;
    wc.hbrBackground = reinterpret_cast<HBRUSH>(COLOR_WINDOW + 1);
    wc.hCursor = LoadCursorW(nullptr, IDC_ARROW);
    wc.lpszClassName = L"BGTokWebView2PoC";
    RegisterClassW(&wc);

    g_state.main_hwnd = CreateWindowW(
        wc.lpszClassName, L"BG Tok - Independent WebView2 For You PoC",
        WS_OVERLAPPEDWINDOW, CW_USEDEFAULT, CW_USEDEFAULT, 1180, 760,
        nullptr, nullptr, hInstance, nullptr);

    ShowWindow(g_state.main_hwnd, nCmdShow);
    UpdateWindow(g_state.main_hwnd);

    MSG msg{};
    while (GetMessageW(&msg, nullptr, 0, 0)) {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    CoUninitialize();
    return 0;
}

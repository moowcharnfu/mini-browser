#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::{
    webview::PageLoadEvent, AppHandle, Emitter, Manager, LogicalPosition, LogicalSize,
    WebviewBuilder, WebviewUrl, Webview, WindowEvent,
};
use tauri::TitleBarStyle;

/// 调试日志：仅在 debug 构建中输出
macro_rules! debug_log {
    ($($arg:tt)*) => {
        if cfg!(debug_assertions) {
            std::eprintln!($($arg)*);
        }
    };
}

/// 中文右键菜单脚本（注入到每个 content webview）
const CTX_MENU_SCRIPT: &str = r#"(function(){
    function showMenu(e){
        e.preventDefault();
        e.stopPropagation();
        var m=document.getElementById('__mb_menu');if(m)m.remove();
        var l=e.target.closest('a');var lu=l&&l.href;
        var menu=document.createElement('div');
        menu.id='__mb_menu';
        menu.style.cssText='position:fixed;z-index:999999;background:#1a1a2e;border:1px solid #2d2d4a;border-radius:8px;padding:4px;min-width:140px;font:13px system-ui,sans-serif;color:#e8e8f0;box-shadow:0 4px 12px rgba(0,0,0,0.4)';
        function add(t,f){
            var d=document.createElement('div');
            d.textContent=t;
            d.style.cssText='padding:6px 14px;cursor:pointer;border-radius:4px;';
            d.onmouseenter=function(){this.style.background='#2d2d52'};
            d.onmouseleave=function(){this.style.background='transparent'};
            d.onclick=function(ev){ev.stopPropagation();menu.remove();f();};
            menu.appendChild(d);
        }
        if(lu){
            add('打开链接',function(){window.location.href=lu});
            add('复制链接',function(){try{navigator.clipboard.writeText(lu)}catch(e){}});
            menu.appendChild(document.createElement('hr'));
        }
        add('后退',function(){window.history.back()});
        add('前进',function(){window.history.forward()});
        add('刷新',function(){window.location.reload()});
        add('复制',function(){try{navigator.clipboard.writeText(document.getSelection().toString())}catch(e){}});
        menu.style.left=Math.min(e.clientX,window.innerWidth-160)+'px';
        var contentH = window.__mb_content_height || window.innerHeight;
        menu.style.top=Math.min(e.clientY, contentH - 200)+'px';
        if(document.body)document.body.appendChild(menu);
    }
    document.addEventListener('contextmenu',showMenu);
    document.addEventListener('click',function(){var m=document.getElementById('__mb_menu');if(m)m.remove()});
    document.addEventListener('keydown',function(e){if(e.key==='Escape'){var m=document.getElementById('__mb_menu');if(m)m.remove()}});
})();"#;

/// 双击打开链接脚本
const DBL_CLICK_SCRIPT: &str = r#"(function(){
    document.addEventListener('click',function(e){
        var a=e.target.closest('a');
        if(!a||!a.href)return;
        if(e.button!==0)return;
        var h=a.getAttribute('href')||'';
        if(h.startsWith('javascript:')||h==='#'||h.startsWith('#'))return;
        if(e.detail!==2){
            e.preventDefault();
            e.stopPropagation();
        }
    },true);
})();"#;

/// 最大标签数量限制
const MAX_TABS: usize = 10;

/// 睡眠脚本：暂停后台 WebView 的 JS 执行
const SUSPEND_SCRIPT: &str = r#"(function(){
    if (window.__mb_suspended) return;
    window.__mb_suspended = true;

    window.__mb_orig_setTimeout = window.setTimeout;
    window.__mb_orig_setInterval = window.setInterval;
    window.__mb_orig_requestAnimationFrame = window.requestAnimationFrame;

    window.setTimeout = function() { return -1; };
    window.setInterval = function() { return -1; };
    window.requestAnimationFrame = function() { return -1; };

    document.querySelectorAll('video, audio').forEach(function(el) {
        if (!el.paused) {
            el.__mb_was_playing = true;
            el.pause();
        }
    });

    console.log('[mini-browser] tab suspended');
})();"#;

/// 恢复脚本：恢复前台 WebView 的 JS 执行
const RESUME_SCRIPT: &str = r#"(function(){
    if (!window.__mb_suspended) return;
    window.__mb_suspended = false;

    window.setTimeout = window.__mb_orig_setTimeout;
    window.setInterval = window.__mb_orig_setInterval;
    window.requestAnimationFrame = window.__mb_orig_requestAnimationFrame;

    document.querySelectorAll('video, audio').forEach(function(el) {
        if (el.__mb_was_playing) {
            el.play().catch(function(){});
            el.__mb_was_playing = false;
        }
    });

    console.log('[mini-browser] tab resumed');
})();"#;

#[derive(Clone, serde::Serialize)]
struct UrlPayload {
    tab_id: i32,
    url: String,
}

#[derive(Clone, serde::Serialize)]
struct LoadingPayload {
    tab_id: i32,
    loading: bool,
}

/// 存储所有 WebView 的句柄
struct WebViewPool {
    webviews: HashMap<i32, Webview>,
    active_tab_id: Option<i32>,
    /// 内容区域在窗口中的位置（CSS 逻辑像素），由前端 ResizeObserver 实时更新
    content_x: f64,
    content_y: f64,
    content_width: f64,
    content_height: f64,
}

impl WebViewPool {
    fn new() -> Self {
        WebViewPool {
            webviews: HashMap::with_capacity(MAX_TABS),
            active_tab_id: None,
            content_x: 0.0,
            content_y: 108.0,   // 初始 fallback，React 会立即覆盖
            content_width: 1200.0,
            content_height: 668.0, // 1200-132 = 1068, wait no: 800-132=668
        }
    }

    fn insert(&mut self, tab_id: i32, webview: Webview) {
        self.webviews.insert(tab_id, webview);
    }

    fn get(&self, tab_id: i32) -> Option<&Webview> {
        self.webviews.get(&tab_id)
    }

    fn remove(&mut self, tab_id: i32) -> Option<Webview> {
        if self.active_tab_id == Some(tab_id) {
            self.active_tab_id = None;
        }
        self.webviews.remove(&tab_id)
    }

    fn len(&self) -> usize {
        self.webviews.len()
    }
}

#[tauri::command]
fn create_tab(tab_id: i32, url: String, x: f64, y: f64, width: f64, height: f64, app: AppHandle) -> Result<(), String> {
    debug_log!("[create_tab] tab_id={} url={} pos={}x{} size={}x{}", tab_id, url, x, y, width, height);

    let window = match app.get_window("main") {
        Some(w) => w,
        None => {
            debug_log!("[create_tab] main window not found");
            return Err("main window not found".into());
        }
    };

    let nav_handle = app.clone();
    let load_handle = app.clone();

    let parsed_url = url.parse::<url::Url>().map_err(|e| format!("URL 解析失败: {}", e))?;

    // 注入内容区域高度（右键菜单定位用）
    let set_content_height_script = format!(
        "window.__mb_content_height = {};",
        height
    );

    let builder = WebviewBuilder::new(
        format!("content-{}", tab_id),
        WebviewUrl::External(parsed_url),
    )
    .incognito(true)
    .initialization_script(&(CTX_MENU_SCRIPT.to_owned() + &set_content_height_script + DBL_CLICK_SCRIPT))
    .on_navigation(move |url| {
        debug_log!("[on_navigation] tab_id={} url={} ALLOW={}", tab_id, url, true);
        let _ = nav_handle.emit(
            "browser://url-changed",
            UrlPayload {
                tab_id,
                url: url.to_string(),
            },
        );
        true
    })
    .on_page_load(move |_wv, payload| {
        let loading = matches!(payload.event(), PageLoadEvent::Started);
        let url_str = payload.url().to_string();
        debug_log!("[on_page_load] tab_id={} event={:?} loading={} url={}", tab_id, payload.event(), loading, url_str);
        let _ = load_handle.emit(
            "browser://loading",
            LoadingPayload { tab_id, loading },
        );
        if !loading {
            debug_log!("[on_page_load] FINISHED, emitting url-changed");
            let _ = load_handle.emit(
                "browser://url-changed",
                UrlPayload { tab_id, url: url_str },
            );
        }
    });

    let webview = window
        .add_child(
            builder,
            LogicalPosition::new(x, y),
            LogicalSize::new(width, height),
        )
        .map_err(|e| format!("创建 WebView 失败: {}", e))?;

    // 存入全局状态（持有锁时二次检查，避免 TOCTOU）
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    {
        let mut pool_guard = pool.lock().unwrap();
        if pool_guard.len() >= MAX_TABS {
            let _ = webview.close();
            return Err(format!("达到最大标签数量限制 ({})", MAX_TABS));
        }
        pool_guard.insert(tab_id, webview);
    }
    Ok(())
}

#[tauri::command]
fn activate_tab(active_tab_id: i32, app: AppHandle) {
    debug_log!("[activate_tab] active_tab_id={}", active_tab_id);
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let mut pool_guard = pool.lock().unwrap();

    let prev_active = pool_guard.active_tab_id.replace(active_tab_id);

    // 只隐藏前一个活跃 tab
    if let Some(prev_id) = prev_active {
        if prev_id != active_tab_id {
            if let Some(wv) = pool_guard.webviews.get(&prev_id) {
                let _ = wv.set_position(LogicalPosition::new(-99999.0, -99999.0));
                let _ = wv.eval(SUSPEND_SCRIPT);
            }
        }
    }

    // 使用 pool 中存储的内容区域位置显示新 tab
    let cx = pool_guard.content_x;
    let cy = pool_guard.content_y;
    if let Some(wv) = pool_guard.webviews.get(&active_tab_id) {
        let _ = wv.set_position(LogicalPosition::new(cx, cy));
        let _ = wv.eval("document.documentElement.style.visibility='visible'; document.documentElement.style.opacity='1'; document.documentElement.style.pointerEvents='auto';");
        let _ = wv.eval(RESUME_SCRIPT);
    }
}

#[tauri::command]
fn close_tab(tab_id: i32, app: AppHandle) {
    debug_log!("[close_tab] tab_id={}", tab_id);
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let mut pool_guard = pool.lock().unwrap();
    if let Some(webview) = pool_guard.remove(tab_id) {
        let _ = webview.close();
    }
}

#[tauri::command]
fn navigate_to_url(tab_id: i32, url: String, app: AppHandle) {
    debug_log!("[navigate_to_url] tab_id={} url={}", tab_id, url);
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let pool_guard = pool.lock().unwrap();
    if let Some(webview) = pool_guard.get(tab_id) {
        debug_log!("[navigate_to_url] found webview, navigating to: {}", url);
        if let Ok(parsed_url) = url::Url::parse(&url) {
            let result = webview.navigate(parsed_url);
            debug_log!("[navigate_to_url] navigate result: {:?}", result);
        } else {
            debug_log!("[navigate_to_url] invalid URL: {}", url);
        }
    } else {
        debug_log!("[navigate_to_url] webview not found for tab_id={}", tab_id);
    }
}

#[tauri::command]
fn reload_tab(tab_id: i32, app: AppHandle) {
    debug_log!("[reload_tab] tab_id={}", tab_id);
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let pool_guard = pool.lock().unwrap();
    if let Some(webview) = pool_guard.get(tab_id) {
        let _ = webview.eval("window.location.reload();");
    }
}

#[tauri::command]
fn go_back_tab(tab_id: i32, app: AppHandle) {
    debug_log!("[go_back_tab] tab_id={}", tab_id);
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let pool_guard = pool.lock().unwrap();
    if let Some(webview) = pool_guard.get(tab_id) {
        let _ = webview.eval("window.history.back();");
    }
}

#[tauri::command]
fn go_forward_tab(tab_id: i32, app: AppHandle) {
    debug_log!("[go_forward_tab] tab_id={}", tab_id);
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let pool_guard = pool.lock().unwrap();
    if let Some(webview) = pool_guard.get(tab_id) {
        let _ = webview.eval("window.history.forward();");
    }
}

#[tauri::command]
fn open_devtools_tab(tab_id: i32, app: AppHandle) {
    debug_log!("[open_devtools_tab] tab_id={}", tab_id);
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let pool_guard = pool.lock().unwrap();
    if let Some(webview) = pool_guard.get(tab_id) {
        webview.open_devtools();
    }
}

/// 前端 ResizeObserver 检测到内容区域尺寸变化时调用
/// 同步更新所有 WebView 的位置和尺寸
#[tauri::command]
fn resize_content_area(x: f64, y: f64, width: f64, height: f64, app: AppHandle) {
    debug_log!("[resize_content_area] pos={}x{} size={}x{}", x, y, width, height);
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let mut pool_guard = pool.lock().unwrap();

    // 更新存储的内容区域位置
    pool_guard.content_x = x;
    pool_guard.content_y = y;
    pool_guard.content_width = width;
    pool_guard.content_height = height;

    let active_id = pool_guard.active_tab_id;
    for (&tab_id, webview) in &pool_guard.webviews {
        let pos = if active_id == Some(tab_id) {
            LogicalPosition::new(x, y)
        } else {
            LogicalPosition::new(-99999.0, -99999.0)
        };
        let _ = webview.set_position(pos);
        let _ = webview.set_size(LogicalSize::new(width, height));
        // 同步更新右键菜单定位用的 __mb_content_height
        let _ = webview.eval(&format!("window.__mb_content_height = {};", height));
    }
}

fn main() {
    tauri::Builder::default()
        .manage(Arc::new(Mutex::new(WebViewPool::new())))
        .invoke_handler(tauri::generate_handler![
            create_tab,
            activate_tab,
            close_tab,
            navigate_to_url,
            reload_tab,
            go_back_tab,
            go_forward_tab,
            open_devtools_tab,
            resize_content_area,
        ])
        .setup(|app| {
            debug_log!("[setup] initializing mini browser with multi-webview");

            // macOS: 使用 Overlay 标题栏风格（无边框透明标题栏）
            #[cfg(target_os = "macos")]
            {
                if let Some(window) = app.get_window("main") {
                    let _ = window.set_decorations(false);
                    let _ = window.set_title_bar_style(TitleBarStyle::Overlay);
                    debug_log!("[setup] macOS: decorations removed, titlebar overlay");
                }
            }

            // 创建初始 tab（使用 fallback 尺寸，React 会立即通过 resize_content_area 纠正）
            create_tab(1, "about:blank".into(), 0.0, 108.0, 1200.0, 668.0, app.app_handle().clone())
                .unwrap_or_else(|e| panic!("创建初始 tab 失败: {}", e));

            // 激活初始 tab
            activate_tab(1, app.app_handle().clone());

            // 窗口 resize 时同步所有 WebView 尺寸（React 的 ResizeObserver 已经处理此逻辑）
            // 保留监听器仅用于调试信息
            let window = app.get_window("main").expect("main window not found");
            window.on_window_event(move |event| {
                if let WindowEvent::Resized(physical) = event {
                    debug_log!("[on_window_event::Resized] physical={}x{}", physical.width, physical.height);
                    // 注意：React 端的 ResizeObserver 会检测到 flex 布局变化并调用 resize_content_area
                    // 因此 Rust 端不需要再做任何计算
                }
            });

            debug_log!("[setup] done");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error");
}

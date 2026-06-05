#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::{
    webview::PageLoadEvent, AppHandle, Emitter, Manager, LogicalPosition, LogicalSize,
    PhysicalSize, WebviewBuilder, WebviewUrl, Webview, WindowEvent,
};

/// 调试日志：仅在 debug 构建中输出
macro_rules! debug_log {
    ($($arg:tt)*) => {
        if cfg!(debug_assertions) {
            std::eprintln!($($arg)*);
        }
    };
}

/// 中文右键菜单脚本（注入到每个 content webview）
/// 使用 __mb_content_height 代替 window.innerHeight，避免跨平台差异
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

    // 保存原始 API
    window.__mb_orig_setTimeout = window.setTimeout;
    window.__mb_orig_setInterval = window.setInterval;
    window.__mb_orig_requestAnimationFrame = window.requestAnimationFrame;

    // 通过覆盖来阻止新定时器
    window.setTimeout = function() { return -1; };
    window.setInterval = function() { return -1; };
    window.requestAnimationFrame = function() { return -1; };

    // 暂停媒体
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

    // 恢复原始 API
    window.setTimeout = window.__mb_orig_setTimeout;
    window.setInterval = window.__mb_orig_setInterval;
    window.requestAnimationFrame = window.__mb_orig_requestAnimationFrame;

    // 恢复媒体
    document.querySelectorAll('video, audio').forEach(function(el) {
        if (el.__mb_was_playing) {
            el.play().catch(function(){});
            el.__mb_was_playing = false;
        }
    });

    console.log('[mini-browser] tab resumed');
})();"#;

/// Total height of the fixed UI chrome
const UI_CHROME_HEIGHT: f64 = 132.0;

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
}

impl WebViewPool {
    fn new() -> Self {
        WebViewPool {
            webviews: HashMap::with_capacity(MAX_TABS),
            active_tab_id: None,
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

/// 计算内容区域尺寸（逻辑像素）
fn calc_content_size(physical_width: f64, physical_height: f64, scale: f64) -> (f64, f64) {
    let logical_width = physical_width / scale;
    let logical_height = physical_height / scale;
    let content_height = (logical_height - UI_CHROME_HEIGHT).max(100.0);
    (logical_width, content_height)
}

/// 获取内容区域尺寸（从 Window 对象）
fn get_content_size(window: &tauri::Window) -> (f64, f64) {
    let scale = window.scale_factor().unwrap_or(1.0);
    let physical = window.outer_size().unwrap_or(PhysicalSize::new(1200, 800));
    let size = calc_content_size(physical.width as f64, physical.height as f64, scale);
    debug_log!("[get_content_size] physical={}x{} scale={} logical={}x{}",
        physical.width, physical.height, scale, size.0, size.1);
    size
}

#[tauri::command]
fn create_tab(tab_id: i32, url: String, app: AppHandle) -> Result<(), String> {
    debug_log!("[create_tab] tab_id={} url={}", tab_id, url);

    let window = match app.get_window("main") {
        Some(w) => w,
        None => {
            debug_log!("[create_tab] main window not found");
            return Err("main window not found".into());
        }
    };

    let nav_handle = app.clone();
    let load_handle = app.clone();
    let (content_width, content_height) = get_content_size(&window);

    let parsed_url = url.parse::<url::Url>().map_err(|e| format!("URL 解析失败: {}", e))?;

    // 注入内容区域高度（跨平台一致），供右键菜单等脚本使用
    let set_content_height_script = format!(
        "window.__mb_content_height = {};",
        content_height
    );

    let builder = WebviewBuilder::new(
        format!("content-{}", tab_id),
        WebviewUrl::External(parsed_url),
    )
    .incognito(true)
    .initialization_script(&(CTX_MENU_SCRIPT.to_owned() + &set_content_height_script + DBL_CLICK_SCRIPT))
    .on_navigation(move |url| {
        debug_log!("[on_navigation] tab_id={} url={}", tab_id, url);
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
        debug_log!("[on_page_load] tab_id={} loading={} url={}", tab_id, loading, url_str);
        let _ = load_handle.emit(
            "browser://loading",
            LoadingPayload { tab_id, loading },
        );
        if !loading {
            let _ = load_handle.emit(
                "browser://url-changed",
                UrlPayload { tab_id, url: url_str },
            );
        }
    });

    let webview = window
        .add_child(
            builder,
            LogicalPosition::new(0.0, UI_CHROME_HEIGHT),
            LogicalSize::new(content_width, content_height),
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

    // 显示新活跃 tab
    if let Some(wv) = pool_guard.webviews.get(&active_tab_id) {
        let _ = wv.set_position(LogicalPosition::new(0.0, UI_CHROME_HEIGHT));
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
        // 关闭 webview
        let _ = webview.close();
    }
}

#[tauri::command]
fn navigate_to_url(tab_id: i32, url: String, app: AppHandle) {
    debug_log!("[navigate_to_url] tab_id={} url={}", tab_id, url);
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let pool_guard = pool.lock().unwrap();
    if let Some(webview) = pool_guard.get(tab_id) {
        // 使用原生 navigate API（比 eval 更可靠）
        if let Ok(parsed_url) = url::Url::parse(&url) {
            let _ = webview.navigate(parsed_url);
        } else {
            debug_log!("[navigate_to_url] invalid URL: {}", url);
        }
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
        ])
        .setup(|app| {
            debug_log!("[setup] initializing mini browser with multi-webview");

            // 复用 create_tab 创建初始 tab
            create_tab(1, "about:blank".into(), app.app_handle().clone())
                .unwrap_or_else(|e| panic!("创建初始 tab 失败: {}", e));

            // 激活初始 tab（确保可见性状态正确）
            activate_tab(1, app.app_handle().clone());

            // 窗口 resize 时同步所有 WebView
            let window = app.get_window("main").expect("main window not found");
            let resize_handle = app.app_handle().clone();
            window.on_window_event(move |event| {
                if let WindowEvent::Resized(physical) = event {
                    if let Some(win) = resize_handle.get_window("main") {
                        let pool = resize_handle.state::<Arc<Mutex<WebViewPool>>>();
                        let pool_guard = pool.lock().unwrap();
                        let scale = win.scale_factor().unwrap_or(1.0);
                        let (content_width, content_height) =
                            calc_content_size(physical.width as f64, physical.height as f64, scale);
                        let active_id = pool_guard.active_tab_id;
                        for (&tab_id, webview) in &pool_guard.webviews {
                            let pos = if active_id == Some(tab_id) {
                                LogicalPosition::new(0.0, UI_CHROME_HEIGHT)
                            } else {
                                LogicalPosition::new(-99999.0, -99999.0)
                            };
                            let _ = webview.set_position(pos);
                            let _ = webview.set_size(LogicalSize::new(content_width, content_height));
                            // 同步更新 __mb_content_height（右键菜单定位用）
                            let _ = webview.eval(&format!("window.__mb_content_height = {};", content_height));
                        }
                    }
                }
            });

            debug_log!("[setup] done");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error");
}

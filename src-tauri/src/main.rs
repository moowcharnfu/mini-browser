#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tauri::{
    webview::{PageLoadEvent, WebviewBuilder},
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, WebviewUrl, WindowEvent,
};

/// 生成当前平台的 User-Agent 字符串
fn platform_user_agent() -> String {
    let platform = if cfg!(target_os = "macos") {
        "Macintosh; Intel Mac OS X 10_15_7"
    } else if cfg!(target_os = "windows") {
        "Windows NT 10.0; Win64; x64"
    } else {
        "X11; Linux x86_64"
    };
    format!(
        "Mozilla/5.0 ({}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        platform
    )
}

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

/// LRU 阈值：后台超过此数量的 tab 将触发 WebView 销毁
const LRU_THRESHOLD: usize = 3;

/// 激活 tab 时返回的状态，指示前端是否需要重建 WebView
#[derive(Clone, serde::Serialize)]
struct ActivateResult {
    tab_id: i32,
    needs_recreate: bool,
    exists: bool,
}

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

/// 存储所有嵌入 webview 的句柄
struct WebViewPool {
    webviews: HashMap<i32, tauri::Webview>,
    active_tab_id: Option<i32>,
    /// LRU 顺序：最近使用的在末尾，最久未使用的在开头
    lru_order: VecDeque<i32>,
    /// 内容区域在窗口中的位置（viewport 相对坐标，CSS 逻辑像素）
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
            lru_order: VecDeque::with_capacity(MAX_TABS),
            content_x: 0.0,
            content_y: 0.0,
            content_width: 1200.0,
            content_height: 800.0,
        }
    }

    fn insert(&mut self, tab_id: i32, webview: tauri::Webview) {
        self.webviews.insert(tab_id, webview);
        self.lru_order.push_back(tab_id);
    }

    fn remove(&mut self, tab_id: i32) -> Option<tauri::Webview> {
        if self.active_tab_id == Some(tab_id) {
            self.active_tab_id = None;
        }
        self.lru_order.retain(|&id| id != tab_id);
        self.webviews.remove(&tab_id)
    }

    fn len(&self) -> usize {
        self.webviews.len()
    }

    /// LRU 淘汰：当 webview 数超过阈值时，销毁最久未使用的
    fn evict_lru(&mut self, _app: &AppHandle) {
        while self.webviews.len() > LRU_THRESHOLD {
            let evict_id = self.lru_order.iter().find(|&&id| {
                self.active_tab_id != Some(id) && self.webviews.contains_key(&id)
            }).copied();

            match evict_id {
                Some(id) => {
                    debug_log!("[evict_lru] evicting tab_id={}", id);
                    if let Some(webview) = self.webviews.remove(&id) {
                        self.lru_order.retain(|&lid| lid != id);
                        let _ = webview.close();
                    }
                }
                None => break,
            }
        }
    }

    /// 记录 tab 被激活（移至 LRU 末尾）
    fn touch_lru(&mut self, tab_id: i32) {
        self.lru_order.retain(|&id| id != tab_id);
        self.lru_order.push_back(tab_id);
    }

    /// 检查 tab 的嵌入 webview 是否存在
    fn has_webview(&self, tab_id: i32) -> bool {
        self.webviews.contains_key(&tab_id)
    }
}

#[tauri::command]
fn create_tab(tab_id: i32, url: String, x: f64, y: f64, width: f64, height: f64, app: AppHandle) -> Result<(), String> {
    debug_log!("[create_tab] tab_id={} url={} pos={}x{} size={}x{}", tab_id, url, x, y, width, height);

    let label = format!("content-{}", tab_id);
    let parsed_url = url.parse::<url::Url>().map_err(|e| format!("URL 解析失败: {}", e))?;

    let height_script = format!("window.__mb_content_height = {};", height);
    let init_script = CTX_MENU_SCRIPT.to_owned() + &height_script + DBL_CLICK_SCRIPT;

    let nav_handle = app.clone();
    let load_handle = app.clone();
    let nav_tab_id = tab_id;
    let load_tab_id = tab_id;
    let nav_app = app.clone();

    let webview_builder = WebviewBuilder::new(&label, WebviewUrl::External(parsed_url))
        .user_agent(&platform_user_agent())
        .initialization_script(&init_script)
        .incognito(true)
        .on_navigation(move |url| {
            debug_log!("[on_navigation] tab_id={} url={} ALLOW={}", nav_tab_id, url, true);
            let _ = nav_app.emit(
                "browser://url-changed",
                UrlPayload {
                    tab_id: nav_tab_id,
                    url: url.to_string(),
                },
            );
            true
        })
        .on_page_load(move |_webview, payload| {
            let loading = matches!(payload.event(), PageLoadEvent::Started);
            let url_str = payload.url().to_string();
            debug_log!("[on_page_load] tab_id={} event={:?} loading={} url={}", load_tab_id, payload.event(), loading, url_str);
            let _ = load_handle.emit(
                "browser://loading",
                LoadingPayload { tab_id: load_tab_id, loading },
            );
            if !loading {
                debug_log!("[on_page_load] FINISHED, emitting url-changed");
                let _ = nav_handle.emit(
                    "browser://url-changed",
                    UrlPayload { tab_id: load_tab_id, url: url_str },
                );
            }
        });

    // 获取主窗口（Window 句柄，add_child 方法所在类型）
    let main_window = app.get_window("main").ok_or("主窗口未找到")?;
    let webview = main_window
        .add_child(
            webview_builder,
            LogicalPosition::new(x, y),
            LogicalSize::new(width, height),
        )
        .map_err(|e| format!("嵌入 webview 失败: {}", e))?;

    // 存入全局状态
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    {
        let mut pool_guard = pool.lock().unwrap();
        if pool_guard.len() >= MAX_TABS {
            let _ = webview.close();
            return Err(format!("达到最大标签数量限制 ({})", MAX_TABS));
        }
        pool_guard.insert(tab_id, webview);
        pool_guard.evict_lru(&app);
    }
    Ok(())
}

#[tauri::command]
fn activate_tab(active_tab_id: i32, app: AppHandle) -> ActivateResult {
    debug_log!("[activate_tab] active_tab_id={}", active_tab_id);
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let mut pool_guard = pool.lock().unwrap();

    let prev_active = pool_guard.active_tab_id.replace(active_tab_id);

    // 隐藏前一个活跃 tab 的嵌入 webview
    if let Some(prev_id) = prev_active {
        if prev_id != active_tab_id {
            if let Some(webview) = pool_guard.webviews.get(&prev_id) {
                let _ = webview.hide();
            }
        }
    }

    // 检查目标 tab 的 webview 是否还存在
    if !pool_guard.has_webview(active_tab_id) {
        debug_log!("[activate_tab] tab_id={} webview not found, needs recreate", active_tab_id);
        return ActivateResult {
            tab_id: active_tab_id,
            needs_recreate: true,
            exists: false,
        };
    }

    // 更新 LRU 顺序
    pool_guard.touch_lru(active_tab_id);

    // 直接使用 viewport 坐标设置嵌入 webview 的位置和尺寸
    if let Some(webview) = pool_guard.webviews.get(&active_tab_id) {
        let _ = webview.set_position(LogicalPosition::new(pool_guard.content_x, pool_guard.content_y));
        let _ = webview.set_size(LogicalSize::new(pool_guard.content_width, pool_guard.content_height));
        let _ = webview.show();
        let _ = webview.set_focus();
    }

    ActivateResult {
        tab_id: active_tab_id,
        needs_recreate: false,
        exists: true,
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
    if let Some(webview) = pool_guard.webviews.get(&tab_id) {
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
    if let Some(webview) = pool_guard.webviews.get(&tab_id) {
        let _ = webview.eval("window.location.reload();");
    }
}

#[tauri::command]
fn go_back_tab(tab_id: i32, app: AppHandle) {
    debug_log!("[go_back_tab] tab_id={}", tab_id);
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let pool_guard = pool.lock().unwrap();
    if let Some(webview) = pool_guard.webviews.get(&tab_id) {
        let _ = webview.eval("window.history.back();");
    }
}

#[tauri::command]
fn go_forward_tab(tab_id: i32, app: AppHandle) {
    debug_log!("[go_forward_tab] tab_id={}", tab_id);
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let pool_guard = pool.lock().unwrap();
    if let Some(webview) = pool_guard.webviews.get(&tab_id) {
        let _ = webview.eval("window.history.forward();");
    }
}

#[tauri::command]
fn open_devtools_tab(tab_id: i32, app: AppHandle) {
    debug_log!("[open_devtools_tab] tab_id={}", tab_id);
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let pool_guard = pool.lock().unwrap();
    if let Some(webview) = pool_guard.webviews.get(&tab_id) {
        webview.open_devtools();
    }
}

/// 前端 ResizeObserver 检测到内容区域尺寸变化时调用
/// 直接使用 viewport 坐标更新嵌入 webview 的位置和尺寸
#[tauri::command]
fn resize_content_area(x: f64, y: f64, width: f64, height: f64, app: AppHandle) {
    debug_log!("[resize_content_area] pos={}x{} size={}x{}", x, y, width, height);
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let mut pool_guard = pool.lock().unwrap();

    pool_guard.content_x = x;
    pool_guard.content_y = y;
    pool_guard.content_width = width;
    pool_guard.content_height = height;

    let active_id = pool_guard.active_tab_id;
    for (&tab_id, webview) in &pool_guard.webviews {
        if active_id == Some(tab_id) {
            let _ = webview.set_position(LogicalPosition::new(x, y));
            let _ = webview.set_size(LogicalSize::new(width, height));
            let _ = webview.set_focus();
        } else {
            let _ = webview.hide();
        }
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
            debug_log!("[setup] initializing mini browser with native decorations");

            let window = app.get_webview_window("main").expect("main window not found");
            let close_app = app.handle().clone();
            window.on_window_event(move |event| {
                if let WindowEvent::CloseRequested { .. } = event {
                    debug_log!("[on_window_event::CloseRequested] cleaning up child webviews");
                    let pool = close_app.state::<Arc<Mutex<WebViewPool>>>();
                    let pool_guard = pool.lock().unwrap();
                    for (_, w) in &pool_guard.webviews {
                        let _ = w.close();
                    }
                }
            });

            debug_log!("[setup] done");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error");
}
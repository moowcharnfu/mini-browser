#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

static NEXT_TAB_ID: AtomicI32 = AtomicI32::new(1000);
use tauri::{
    webview::{NewWindowResponse, PageLoadEvent, WebviewBuilder},
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, WebviewUrl, WindowEvent,
};

/// 自定义 User-Agent：在 AppleWebKit 后追加 Safari/Chrome/Firefox 标识以通过浏览器检测
fn user_agent() -> &'static str {
    #[cfg(target_os = "macos")]
    { "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Safari/603.1 Chrome/79.0.0.0 Firefox/72.0 moowcharnfu" }
    #[cfg(target_os = "windows")]
    { "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/605.1.15 (KHTML, like Gecko) Safari/603.1 Chrome/79.0.0.0 Firefox/72.0 moowcharnfu" }
    #[cfg(target_os = "linux")]
    { "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Safari/603.1 Chrome/79.0.0.0 Firefox/72.0 moowcharnfu" }
}

/// 生成当前平台的 User-Agent 字符串
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

/// Popup 处理脚本：主 frame 使用原生 window.open 触发 on_new_window Rust 回调
/// 不依赖 __TAURI_INTERNALS__.invoke（避免页面 CSP 拦截 ipc://）
/// 同时接收 iframe 通过 postMessage 中继的 popup 请求
const POPUP_SCRIPT: &str = r#"(function(){
    if(window.__mb_popup)return;window.__mb_popup=true;
    var ow=window.open;
    window.open=function(u,t,f){
        if(u&&typeof u==='string'){
            var r=ow.call(window,u,t||'_blank',f);
            return r||{closed:false,close:function(){}};
        }
        return ow.apply(this,arguments);
    };
    window.addEventListener('message',function(e){
        var d=e.data||{};
        if(d.type==='__mb_popup'&&d.url){
            var w=window.open;
            if(w)w(d.url,'_blank');
        }
    });
})();"#;

/// 注入所有 frame 的轻量脚本：iframe 内的 target=_blank / window.open → postMessage 到父 frame
/// 主 frame 中跳过（主 frame 由 POPUP_SCRIPT 处理，避免 capture 阶段 stopPropagation 干扰页面元素事件）
const POPUP_IFRAME_SCRIPT: &str = r#"(function(){
    if(window.__mb_ifr)return;window.__mb_ifr=true;
    try{if(window.self===window.top)return}catch(e){}
    document.addEventListener('click',function(e){
        var a=e.target.closest('a');if(a&&a.target==='_blank'&&a.href&&!a.href.startsWith('about:')){e.preventDefault();e.stopPropagation();try{window.parent.postMessage({type:'__mb_popup',url:a.href},'*')}catch(e){}}
    },true);
    var ow=window.open;
    window.open=function(u){if(u&&typeof u==='string'){try{window.parent.postMessage({type:'__mb_popup',url:u},'*')}catch(e){}return{closed:false,close:function(){}}};return ow.apply(this,arguments)};
})();"#;

/// Linux 工具栏脚本：在主 webview 上覆盖浏览器控件
/// 通过 on_page_load 注入，每次页面加载都会执行
/// 注意：此脚本在注入时由插件回调包裹 inline 数据（__MB_INLINE_TABS, __MB_INLINE_ACTIVE）
/// 不使用 IPC 调用（外部页面 IPC 不可用），改用 URL 导航模式
const TOOLBAR_SCRIPT: &str = r#";(function(){
    try{
    if(document.getElementById('__mb_tb'))return;
    var L=window.location.href;
    if(L&&(L.startsWith('http://localhost')||L.startsWith('tauri://')||L.startsWith('https://tauri')))return;
    // Intercept target=_blank links
    document.addEventListener('click',function(e){var a=e.target.closest('a');if(a&&a.target==='_blank'){e.preventDefault();window.location.href=a.href}},true);
    // Tab state from inline data (set by Rust plugin callback)
    var tabs=window.__MB_INLINE_TABS||[];
    var activeId=window.__MB_INLINE_ACTIVE||0;
    window.__mb_active_tab_id=activeId;
    function nav(u){if(!u)return;if(!u.match(/^https?:\/\//)){u=u.includes('.')?'https://'+u:'https://www.google.com/search?q='+encodeURIComponent(u)}window.location.href=u}
    // Cleanup inline data
    try{delete window.__MB_INLINE_TABS;delete window.__MB_INLINE_ACTIVE}catch(e){}
    buildUI(tabs,activeId);
    function buildUI(tabs,activeId){
    var d=document.createElement('div');d.id='__mb_tb';d.style.cssText='position:fixed;top:0;left:0;right:0;z-index:2147483647;background:#1a1a2e;border-bottom:1px solid #2d2d4a;font:13px system-ui,sans-serif;color:#e8e8f0;box-shadow:0 2px 8px rgba(0,0,0,0.4);';
    // Tab bar
    var tb=document.createElement('div');tb.style.cssText='display:flex;align-items:center;gap:2px;padding:2px 8px;border-bottom:1px solid #2d2d4a;';
    tabs.forEach(function(t){
        var t2=document.createElement('div');t2.style.cssText='display:flex;align-items:center;gap:4px;padding:4px 6px;border-radius:6px;cursor:pointer;min-width:60px;max-width:140px;height:26px;font-size:12px;flex-shrink:0;color:#e8e8f0;background:'+(t.id===activeId?'#2d2d52':'transparent')+';';
        t2.onmouseenter=function(){this.style.background='#2d2d52'};
        t2.onmouseleave=function(){this.style.background=t.id===activeId?'#2d2d52':'transparent'};
        t2.onclick=function(){if(t.id!==activeId){window.location.href='about:blank?__mb_switch='+t.id}};
        var sp=document.createElement('span');sp.style.cssText='overflow:hidden;text-overflow:ellipsis;white-space:nowrap;flex:1;';sp.textContent=t.url&&t.url!=='about:blank'?t.url:'新标签页';
        t2.appendChild(sp);
        var cx=document.createElement('span');cx.textContent='×';cx.style.cssText='width:16px;height:16px;border-radius:4px;display:flex;align-items:center;justify-content:center;font-size:12px;color:#606080;cursor:pointer;flex-shrink:0;';
        cx.onmouseenter=function(){this.style.background='#3d3d62'};cx.onmouseleave=function(){this.style.background='transparent'};
        cx.onclick=function(e){e.stopPropagation();window.location.href='about:blank?__mb_close='+t.id};
        t2.appendChild(cx);tb.appendChild(t2);
    });
    var nt=document.createElement('button');nt.textContent='+';nt.style.cssText='width:24px;height:24px;border:none;background:transparent;color:#a0a0c0;border-radius:6px;cursor:pointer;font-size:16px;display:flex;align-items:center;justify-content:center;flex-shrink:0;margin-left:2px;';
    nt.onmouseenter=function(){this.style.background='#2d2d52'};nt.onmouseleave=function(){this.style.background='transparent'};
    nt.onclick=function(){window.location.href='about:blank?__mb_new='+Date.now()};
    tb.appendChild(nt);d.appendChild(tb);
    // Navigation bar
    var nb=document.createElement('div');nb.style.cssText='display:flex;align-items:center;gap:4px;padding:6px 10px;';
    function bt(t,f){var b=document.createElement('button');b.textContent=t;b.style.cssText='width:28px;height:28px;border:none;background:transparent;color:#a0a0c0;border-radius:6px;cursor:pointer;font-size:14px;display:flex;align-items:center;justify-content:center;flex-shrink:0;';b.onmouseenter=function(){this.style.background='#2d2d52'};b.onmouseleave=function(){this.style.background='transparent'};b.onclick=f;return b;}
    nb.appendChild(bt('←',function(){window.history.back()}));
    nb.appendChild(bt('→',function(){window.history.forward()}));
    nb.appendChild(bt('↻',function(){window.location.reload()}));
    var u=document.createElement('input');u.style.cssText='flex:1;height:32px;border:1.5px solid #2d2d4a;border-radius:8px;background:#0f0f1a;color:#e8e8f0;padding:0 10px;font-size:13px;outline:none;min-width:0;';u.value=L.startsWith('about:blank')?'':L;
    u.placeholder='输入网址或搜索...';
    u.onfocus=function(){this.select()};
    u.onkeydown=function(e){if(e.key==='Enter'){var v=u.value.trim();if(v)nav(v)}};
    nb.appendChild(u);
    var gb=document.createElement('button');gb.textContent='→';gb.style.cssText='width:28px;height:28px;border:none;background:#6366f1;color:#fff;border-radius:6px;cursor:pointer;font-size:14px;display:flex;align-items:center;justify-content:center;flex-shrink:0;';
    gb.onclick=function(){var v=u.value.trim();if(v)nav(v)};
    nb.appendChild(gb);
    nb.appendChild(bt('🔧',function(){window.location.href='about:blank?__mb_devtools='+activeId;}));
    d.appendChild(nb);
    // Welcome page for about:blank (including about:blank?__mb_new=... and __mb_close=...)
    if(L.startsWith('about:blank')){
        var w=document.createElement('div');w.id='__mb_welcome';w.style.cssText='display:flex;align-items:center;justify-content:center;height:calc(100vh - 80px);background:#0f0f1a;';
        var wc=document.createElement('div');wc.style.cssText='text-align:center;color:#606080;';
        wc.innerHTML='<div style="font-size:48px;opacity:0.4;margin-bottom:16px;">🌐</div><h2 style="font-size:18px;font-weight:600;margin-bottom:8px;color:#e8e8f0;">迷你浏览器</h2><p>输入网址开始浏览</p>';
        w.appendChild(wc);d.appendChild(w);
    }
    document.body.prepend(d);
    var p=function(){var e=document.getElementById('__mb_tb');if(e){var h=e.offsetHeight;document.body.style.paddingTop=Math.max(parseInt(document.body.style.paddingTop)||0,h)+'px'}};
    setTimeout(p,50);
    setInterval(function(){var a=u.value,b=window.location.href;if(a!==b&&b&&!b.startsWith('about:blank'))u.value=b},500);
    } // buildUI end
    }catch(e){}
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

#[derive(Clone, serde::Serialize)]
struct TabInfo {
    id: i32,
    url: String,
}

#[derive(Clone, serde::Serialize)]
struct ToolbarState {
    tabs: Vec<TabInfo>,
    active_tab_id: i32,
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
    /// 标签页 URL 记录（tab_id -> url），Linux 上用于工具栏脚本
    tab_urls: HashMap<i32, String>,
    /// 已打开开发者工具的标签页集合（切换 tab 时跟随隐藏/显示）
    devtools_open: HashSet<i32>,
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
            tab_urls: HashMap::with_capacity(MAX_TABS),
            devtools_open: HashSet::new(),
        }
    }

    fn insert(&mut self, tab_id: i32, url: &str, webview: tauri::Webview) {
        self.webviews.insert(tab_id, webview);
        self.tab_urls.insert(tab_id, url.to_string());
        self.lru_order.push_back(tab_id);
    }

    fn insert_tab_only(&mut self, tab_id: i32, url: &str) {
        // Linux: store tab info without a webview
        self.tab_urls.insert(tab_id, url.to_string());
        self.lru_order.push_back(tab_id);
    }

    fn remove(&mut self, tab_id: i32) -> Option<tauri::Webview> {
        if self.active_tab_id == Some(tab_id) {
            self.active_tab_id = None;
        }
        self.lru_order.retain(|&id| id != tab_id);
        self.tab_urls.remove(&tab_id);
        self.devtools_open.remove(&tab_id);
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
    eprintln!("[diag] create_tab ENTER: tab_id={} url={} pos={}x{} size={}x{}", tab_id, url, x, y, width, height);

    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let mut pool_guard = pool.lock().unwrap();
    if pool_guard.len() >= MAX_TABS {
        return Err(format!("达到最大标签数量限制 ({})", MAX_TABS));
    }

    if cfg!(target_os = "linux") {
        // Linux: 不创建子 webview（避免 GTK 分离问题），只存储标签信息
        pool_guard.insert_tab_only(tab_id, &url);
        pool_guard.content_x = x;
        pool_guard.content_y = y;
        pool_guard.content_width = width;
        pool_guard.content_height = height;
        eprintln!("[diag] create_tab DONE (Linux, no add_child): pool has {} tabs, content={}x{} size={}x{}",
            pool_guard.lru_order.len(), x, y, width, height);
        return Ok(());
    }

    // macOS/Windows: 使用 add_child 创建嵌入 webview
    let label = format!("content-{}", tab_id);
    let parsed_url = url.parse::<url::Url>().map_err(|e| format!("URL 解析失败: {}", e))?;

    let height_script = format!("window.__mb_content_height = {};", height);
    let init_script = CTX_MENU_SCRIPT.to_owned() + POPUP_SCRIPT + &height_script;

    let nav_handle = app.clone();
    let load_handle = app.clone();
    let nav_tab_id = tab_id;
    let load_tab_id = tab_id;
    let nav_app = app.clone();
    let popup_app = app.clone();

    let webview_builder = WebviewBuilder::new(&label, WebviewUrl::External(parsed_url))
        .user_agent(user_agent())
        .initialization_script(&init_script)
        .initialization_script_for_all_frames(POPUP_IFRAME_SCRIPT)
        .on_new_window(move |url, _features| {
            let url_str = url.to_string();
            debug_log!("[on_new_window] url={}", url_str);
            let _ = popup_app.emit("popup://request", PopupPayload { url: url_str });
            NewWindowResponse::Deny
        })
        .on_navigation(move |url| {
            let url_str = url.to_string();
            debug_log!("[on_navigation] tab_id={} url={} ALLOW={}", nav_tab_id, url_str, true);
            let _ = nav_app.emit(
                "browser://url-changed",
                UrlPayload {
                    tab_id: nav_tab_id,
                    url: url_str,
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

    let main_window = app.get_window("main").ok_or("主窗口未找到")?;
    let webview = main_window
        .add_child(
            webview_builder,
            LogicalPosition::new(x, y),
            LogicalSize::new(width, height),
        )
        .map_err(|e| format!("嵌入 webview 失败: {}", e))?;

    if pool_guard.len() >= MAX_TABS {
        let _ = webview.close();
        return Err(format!("达到最大标签数量限制 ({})", MAX_TABS));
    }
    pool_guard.insert(tab_id, &url, webview);
    pool_guard.content_x = x;
    pool_guard.content_y = y;
    pool_guard.content_width = width;
    pool_guard.content_height = height;
    pool_guard.evict_lru(&app);
    eprintln!("[diag] create_tab DONE: pool has {} webviews, content={}x{} size={}x{}",
        pool_guard.len(), x, y, width, height);
    Ok(())
}

#[tauri::command]
fn activate_tab(active_tab_id: i32, app: AppHandle) -> ActivateResult {
    eprintln!("[diag] activate_tab ENTER: active_tab_id={}", active_tab_id);
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let mut pool_guard = pool.lock().unwrap();

    let prev_active = pool_guard.active_tab_id.replace(active_tab_id);
    eprintln!("[diag] activate_tab: prev_active={:?} webviews_count={}", prev_active, pool_guard.webviews.len());

    if cfg!(target_os = "linux") {
        // Linux: 导航主 webview 到该 tab 的 URL
        let url = pool_guard.tab_urls.get(&active_tab_id).cloned().unwrap_or_default();
        drop(pool_guard);
        if !url.is_empty() && url != "about:blank" {
            eprintln!("[diag] activate_tab: Linux navigating main webview to url={}", url);
            if let Some(wv) = app.get_webview_window("main") {
                let escaped = url.replace('\'', "\\'");
                let _ = wv.eval(&format!("window.location.href = '{}';", escaped));
            }
        }
        return ActivateResult {
            tab_id: active_tab_id,
            needs_recreate: false,
            exists: true,
        };
    }

    // macOS/Windows: hide previous, show target
    if let Some(prev_id) = prev_active {
        if prev_id != active_tab_id {
            if let Some(webview) = pool_guard.webviews.get(&prev_id) {
                eprintln!("[diag] activate_tab: hiding prev tab_id={}", prev_id);
                let _ = webview.hide();
            }
            // Close devtools for previous tab and clean up state
            if pool_guard.devtools_open.contains(&prev_id) {
                if let Some(webview) = pool_guard.webviews.get(&prev_id) {
                    eprintln!("[diag] activate_tab: closing devtools for prev tab_id={}", prev_id);
                    let _ = webview.close_devtools();
                }
            }
            pool_guard.devtools_open.remove(&prev_id);
        }
    }

    if !pool_guard.has_webview(active_tab_id) {
        eprintln!("[diag] activate_tab: webview NOT FOUND for tab_id={}, needs recreate", active_tab_id);
        return ActivateResult {
            tab_id: active_tab_id,
            needs_recreate: true,
            exists: false,
        };
    }

    pool_guard.touch_lru(active_tab_id);

if let Some(webview) = pool_guard.webviews.get(&active_tab_id) {
        eprintln!("[diag] activate_tab: setting pos={}x{} size={}x{} then show/set_focus",
            pool_guard.content_x, pool_guard.content_y, pool_guard.content_width, pool_guard.content_height);
        let _ = webview.set_position(LogicalPosition::new(pool_guard.content_x, pool_guard.content_y));
        let _ = webview.set_size(LogicalSize::new(pool_guard.content_width, pool_guard.content_height));
        let _ = webview.show();
        let _ = webview.set_focus();
        eprintln!("[diag] activate_tab DONE: show+position done");
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
    // Don't close the last tab
    if pool_guard.tab_urls.len() <= 1 {
        debug_log!("[close_tab] refusing to close last tab");
        return;
    }
    if let Some(webview) = pool_guard.remove(tab_id) {
        let _ = webview.close();
    }
}

#[tauri::command]
fn navigate_to_url(tab_id: i32, url: String, app: AppHandle) {
    eprintln!("[diag] navigate_to_url ENTER: tab_id={} url={}", tab_id, url);

    // 更新 tab URL 记录
    {
        let pool = app.state::<Arc<Mutex<WebViewPool>>>();
        let mut pool_guard = pool.lock().unwrap();
        pool_guard.tab_urls.insert(tab_id, url.clone());
        // Linux: 确保 active_tab_id 同步更新，供工具栏 get_toolbar_state 使用
        if cfg!(target_os = "linux") {
            pool_guard.active_tab_id = Some(tab_id);
            // 确保 tab 在 lru_order 中，供 get_toolbar_state 正确返回标签列表
            if !pool_guard.lru_order.contains(&tab_id) {
                pool_guard.lru_order.push_back(tab_id);
            }
        }
    }

    if cfg!(target_os = "linux") {
        // Linux: 直接导航主 webview
        eprintln!("[diag] navigate_to_url: Linux navigating main webview to: {}", url);
        if let Some(wv) = app.get_webview_window("main") {
            let escaped = url.replace('\'', "\\'");
            let result = wv.eval(&format!("window.location.href = '{}';", escaped));
            eprintln!("[diag] navigate_to_url: eval result={:?}", result);
        } else {
            eprintln!("[diag] navigate_to_url: ERROR main webview not found");
        }
        return;
    }

    // macOS/Windows: 导航子 webview
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let has_wv = { pool.lock().unwrap().has_webview(tab_id) };

    if !has_wv {
        eprintln!("[diag] navigate_to_url: webview not found, recreating");
        let (cx, cy, cw, ch) = {
            let g = pool.lock().unwrap();
            (g.content_x, g.content_y, g.content_width, g.content_height)
        };
        let _ = create_tab(tab_id, url.clone(), cx, cy, cw, ch, app.clone());
    }

    let pool_guard = pool.lock().unwrap();
    if let Some(webview) = pool_guard.webviews.get(&tab_id) {
        eprintln!("[diag] navigate_to_url: found webview, navigating to: {}", url);
        if let Ok(parsed_url) = url::Url::parse(&url) {
            let result = webview.navigate(parsed_url);
            eprintln!("[diag] navigate_to_url: navigate result: {:?}", result);
        } else {
            eprintln!("[diag] navigate_to_url: invalid URL: {}", url);
        }
    } else {
        eprintln!("[diag] navigate_to_url: webview NOT FOUND after recreate attempt, tab_id={}", tab_id);
    }
}

#[tauri::command]
fn reload_tab(tab_id: i32, app: AppHandle) {
    debug_log!("[reload_tab] tab_id={}", tab_id);
    if cfg!(target_os = "linux") {
        if let Some(wv) = app.get_webview_window("main") {
            let _ = wv.eval("window.location.reload();");
        }
        return;
    }
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let pool_guard = pool.lock().unwrap();
    if let Some(webview) = pool_guard.webviews.get(&tab_id) {
        let _ = webview.eval("window.location.reload();");
    }
}

#[tauri::command]
fn go_back_tab(tab_id: i32, app: AppHandle) {
    debug_log!("[go_back_tab] tab_id={}", tab_id);
    if cfg!(target_os = "linux") {
        if let Some(wv) = app.get_webview_window("main") {
            let _ = wv.eval("window.history.back();");
        }
        return;
    }
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let pool_guard = pool.lock().unwrap();
    if let Some(webview) = pool_guard.webviews.get(&tab_id) {
        let _ = webview.eval("window.history.back();");
    }
}

#[tauri::command]
fn go_forward_tab(tab_id: i32, app: AppHandle) {
    debug_log!("[go_forward_tab] tab_id={}", tab_id);
    if cfg!(target_os = "linux") {
        if let Some(wv) = app.get_webview_window("main") {
            let _ = wv.eval("window.history.forward();");
        }
        return;
    }
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let pool_guard = pool.lock().unwrap();
    if let Some(webview) = pool_guard.webviews.get(&tab_id) {
        let _ = webview.eval("window.history.forward();");
    }
}

#[tauri::command]
fn open_devtools_tab(tab_id: i32, app: AppHandle) {
    eprintln!("[open_devtools_tab] tab_id={}", tab_id);
    if cfg!(target_os = "linux") {
        if let Some(wv) = app.get_webview_window("main") {
            wv.open_devtools();
        }
        return;
    }
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let mut pool_guard = pool.lock().unwrap();

    if pool_guard.devtools_open.contains(&tab_id) {
        pool_guard.devtools_open.remove(&tab_id);
        if let Some(webview) = pool_guard.webviews.get(&tab_id) {
            eprintln!("[open_devtools_tab] closing devtools for tab_id={}", tab_id);
            let _ = webview.close_devtools();
        }
    } else {
        pool_guard.devtools_open.insert(tab_id);
        if let Some(webview) = pool_guard.webviews.get(&tab_id) {
            eprintln!("[open_devtools_tab] opening devtools for tab_id={}", tab_id);
            webview.open_devtools();
        }
    }
}

#[tauri::command]
fn get_settings(app: AppHandle) -> SessionSettings {
    let settings = app.state::<Arc<Mutex<SessionSettings>>>();
    let settings_guard = settings.lock().unwrap();
    eprintln!("[DIAG:get_settings] return: auto_clear_on_exit={}", settings_guard.auto_clear_on_exit);
    settings_guard.clone()
}

#[tauri::command]
fn set_settings(settings: SessionSettings, app: AppHandle) {
    eprintln!("[DIAG:set_settings] ENTER: auto_clear_on_exit={}", settings.auto_clear_on_exit);
    let s = app.state::<Arc<Mutex<SessionSettings>>>();
    let mut s_guard = s.lock().unwrap();
    *s_guard = settings;
}

#[tauri::command]
fn clear_browsing_data(app: AppHandle) {
    // 清除所有活跃 webview 的 localStorage/sessionStorage
    if !cfg!(target_os = "linux") {
        let pool = app.state::<Arc<Mutex<WebViewPool>>>();
        let pool_guard = pool.lock().unwrap();
        for (_, wv) in &pool_guard.webviews {
            let _ = wv.eval(CLEAR_JS);
        }
    } else {
        if let Some(wv) = app.get_webview_window("main") {
            let _ = wv.eval(CLEAR_JS);
        }
    }
}

/// Linux only: 获取当前工具栏状态（标签列表 + 活跃 tab ID）
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct SessionSettings {
    auto_clear_on_exit: bool,
}

/// 清除浏览器数据的 JS 脚本（解除 webview 内 localStorage 和 sessionStorage 的绑定）
const CLEAR_JS: &str = r#"(function(){try{localStorage.clear();sessionStorage.clear();}catch(e){}})();"#;

#[derive(Clone, serde::Serialize)]
struct PopupPayload {
    url: String,
}

#[tauri::command]
fn request_popup(url: String, app: AppHandle) {
    debug_log!("[request_popup] url={}", url);
    let _ = app.emit("popup://request", PopupPayload { url });
}

#[tauri::command]
fn get_toolbar_state(app: AppHandle) -> ToolbarState {
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let pool_guard = pool.lock().unwrap();
    let tabs: Vec<TabInfo> = if cfg!(target_os = "linux") {
        pool_guard.lru_order.iter().filter_map(|&id| {
            pool_guard.tab_urls.get(&id).map(|url| TabInfo {
                id,
                url: url.clone(),
            })
        }).collect()
    } else {
        pool_guard.webviews.iter().map(|(&id, _)| TabInfo {
            id,
            url: String::new(),
        }).collect()
    };
    let active_id = pool_guard.active_tab_id.unwrap_or(0);
    eprintln!("[diag] get_toolbar_state: {} tabs, active_id={}", tabs.len(), active_id);
    if cfg!(target_os = "linux") {
        for t in &tabs {
            eprintln!("[diag] get_toolbar_state: tab id={} url={}", t.id, t.url);
        }
    }
    ToolbarState {
        active_tab_id: active_id,
        tabs,
    }
}

/// 前端 ResizeObserver 检测到内容区域尺寸变化时调用
/// 直接使用 viewport 坐标更新嵌入 webview 的位置和尺寸
#[tauri::command]
fn resize_content_area(x: f64, y: f64, width: f64, height: f64, app: AppHandle) {
    eprintln!("[diag] resize_content_area ENTER: pos={}x{} size={}x{}", x, y, width, height);

    // Linux: no child webviews (main webview is used directly)
    if cfg!(target_os = "linux") {
        eprintln!("[diag] resize_content_area: Linux no-op (no child webviews)");
        return;
    }

    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let mut pool_guard = pool.lock().unwrap();

    eprintln!("[diag] resize_content_area: active_tab_id={:?}", pool_guard.active_tab_id);

    // 零/负尺寸为隐藏信号：隐藏活跃 webview 但不更新存储的 content 尺寸
    if width <= 0.0 || height <= 0.0 {
        if let Some(id) = pool_guard.active_tab_id {
            if let Some(wv) = pool_guard.webviews.get(&id) {
                eprintln!("[diag] resize_content_area HIDE: hide() path");
                let _ = wv.hide();
            }
        } else {
            eprintln!("[diag] resize_content_area HIDE: active_tab_id is None, NO WEBVIEW TO HIDE");
        }
        return;
    }

    eprintln!("[diag] resize_content_area SHOW: updating content={}x{} size={}x{}", x, y, width, height);
    pool_guard.content_x = x;
    pool_guard.content_y = y;
    pool_guard.content_width = width;
    pool_guard.content_height = height;

    let active_id = pool_guard.active_tab_id;

    // 显示 active tab，隐藏 inactive tab
    for (&tab_id, webview) in &pool_guard.webviews {
        if active_id == Some(tab_id) {
            eprintln!("[diag] resize_content_area SHOW: showing+pos active tab_id={}", tab_id);
            let _ = webview.show();
            let _ = webview.set_position(LogicalPosition::new(x, y));
            let _ = webview.set_size(LogicalSize::new(width, height));
            let _ = webview.set_focus();
        } else {
            eprintln!("[diag] resize_content_area SHOW: hiding inactive tab_id={}", tab_id);
            let _ = webview.hide();
        }
    }
}

fn main() {
    tauri::Builder::default()
        .manage(Arc::new(Mutex::new(WebViewPool::new())))
        .manage(Arc::new(Mutex::new(SessionSettings {
            auto_clear_on_exit: false,
        })))
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
            get_toolbar_state,
            get_settings,
            set_settings,
            clear_browsing_data,
            request_popup,
        ])
        .plugin(
            tauri::plugin::Builder::<tauri::Wry>::new("toolbar")
                .on_page_load(move |webview, payload| {
                    eprintln!("[diag] toolbar: PLUGIN on_page_load event={:?} url={}", payload.event(), payload.url());
                    if cfg!(target_os = "linux") && matches!(payload.event(), PageLoadEvent::Finished) {
                        let url = payload.url().as_str();
                        if !url.starts_with("http://localhost") && !url.starts_with("tauri://") {
                            let app = webview.app_handle();
                            let pool = app.state::<Arc<Mutex<WebViewPool>>>();
                            let mut pool_guard = pool.lock().unwrap();

                            // Handle URL-based tab commands using manual string scanning
                            // (url crate query_pairs() doesn't work for about: scheme URLs)
                            if url.starts_with("about:blank") {
                                if let Some(_pos) = url.find("__mb_new=") {
                                    // Use atomic counter for tab IDs (Date.now() values are too large for i32)
                                    let new_id = NEXT_TAB_ID.fetch_add(1, Ordering::Relaxed);
                                    eprintln!("[diag] toolbar: URL_NEW tab id={}", new_id);
                                    pool_guard.insert_tab_only(new_id, "about:blank");
                                    pool_guard.active_tab_id = Some(new_id);
                                }
                                if let Some(pos) = url.find("__mb_close=") {
                                    let s = &url[pos + "__mb_close=".len()..];
                                    let v = s.split('&').next().unwrap_or(s);
                                    if let Ok(close_id) = v.parse::<i32>() {
                                        eprintln!("[diag] toolbar: URL_CLOSE tab id={}", close_id);
                                        pool_guard.tab_urls.remove(&close_id);
                                        pool_guard.lru_order.retain(|&id| id != close_id);
                                        if pool_guard.active_tab_id == Some(close_id) {
                                            pool_guard.active_tab_id = pool_guard.lru_order.back().copied();
                                        }
                                    }
                                }
                                if let Some(pos) = url.find("__mb_switch=") {
                                    let s = &url[pos + "__mb_switch=".len()..];
                                    let v = s.split('&').next().unwrap_or(s);
                                    if let Ok(switch_id) = v.parse::<i32>() {
                                        eprintln!("[diag] toolbar: URL_SWITCH tab id={}", switch_id);
                                        pool_guard.active_tab_id = Some(switch_id);
                                        let tab_url = pool_guard.tab_urls.get(&switch_id).cloned().unwrap_or_default();
                                        if !tab_url.is_empty() && tab_url != "about:blank" {
                                            drop(pool_guard);
                                            let escaped = tab_url.replace('\'', "\\'");
                                            let _ = webview.eval(&format!("window.location.href = '{}';", escaped));
                                            return;
                                        }
                                    }
                                }
                                if let Some(pos) = url.find("__mb_devtools=") {
                                    if let Ok(_devtools_id) = url[pos + "__mb_devtools=".len()..].split('&').next().unwrap_or_default().parse::<i32>() {
                                        eprintln!("[diag] toolbar: URL_DEVTOOLS id={}", _devtools_id);
                                        if let Some(wv) = webview.app_handle().get_webview_window("main") {
                                            wv.open_devtools();
                                        }
                                        let _ = webview.eval("window.history.back()");
                                        return;
                                    }
                                }
                            } else {
                                // Normal URL: update active tab's URL without matching
                                if let Some(active_id) = pool_guard.active_tab_id {
                                    pool_guard.tab_urls.insert(active_id, url.to_string());
                                }
                            }

                            // Build inline tab state
                            let tab_list: Vec<TabInfo> = pool_guard.lru_order.iter().filter_map(|&id| {
                                pool_guard.tab_urls.get(&id).map(|u| TabInfo { id, url: u.clone() })
                            }).collect();
                            let tabs_json = serde_json::to_string(&tab_list).unwrap_or_default();
                            let active_id = pool_guard.active_tab_id.unwrap_or(0);
                            drop(pool_guard);

                            eprintln!("[diag] toolbar: INJECTING tabs_json={} active_id={}", tabs_json, active_id);
                            let script = format!(
                                r#";(function(){{try{{window.__MB_INLINE_TABS={};window.__MB_INLINE_ACTIVE={};{}}}catch(e){{}}}})();"#,
                                tabs_json, active_id, TOOLBAR_SCRIPT
                            );
                            let r = webview.eval(&script);
                            eprintln!("[diag] toolbar: inject result={:?}", r);
                        } else {
                            eprintln!("[diag] toolbar: SKIP (app url) url={}", url);
                        }
                    }
                })
                .build()
        )
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
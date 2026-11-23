#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

static NEXT_TAB_ID: AtomicI32 = AtomicI32::new(1000);
use tauri::{
    webview::{NewWindowResponse, PageLoadEvent, WebviewBuilder},
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder,
    WindowEvent,
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

/// serde_json 生成带引号的 JS 字符串字面量，自动转义 ' \ 等全部字符
fn js_string_literal(s: &str) -> String {
    serde_json::to_string(s).expect("String 序列化不会失败")
}

/// 调试日志：仅在 debug 构建中输出
macro_rules! debug_log {
    ($($arg:tt)*) => {
        if cfg!(debug_assertions) {
            std::eprintln!($($arg)*);
        }
    };
}

/// 取池锁：线程 panic 导致锁中毒时恢复数据，避免整个应用被毒锁拖垮
fn lock_pool(pool: &Arc<Mutex<WebViewPool>>) -> MutexGuard<'_, WebViewPool> {
    pool.lock().unwrap_or_else(|e| e.into_inner())
}

/// 取设置锁：同上
fn lock_settings(settings: &Arc<Mutex<SessionSettings>>) -> MutexGuard<'_, SessionSettings> {
    settings.lock().unwrap_or_else(|e| e.into_inner())
}

/// URL 比较：忽略末尾斜杠、fragment 与大小写差异
/// 用于判断「页面回传的 URL」与「Rust 侧记录的 URL」是否同一条
fn urls_equivalent(a: &str, b: &str) -> bool {
    /// 仅 scheme + host 大小写不敏感；路径与查询串大小写敏感（/Path 与 /path 常是两个页面）
    fn normalize(u: &str) -> String {
        let mut s = u.trim().to_string();
        if let Some(idx) = s.find('#') {
            s.truncate(idx);
        }
        if let Some(pos) = s.find("://") {
            let scheme = s[..pos].to_ascii_lowercase();
            let rest = &s[pos + 3..];
            let end = rest.find(['/', '?']).unwrap_or(rest.len());
            let (host, tail) = rest.split_at(end);
            s = format!("{}://{}{}", scheme, host.to_ascii_lowercase(), tail);
        }
        while s.len() > 1 && s.ends_with('/') {
            s.pop();
        }
        s
    }
    normalize(a) == normalize(b)
}

/// 查询串编码集：保留 unreserved 字符，其余百分号编码
const QUERY_ENCODE_SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// 判断输入是否形如主机名（含 IP、localhost、域名）
fn is_host_like(input: &str) -> bool {
    if input.is_empty() || input.chars().any(char::is_whitespace) {
        return false;
    }
    let authority = input.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.split(':').next().unwrap_or("");
    if host.is_empty() {
        return false;
    }
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() == 4
        && labels
            .iter()
            .all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_digit()))
    {
        return true;
    }
    if labels.len() < 2 {
        return false;
    }
    let tld = labels[labels.len() - 1];
    tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic())
}

/// 本机/内网地址：http 协议优先，避免内网 http 服务被 https 拦下
fn is_local_host(input: &str) -> bool {
    let authority = input.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.split(':').next().unwrap_or("");
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let nums: Vec<u32> = host.split('.').filter_map(|l| l.parse::<u32>().ok()).collect();
    if nums.len() != 4 {
        return false;
    }
    nums[0] == 127
        || nums[0] == 10
        || (nums[0] == 192 && nums[1] == 168)
        || (nums[0] == 172 && (16..=31).contains(&nums[1]))
}

/// 归一化地址栏输入：判定为 URL 还是搜索词
/// 统一入口：React 工具栏与 Linux 注入工具栏都走这里，避免两套规则漂移
fn normalize_url_input(raw: &str) -> String {
    let input = raw.trim();
    if input.is_empty() {
        return String::new();
    }
    if input.starts_with("about:") || input.contains("://") {
        return input.to_string();
    }
    if is_host_like(input) {
        let scheme = if is_local_host(input) { "http" } else { "https" };
        return format!("{}://{}", scheme, input);
    }
    format!(
        "https://www.google.com/search?q={}",
        percent_encoding::utf8_percent_encode(input, QUERY_ENCODE_SET)
    )
}

/// 解析 about:blank?__mb_xxx=yyy 协议命令
/// url crate 的 query_pairs 对 about: scheme 不友好，这里手工切分并做百分号解码
/// 只认 about: URL 且 __mb_ 前缀的 key，避免把普通页面的查询串误当成命令。
/// 注意：WebKitGTK 没有隔离 JS 世界，外部页面仍可自行导航到 about:blank?__mb_xxx=1
/// 来触发命令（架构固有局限）；这里只能保证状态完整性——未知标签 id 一律忽略
fn parse_command(url: &str) -> Option<(String, String)> {
    if !url.starts_with("about:") {
        return None;
    }
    let query = url.split_once('?')?.1;
    let (key, value) = query.split_once('=')?;
    if !key.starts_with("__mb_") {
        return None;
    }
    let value = value.split('&').next().unwrap_or(value);
    Some((
        key.to_string(),
        percent_encoding::percent_decode_str(value)
            .decode_utf8_lossy()
            .to_string(),
    ))
}

/// 主 webview 导航：Linux 工具栏方案唯一的页面切换通道
fn navigate_main(app: &AppHandle, url: &str) {
    if let Some(wv) = app.get_webview_window("main") {
        let _ = wv.eval(format!("window.location.href = {};", js_string_literal(url)));
    }
}

/// 主 webview 导航（替换当前历史条目）
/// Linux 协议命令用此方式，避免 about:blank 跳转残留在 webview 历史里
fn navigate_main_replace(app: &AppHandle, url: &str) {
    if let Some(wv) = app.get_webview_window("main") {
        let _ = wv.eval(format!("window.location.replace({});", js_string_literal(url)));
    }
}

/// 按 tab 执行 JS：Linux 无子 webview，统一在主窗口执行；其他平台在子 webview 执行
fn eval_tab_js(app: &AppHandle, tab_id: i32, js: &str) {
    if cfg!(target_os = "linux") {
        if let Some(wv) = app.get_webview_window("main") {
            let _ = wv.eval(js);
        }
        return;
    }
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let pool_guard = lock_pool(&pool);
    if let Some(webview) = pool_guard.webviews.get(&tab_id) {
        let _ = webview.eval(js);
    }
}

/// 中文右键菜单脚本（注入到每个 content webview）
/// Linux 上后退/前进走 Rust 协议命令（window.__mb_protocol_cmd），其余平台用 webview 自身历史
const CTX_MENU_SCRIPT: &str = r#"(function(){
    if(window.__mb_ctx_installed)return;window.__mb_ctx_installed=true;
    function cmd(k,v){window.location.replace('about:blank?__mb_'+k+'='+encodeURIComponent(v==null?'':v))}
    function back(){if(window.__mb_protocol_cmd){cmd('back',window.__mb_active_tab_id||0)}else{window.history.back()}}
    function forward(){if(window.__mb_protocol_cmd){cmd('forward',window.__mb_active_tab_id||0)}else{window.history.forward()}}
    function showMenu(e){
        e.preventDefault();
        e.stopPropagation();
        var m=document.getElementById('__mb_menu');if(m)m.remove();
        var l=e.target.closest('a');var lu=l&&l.href;
        var menu=document.createElement('div');
        menu.id='__mb_menu';
        // z-index 必须与工具栏一致（工具栏用 2147483647）：菜单挂在 body 上、晚于工具栏插入，
        // 同层级时 DOM 顺序决定遮挡关系，否则在欢迎页会被铺满全屏的工具栏盖住
        menu.style.cssText='position:fixed;z-index:2147483647;background:#1a1a2e;border:1px solid #2d2d4a;border-radius:8px;padding:4px;min-width:140px;font:13px system-ui,sans-serif;color:#e8e8f0;box-shadow:0 4px 12px rgba(0,0,0,0.4)';
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
            add('打开链接',function(){window.location.replace(lu)});
            add('复制链接',function(){try{navigator.clipboard.writeText(lu)}catch(e){}});
            menu.appendChild(document.createElement('hr'));
        }
        add('后退',back);
        add('前进',forward);
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
/// 由 Rust 侧注入（页面加载完成时 + 周期性兜底），每次注入都会带上最新状态
/// 状态放在 window.__MB_STATE__（Rust 侧写入），脚本据此重建工具栏：
/// - 状态未变化时不重建（避免打断用户输入）
/// - 文档尚未就绪时轮询等待 body，避免页面慢加载时工具栏丢失
///
/// 不使用 IPC 调用（外部页面 IPC 不可用），改用 URL 导航模式
const TOOLBAR_SCRIPT: &str = r#";(function(){
    try{
    var L=window.location.href;
    var state=window.__MB_STATE__||{tabs:[],active:0,settings:{auto_clear_on_exit:false},app_urls:[]};
    // 应用自身页面（dev server / 自定义协议）不注入工具栏：按精确前缀判断，
    // 不能用 http://localhost 前缀，否则浏览本地站点时会被误判
    var appUrls=state.app_urls||[];
    for(var ai=0;ai<appUrls.length;ai++){if(L&&L.indexOf(appUrls[ai])===0)return}
    var sig=JSON.stringify([state.tabs,state.active,state.settings]);
    if(document.getElementById('__mb_tb')&&window.__MB_SIG__===sig)return;
    window.__MB_SIG__=sig;
    var oldInput=document.querySelector('#__mb_tb input');
    var keepValue=oldInput?oldInput.value:'';
    var keepFocus=!!(oldInput&&document.activeElement===oldInput);
    var old=document.getElementById('__mb_tb');if(old)old.remove();
    var oldMenu=document.getElementById('__mb_menu');if(oldMenu)oldMenu.remove();
    // Linux 协议模式：后退/前进等命令交由 Rust 处理，避免污染 webview 自身历史
    window.__mb_protocol_cmd=true;
    // Intercept target=_blank links（只装一次）
    if(!window.__mb_blank_hook){window.__mb_blank_hook=true;document.addEventListener('click',function(e){var a=e.target.closest('a');if(a&&a.target==='_blank'){e.preventDefault();window.location.replace(a.href)}},true);}
    // Tab state from Rust
    var tabs=state.tabs||[];
    var activeId=state.active||0;
    var settings=state.settings||{auto_clear_on_exit:false};
    window.__mb_active_tab_id=activeId;
    // 命令通过 about:blank 协议回传 Rust；用 location.replace 不在 webview 历史里留痕
    function cmd(k,v){window.location.replace('about:blank?__mb_'+k+'='+encodeURIComponent(v==null?'':v))}
    function mount(){
        try{
        if(document.getElementById('__mb_tb'))return true;
        if(!document.body)return false;
        buildUI(tabs,activeId,settings);
        return true;
        }catch(e){return false}
    }
    if(!mount()){var iv=setInterval(function(){if(mount())clearInterval(iv)},100);}
    function buildUI(tabs,activeId,settings){
    var d=document.createElement('div');d.id='__mb_tb';d.style.cssText='position:fixed;top:0;left:0;right:0;z-index:2147483647;background:#1a1a2e;border-bottom:1px solid #2d2d4a;font:13px system-ui,sans-serif;color:#e8e8f0;box-shadow:0 2px 8px rgba(0,0,0,0.4);';
    // Tab bar
    var tb=document.createElement('div');tb.style.cssText='display:flex;align-items:center;gap:2px;padding:2px 8px;border-bottom:1px solid #2d2d4a;';
    tabs.forEach(function(t){
        var t2=document.createElement('div');t2.style.cssText='display:flex;align-items:center;gap:4px;padding:4px 6px;border-radius:6px;cursor:pointer;min-width:60px;max-width:140px;height:26px;font-size:12px;flex-shrink:0;color:#e8e8f0;background:'+(t.id===activeId?'#2d2d52':'transparent')+';';
        t2.onmouseenter=function(){this.style.background='#2d2d52'};
        t2.onmouseleave=function(){this.style.background=t.id===activeId?'#2d2d52':'transparent'};
        t2.onclick=function(){if(t.id!==activeId)cmd('switch',t.id)};
        var sp=document.createElement('span');sp.style.cssText='overflow:hidden;text-overflow:ellipsis;white-space:nowrap;flex:1;';sp.textContent=t.url&&t.url!=='about:blank'?t.url:'新标签页';
        t2.appendChild(sp);
        var cx=document.createElement('span');cx.textContent='×';cx.style.cssText='width:16px;height:16px;border-radius:4px;display:flex;align-items:center;justify-content:center;font-size:12px;color:#606080;cursor:pointer;flex-shrink:0;';
        cx.onmouseenter=function(){this.style.background='#3d3d62'};cx.onmouseleave=function(){this.style.background='transparent'};
        cx.onclick=function(e){e.stopPropagation();cmd('close',t.id)};
        t2.appendChild(cx);tb.appendChild(t2);
    });
    var nt=document.createElement('button');nt.textContent='+';nt.style.cssText='width:24px;height:24px;border:none;background:transparent;color:#a0a0c0;border-radius:6px;cursor:pointer;font-size:16px;display:flex;align-items:center;justify-content:center;flex-shrink:0;margin-left:2px;';
    nt.onmouseenter=function(){this.style.background='#2d2d52'};nt.onmouseleave=function(){this.style.background='transparent'};
    nt.onclick=function(){cmd('new',Date.now())};
    tb.appendChild(nt);d.appendChild(tb);
    // Navigation bar
    var nb=document.createElement('div');nb.style.cssText='display:flex;align-items:center;gap:4px;padding:6px 10px;';
    var act=null;for(var i=0;i<tabs.length;i++){if(tabs[i].id===activeId)act=tabs[i]}
    function bt(t,f,disabled){
        var b=document.createElement('button');b.textContent=t;b.style.cssText='width:28px;height:28px;border:none;background:transparent;color:#a0a0c0;border-radius:6px;cursor:pointer;font-size:14px;display:flex;align-items:center;justify-content:center;flex-shrink:0;'+(disabled?'opacity:0.3;':'');
        b.onmouseenter=function(){this.style.background='#2d2d52'};b.onmouseleave=function(){this.style.background='transparent'};
        b.onclick=function(){if(!disabled)f()};return b;
    }
    nb.appendChild(bt('←',function(){cmd('back',activeId)},!act||!act.can_back));
    nb.appendChild(bt('→',function(){cmd('forward',activeId)},!act||!act.can_forward));
    nb.appendChild(bt('↻',function(){window.location.reload()}));
    var u=document.createElement('input');u.style.cssText='flex:1;height:32px;border:1.5px solid #2d2d4a;border-radius:8px;background:#0f0f1a;color:#e8e8f0;padding:0 10px;font-size:13px;outline:none;min-width:0;';u.value=L.startsWith('about:blank')?'':L;
    u.placeholder='输入网址或搜索...';
    u.onfocus=function(){this.select()};
    // 用户输入缓冲：程序化回填 u.value 不触发 input 事件，缓冲值不会被地址栏同步覆盖
    // （点「→」按钮会让输入框失焦，失焦瞬间同步会把 u.value 重置回当前 URL，导致点击时读到旧值）
    if(keepFocus)u.value=keepValue;
    u.onkeydown=function(e){if(e.key==='Enter'){var v=u.value.trim();if(v)cmd('nav',v)}};
    nb.appendChild(u);
    var gb=document.createElement('button');gb.textContent='→';gb.style.cssText='width:28px;height:28px;border:none;background:#6366f1;color:#fff;border-radius:6px;cursor:pointer;font-size:14px;display:flex;align-items:center;justify-content:center;flex-shrink:0;';
    // 点击按钮会让输入框先失焦，失焦瞬间地址栏同步会把输入值重置回当前 URL，
    // 因此在 mousedown（失焦之前）先快照用户输入的值
    var snap='';
    gb.onmousedown=function(){snap=u.value};
    gb.onclick=function(){var v=(snap||u.value).trim();if(v)cmd('nav',v)};
    nb.appendChild(gb);
    nb.appendChild(bt('🐞',function(){cmd('devtools',activeId)}));
    nb.appendChild(bt('⚙',function(){var p=document.getElementById('__mb_settings');if(p)p.style.display=p.style.display==='none'?'flex':'none'}));
    d.appendChild(nb);
    // Settings panel: 退出自动清除 / 清除缓存
    var sp2=document.createElement('div');sp2.id='__mb_settings';sp2.style.cssText='display:none;align-items:center;gap:14px;padding:6px 12px;border-top:1px solid #2d2d4a;background:#151527;font:12px system-ui,sans-serif;color:#e8e8f0;';
    var row=document.createElement('span');row.textContent='退出时清除数据';
    var tg=document.createElement('div');tg.style.cssText='width:32px;height:18px;border-radius:9px;cursor:pointer;flex-shrink:0;background:'+(settings.auto_clear_on_exit?'#6366f1':'#2d2d4a')+';position:relative;';
    tg.innerHTML='<div style="width:14px;height:14px;border-radius:50%;background:#fff;position:absolute;top:2px;left:'+(settings.auto_clear_on_exit?16:2)+';transition:left .15s"></div>';
    tg.onclick=function(){cmd('autoclear','1')};
    var clr=document.createElement('button');clr.textContent='清除缓存';clr.style.cssText='padding:3px 10px;border:none;border-radius:5px;background:#6366f1;color:#fff;font-size:11px;cursor:pointer;';
    clr.onclick=function(){cmd('clear_cache','1')};
    sp2.appendChild(row);sp2.appendChild(tg);sp2.appendChild(clr);
    d.appendChild(sp2);
    // Welcome page for about:blank (including about:blank?__mb_new=... and __mb_close=...)
    if(L.startsWith('about:blank')){
        var w=document.createElement('div');w.id='__mb_welcome';w.style.cssText='display:flex;align-items:center;justify-content:center;height:calc(100vh - 80px);background:#0f0f1a;';
        var wc=document.createElement('div');wc.style.cssText='text-align:center;color:#606080;';
        wc.innerHTML='<div style="font-size:48px;opacity:0.4;margin-bottom:16px;">🌐</div><h2 style="font-size:18px;font-weight:600;margin-bottom:8px;color:#e8e8f0;">迷你浏览器</h2><p>输入网址开始浏览</p>';
        w.appendChild(wc);d.appendChild(w);
    }
    document.body.prepend(d);
    if(keepFocus){try{u.focus();u.setSelectionRange(u.value.length,u.value.length)}catch(e){}}
    var p=function(){var e=document.getElementById('__mb_tb');if(e){var h=e.offsetHeight;document.body.style.paddingTop=Math.max(parseInt(document.body.style.paddingTop)||0,h)+'px'}};
    setTimeout(p,50);
    // 地址栏同步：仅在输入框未聚焦时回填，避免覆盖用户正在输入的内容
    if(window.__MB_SYNC_IV__)clearInterval(window.__MB_SYNC_IV__);
    window.__MB_SYNC_IV__=setInterval(function(){if(document.activeElement===u)return;var b=window.location.href;if(b&&!b.startsWith('about:blank')&&u.value!==b)u.value=b},500);
    } // buildUI end
    }catch(e){}
})();"#;

/// 最大标签数量限制
const MAX_TABS: usize = 10;

/// LRU 阈值：后台超过此数量的 tab 将触发 WebView 销毁
const LRU_THRESHOLD: usize = 3;

/// 单个 tab 保留的最大历史条数，防止长时间浏览导致内存无限增长
const MAX_HISTORY: usize = 100;

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
    can_back: bool,
    can_forward: bool,
}

/// 单个标签页的导航状态：URL + 历史栈 + 游标
/// Linux 上所有标签共用一个 webview，历史必须由 Rust 侧按 tab 维护，
/// 否则后退会在标签之间串台（且协议跳转会残留在 webview 历史里形成死循环）
struct TabState {
    url: String,
    history: Vec<String>,
    history_index: usize,
    /// 临时条目标记：(所属历史下标, 写入时的 URL)
    /// 只有该条目仍是当前条目时，才允许被 finalize 用页面最终 URL 改写，
    /// 避免迟到的 load 事件覆盖其它条目
    provisional: Option<(usize, String)>,
}

impl TabState {
    fn new(url: &str) -> Self {
        TabState {
            url: url.to_string(),
            history: vec![url.to_string()],
            history_index: 0,
            provisional: Some((0, url.to_string())),
        }
    }

    /// 压栈一个新条目并截断前向分支
    fn push(&mut self, url: &str) {
        self.history.truncate(self.history_index + 1);
        self.history.push(url.to_string());
        self.history_index = self.history.len() - 1;
        self.url = url.to_string();
        if self.history.len() > MAX_HISTORY {
            let excess = self.history.len() - MAX_HISTORY;
            self.history.drain(0..excess);
            self.history_index -= excess;
        }
    }

    /// 记录一次已完成的导航：与当前条目等价则只更新 URL，否则截断前向分支后压栈
    fn record(&mut self, url: &str) {
        if urls_equivalent(&self.url, url) {
            self.url = url.to_string();
            self.provisional = None;
            return;
        }
        self.push(url);
        self.provisional = None;
    }

    /// 导航开始时写入临时条目：页面最终 URL 可能因重定向而不同，稍后由 finalize 修正
    fn record_provisional(&mut self, url: &str) {
        if urls_equivalent(&self.url, url) {
            // 已有同 URL 的条目：保留既有临时标记，等页面最终 URL 回来再修正
            return;
        }
        self.push(url);
        self.provisional = Some((self.history_index, url.to_string()));
    }

    /// 页面加载完成：用最终 URL 修正临时条目
    /// - 临时条目仍是当前条目 → 就地改写（重定向场景不产生多余历史）
    /// - 否则只在与当前条目等价时更新 URL；不相等则忽略（迟到的 load 事件不得污染历史）
    fn finalize(&mut self, url: &str) {
        match self.provisional {
            Some((idx, _)) if idx == self.history_index => {
                self.provisional = None;
                if !urls_equivalent(&self.history[idx], url) {
                    self.history[idx] = url.to_string();
                }
                self.url = url.to_string();
            }
            _ => {
                self.provisional = None;
                if urls_equivalent(&self.url, url) {
                    self.url = url.to_string();
                }
            }
        }
    }

    fn can_back(&self) -> bool {
        self.history_index > 0
    }

    fn can_forward(&self) -> bool {
        self.history_index + 1 < self.history.len()
    }

    fn go_back(&mut self) -> Option<String> {
        if self.history_index == 0 {
            return None;
        }
        // 游标移动后，先前的临时条目不再属于当前条目
        self.provisional = None;
        self.history_index -= 1;
        let url = self.history[self.history_index].clone();
        self.url = url.clone();
        Some(url)
    }

    fn go_forward(&mut self) -> Option<String> {
        if !self.can_forward() {
            return None;
        }
        self.provisional = None;
        self.history_index += 1;
        let url = self.history[self.history_index].clone();
        self.url = url.clone();
        Some(url)
    }
}

/// 存储所有嵌入 webview 的句柄
struct WebViewPool {
    webviews: HashMap<i32, tauri::Webview>,
    tabs: HashMap<i32, TabState>,
    active_tab_id: Option<i32>,
    /// 标签展示顺序（按创建顺序，与 LRU 无关）
    tab_order: VecDeque<i32>,
    /// LRU 顺序：最近使用的在末尾，最久未使用的在开头
    lru_order: VecDeque<i32>,
    /// 内容区域在窗口中的位置（viewport 相对坐标，CSS 逻辑像素）
    content_x: f64,
    content_y: f64,
    content_width: f64,
    content_height: f64,
    /// 已打开开发者工具的标签页集合（切换 tab 时跟随隐藏/显示）
    devtools_open: HashSet<i32>,
    /// 主 webview 最近一次导航到的 URL（load 事件不触发时用于兜底注入工具栏）
    last_page_url: String,
    /// 应用自身页面的 URL 前缀（dev server + 自定义协议）
    /// 必须精确匹配：不能用 "http://localhost" 前缀，否则浏览 localhost:3000 等本地站点时
    /// 会被当成应用自身页面 → 不注入工具栏、不记历史、兜底线程也跳过 → 该页完全不可用
    app_url_prefixes: Vec<String>,
    /// 进行中导航的归属标签：(tab_id, 目标 URL)
    /// 页面加载事件到达时可能已经切换过标签，必须按归属标签记账，否则会写到别的标签上
    pending_nav: Option<(i32, String)>,
}

impl WebViewPool {
    fn new() -> Self {
        WebViewPool {
            webviews: HashMap::with_capacity(MAX_TABS),
            tabs: HashMap::with_capacity(MAX_TABS),
            active_tab_id: None,
            tab_order: VecDeque::with_capacity(MAX_TABS),
            lru_order: VecDeque::with_capacity(MAX_TABS),
            content_x: 0.0,
            content_y: 0.0,
            content_width: 1200.0,
            content_height: 800.0,
            devtools_open: HashSet::new(),
            last_page_url: String::new(),
            pending_nav: None,
            app_url_prefixes: Vec::new(),
        }
    }

    /// 登记标签（已存在则更新历史，不存在则新建）
    fn upsert_tab(&mut self, tab_id: i32, url: &str) {
        match self.tabs.get_mut(&tab_id) {
            Some(tab) => tab.record_provisional(url),
            None => {
                self.tabs.insert(tab_id, TabState::new(url));
                self.tab_order.retain(|&id| id != tab_id);
                self.tab_order.push_back(tab_id);
                self.lru_order.retain(|&id| id != tab_id);
                self.lru_order.push_back(tab_id);
            }
        }
        // 登记导航归属：页面加载事件按此标签记账
        self.pending_nav = Some((tab_id, url.to_string()));
    }

    /// 登记 webview；若该 tab 已有旧句柄，返回旧句柄交由调用方在锁外关闭
    /// （否则并发创建同一 tab 时旧 webview 会变成孤儿，仍挂在窗口上继续加载）
    fn insert(&mut self, tab_id: i32, url: &str, webview: tauri::Webview) -> Option<tauri::Webview> {
        let old = self.webviews.insert(tab_id, webview);
        self.upsert_tab(tab_id, url);
        old
    }

    fn insert_tab_only(&mut self, tab_id: i32, url: &str) {
        // Linux: store tab info without a webview
        self.upsert_tab(tab_id, url);
    }

    fn remove(&mut self, tab_id: i32) -> Option<tauri::Webview> {
        if self.active_tab_id == Some(tab_id) {
            self.active_tab_id = None;
        }
        self.lru_order.retain(|&id| id != tab_id);
        self.tab_order.retain(|&id| id != tab_id);
        self.tabs.remove(&tab_id);
        self.devtools_open.remove(&tab_id);
        self.webviews.remove(&tab_id)
    }

    /// LRU 淘汰：当 webview 数超过阈值时，摘除最久未使用的，返回待关闭的 webview（由调用方在锁外关闭）
    fn evict_lru(&mut self) -> Vec<tauri::Webview> {
        let mut evicted = Vec::new();
        while self.webviews.len() > LRU_THRESHOLD {
            let evict_id = self.lru_order.iter().find(|&&id| {
                self.active_tab_id != Some(id) && self.webviews.contains_key(&id)
            }).copied();

            match evict_id {
                Some(id) => {
                    debug_log!("[evict_lru] evicting tab_id={}", id);
                    if let Some(webview) = self.webviews.remove(&id) {
                        self.lru_order.retain(|&lid| lid != id);
                        // 同步清理 devtools 状态，否则淘汰过的标签第一次点 🐞 只会执行无效的 close
                        self.devtools_open.remove(&id);
                        evicted.push(webview);
                    }
                }
                None => break,
            }
        }
        evicted
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

    fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    fn set_active(&mut self, tab_id: i32) {
        self.active_tab_id = Some(tab_id);
        self.touch_lru(tab_id);
    }

    fn tab_url(&self, tab_id: i32) -> Option<String> {
        self.tabs.get(&tab_id).map(|t| t.url.clone())
    }

    fn record_navigation(&mut self, tab_id: i32, url: &str) {
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            tab.record(url);
        }
    }

    /// 导航开始：写入临时条目（后续由 finalize 用最终 URL 修正）
    fn begin_navigation(&mut self, tab_id: i32, url: &str) {
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            tab.record_provisional(url);
        }
    }

    /// 导航完成：用最终 URL 修正临时条目
    fn finish_navigation(&mut self, tab_id: i32, url: &str) {
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            tab.finalize(url);
        }
    }

    fn go_back(&mut self, tab_id: i32) -> Option<String> {
        self.tabs.get_mut(&tab_id).and_then(|t| t.go_back())
    }

    fn go_forward(&mut self, tab_id: i32) -> Option<String> {
        self.tabs.get_mut(&tab_id).and_then(|t| t.go_forward())
    }
}

#[tauri::command]
fn create_tab(tab_id: i32, url: String, x: f64, y: f64, width: f64, height: f64, app: AppHandle) -> Result<(), String> {
    debug_log!("[diag] create_tab ENTER: tab_id={} url={} pos={}x{} size={}x{}", tab_id, url, x, y, width, height);

    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let target = normalize_url_input(&url);

    if cfg!(target_os = "linux") {
        // Linux: 不创建子 webview（避免 GTK 分离问题），只存储标签信息
        let mut pool_guard = lock_pool(&pool);
        if !pool_guard.tabs.contains_key(&tab_id) && pool_guard.tab_count() >= MAX_TABS {
            return Err(format!("达到最大标签数量限制 ({})", MAX_TABS));
        }
        pool_guard.insert_tab_only(tab_id, &target);
        pool_guard.content_x = x;
        pool_guard.content_y = y;
        pool_guard.content_width = width;
        pool_guard.content_height = height;
        debug_log!("[diag] create_tab DONE (Linux, no add_child): pool has {} tabs, content={}x{} size={}x{}",
            pool_guard.lru_order.len(), x, y, width, height);
        return Ok(());
    }

    // macOS/Windows: 上限检查与 add_child 分离，避免持锁做同步 UI 调用
    // 上限只对「新建标签」生效：重建被 LRU 淘汰标签的 webview 不应被拒（否则该标签永久无法恢复）
    if lock_pool(&pool).tabs.contains_key(&tab_id) {
        debug_log!("[diag] create_tab: rebuilding webview for existing tab_id={}", tab_id);
    } else if lock_pool(&pool).tab_count() >= MAX_TABS {
        return Err(format!("达到最大标签数量限制 ({})", MAX_TABS));
    }

    // 使用 add_child 创建嵌入 webview
    let label = format!("content-{}", tab_id);
    let parsed_url = target.parse::<url::Url>().map_err(|e| format!("URL 解析失败: {}", e))?;

    let height_script = format!("window.__mb_content_height = {};", height);
    let init_script = CTX_MENU_SCRIPT.to_owned() + POPUP_SCRIPT + &height_script;

    let nav_handle = app.clone();
    let load_handle = app.clone();
    let nav_tab_id = tab_id;
    let load_tab_id = tab_id;
    let nav_app = app.clone();
    let nav_app_popup = app.clone();
    let nav_pool = app.state::<Arc<Mutex<WebViewPool>>>().inner().clone();

    let webview_builder = WebviewBuilder::new(&label, WebviewUrl::External(parsed_url))
        .user_agent(user_agent())
        .initialization_script(&init_script)
        .initialization_script_for_all_frames(POPUP_IFRAME_SCRIPT)
        .on_new_window(move |url, _features| {
            let url_str = url.to_string();
            debug_log!("[on_new_window] url={}", url_str);
            let _ = nav_app_popup.emit("popup://request", PopupPayload { url: url_str });
            NewWindowResponse::Deny
        })
        .on_navigation(move |url| {
            let url_str = url.to_string();
            debug_log!("[on_navigation] tab_id={} url={} ALLOW={}", nav_tab_id, url_str, true);
            lock_pool(&nav_pool).record_navigation(nav_tab_id, &url_str);
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

    let evicted = {
        let mut pool_guard = lock_pool(&pool);
        // 入口检查后已释放锁，add_child 期间可能有并发登记，插入前复检防止超限
        // （仅对新建标签；重建已有标签的 webview 不受上限约束）
        if !pool_guard.tabs.contains_key(&tab_id) && pool_guard.tab_count() >= MAX_TABS {
            drop(pool_guard);
            let _ = webview.close();
            return Err(format!("达到最大标签数量限制 ({})", MAX_TABS));
        }
        let orphan = pool_guard.insert(tab_id, &target, webview);
        pool_guard.content_x = x;
        pool_guard.content_y = y;
        pool_guard.content_width = width;
        pool_guard.content_height = height;
        let tab_count = pool_guard.tab_count();
        let evicted = pool_guard.evict_lru();
        debug_log!("[diag] create_tab DONE: pool has {} tabs, content={}x{} size={}x{}",
            tab_count, x, y, width, height);
        (evicted, orphan)
    };
    // 销毁 webview 属 UI 操作，放在锁外执行
    for w in evicted.0 {
        let _ = w.close();
    }
    if let Some(old) = evicted.1 {
        debug_log!("[diag] create_tab: closing orphaned webview for tab_id={}", tab_id);
        let _ = old.close();
    }
    Ok(())
}

#[tauri::command]
fn activate_tab(active_tab_id: i32, app: AppHandle) -> ActivateResult {
    debug_log!("[diag] activate_tab ENTER: active_tab_id={}", active_tab_id);
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let mut pool_guard = lock_pool(&pool);

    let prev_active = pool_guard.active_tab_id.replace(active_tab_id);
    debug_log!("[diag] activate_tab: prev_active={:?} webviews_count={}", prev_active, pool_guard.webviews.len());

    if cfg!(target_os = "linux") {
        // Linux: 导航主 webview 到该 tab 的 URL
        let url = pool_guard.tab_url(active_tab_id).unwrap_or_default();
        drop(pool_guard);
        if !url.is_empty() && url != "about:blank" {
            debug_log!("[diag] activate_tab: Linux navigating main webview to url={}", url);
            navigate_main(&app, &url);
        }
        return ActivateResult {
            tab_id: active_tab_id,
            needs_recreate: false,
            exists: true,
        };
    }

    // macOS/Windows: 锁内只收集句柄与状态，hide/show 等 UI 调用放到锁外
    let prev_to_hide: Option<(i32, Option<tauri::Webview>, bool)> =
        match prev_active.filter(|&id| id != active_tab_id) {
            Some(prev_id) => {
                let hide = pool_guard.webviews.get(&prev_id).cloned();
                let had_devtools = pool_guard.devtools_open.remove(&prev_id);
                Some((prev_id, hide, had_devtools))
            }
            None => None,
        };

    // 记录是否需要重建：先完成旧 webview 的隐藏，再统一早退
    // （否则切到被 LRU 淘汰的标签时旧 webview 永远不隐藏，且旧标签的 devtools 窗口泄漏）
    let needs_recreate = !pool_guard.has_webview(active_tab_id);
    if needs_recreate {
        debug_log!("[diag] activate_tab: webview NOT FOUND for tab_id={}, needs recreate", active_tab_id);
    }

    pool_guard.touch_lru(active_tab_id);
    let target = pool_guard.webviews.get(&active_tab_id).cloned();
    let (cx, cy, cw, ch) = (
        pool_guard.content_x,
        pool_guard.content_y,
        pool_guard.content_width,
        pool_guard.content_height,
    );
    drop(pool_guard);

    if let Some((prev_id, prev_wv, had_devtools)) = prev_to_hide {
        debug_log!("[diag] activate_tab: hiding prev tab_id={}", prev_id);
        if let Some(webview) = &prev_wv {
            let _ = webview.hide();
            if had_devtools {
                debug_log!("[diag] activate_tab: closing devtools for prev tab_id={}", prev_id);
                webview.close_devtools();
            }
        }
    }

    if needs_recreate {
        return ActivateResult {
            tab_id: active_tab_id,
            needs_recreate: true,
            exists: false,
        };
    }

    if let Some(webview) = target {
        debug_log!("[diag] activate_tab: setting pos={}x{} size={}x{} then show/set_focus", cx, cy, cw, ch);
        let _ = webview.set_position(LogicalPosition::new(cx, cy));
        let _ = webview.set_size(LogicalSize::new(cw, ch));
        let _ = webview.show();
        let _ = webview.set_focus();
        debug_log!("[diag] activate_tab DONE: show+position done");
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
    let webview = {
        let mut pool_guard = lock_pool(&pool);
        // Don't close the last tab
        if pool_guard.tab_count() <= 1 {
            debug_log!("[close_tab] refusing to close last tab");
            return;
        }
        pool_guard.remove(tab_id)
    };
    // close 是 UI 调用，放在锁外执行
    if let Some(webview) = webview {
        let _ = webview.close();
    }
}

#[tauri::command]
fn navigate_to_url(tab_id: i32, url: String, app: AppHandle) {
    let target = normalize_url_input(&url);
    debug_log!("[diag] navigate_to_url ENTER: tab_id={} url={} -> {}", tab_id, url, target);
    if target.is_empty() {
        return;
    }

    // 更新 tab 状态（URL + 历史栈）
    {
        let pool = app.state::<Arc<Mutex<WebViewPool>>>();
        let mut pool_guard = lock_pool(&pool);
        pool_guard.upsert_tab(tab_id, &target);
        // Linux: 确保 active_tab_id 同步更新，供工具栏取标签列表
        if cfg!(target_os = "linux") {
            pool_guard.set_active(tab_id);
        }
    }

    if cfg!(target_os = "linux") {
        // Linux: 直接导航主 webview
        debug_log!("[diag] navigate_to_url: Linux navigating main webview to: {}", target);
        navigate_main(&app, &target);
        return;
    }

    // macOS/Windows: 导航子 webview
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let has_wv = lock_pool(&pool).has_webview(tab_id);

    if !has_wv {
        debug_log!("[diag] navigate_to_url: webview not found, recreating");
        let (cx, cy, cw, ch) = {
            let g = lock_pool(&pool);
            (g.content_x, g.content_y, g.content_width, g.content_height)
        };
        let _ = create_tab(tab_id, target.clone(), cx, cy, cw, ch, app.clone());
    }

    // navigate 是 UI 调用，放在锁外执行
    let webview = { lock_pool(&pool).webviews.get(&tab_id).cloned() };
    if let Some(webview) = webview {
        debug_log!("[diag] navigate_to_url: found webview, navigating to: {}", target);
        if let Ok(parsed_url) = url::Url::parse(&target) {
            let result = webview.navigate(parsed_url);
            debug_log!("[diag] navigate_to_url: navigate result: {:?}", result);
        } else {
            debug_log!("[diag] navigate_to_url: invalid URL: {}", target);
        }
    } else {
        debug_log!("[diag] navigate_to_url: webview NOT FOUND after recreate attempt, tab_id={}", tab_id);
    }
}

#[tauri::command]
fn reload_tab(tab_id: i32, app: AppHandle) {
    debug_log!("[reload_tab] tab_id={}", tab_id);
    eval_tab_js(&app, tab_id, "window.location.reload();");
}

/// mac/win：webview 被 LRU 淘汰后，先按标签当前 URL 重建再执行历史操作
fn ensure_webview_macwin(tab_id: i32, app: &AppHandle) {
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let missing = {
        let g = lock_pool(&pool);
        !g.has_webview(tab_id)
    };
    if !missing {
        return;
    }
    let (url, cx, cy, cw, ch) = {
        let g = lock_pool(&pool);
        (
            g.tab_url(tab_id).unwrap_or_default(),
            g.content_x,
            g.content_y,
            g.content_width,
            g.content_height,
        )
    };
    if url.is_empty() || url == "about:blank" {
        return;
    }
    debug_log!("[diag] go_back/forward: rebuilding webview for tab_id={}", tab_id);
    let _ = create_tab(tab_id, url, cx, cy, cw, ch, app.clone());
}

#[tauri::command]
fn go_back_tab(tab_id: i32, app: AppHandle) {
    debug_log!("[go_back_tab] tab_id={}", tab_id);
    if cfg!(target_os = "linux") {
        // Linux: 历史由 Rust 按 tab 维护，导航到该 tab 的上一条 URL
        let pool = app.state::<Arc<Mutex<WebViewPool>>>();
        let (target, fallback) = {
            let mut g = lock_pool(&pool);
            (g.go_back(tab_id), g.tab_url(tab_id).unwrap_or_default())
        };
        let url = target.unwrap_or(fallback);
        if !url.is_empty() {
            navigate_main_replace(&app, &url);
        }
        return;
    }
    ensure_webview_macwin(tab_id, &app);
    eval_tab_js(&app, tab_id, "window.history.back();");
}

#[tauri::command]
fn go_forward_tab(tab_id: i32, app: AppHandle) {
    debug_log!("[go_forward_tab] tab_id={}", tab_id);
    if cfg!(target_os = "linux") {
        // Linux: 历史由 Rust 按 tab 维护，导航到该 tab 的下一条 URL
        let pool = app.state::<Arc<Mutex<WebViewPool>>>();
        let (target, fallback) = {
            let mut g = lock_pool(&pool);
            (g.go_forward(tab_id), g.tab_url(tab_id).unwrap_or_default())
        };
        let url = target.unwrap_or(fallback);
        if !url.is_empty() {
            navigate_main_replace(&app, &url);
        }
        return;
    }
    ensure_webview_macwin(tab_id, &app);
    eval_tab_js(&app, tab_id, "window.history.forward();");
}

#[tauri::command]
fn open_devtools_tab(tab_id: i32, app: AppHandle) {
    debug_log!("[open_devtools_tab] tab_id={}", tab_id);
    if cfg!(target_os = "linux") {
        if let Some(wv) = app.get_webview_window("main") {
            wv.open_devtools();
        }
        return;
    }
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    // 锁内只做状态切换与取句柄，devtools 的开关放在锁外
    let (open, webview) = {
        let mut pool_guard = lock_pool(&pool);
        let open = !pool_guard.devtools_open.contains(&tab_id);
        if open {
            pool_guard.devtools_open.insert(tab_id);
        } else {
            pool_guard.devtools_open.remove(&tab_id);
        }
        (open, pool_guard.webviews.get(&tab_id).cloned())
    };

    if let Some(webview) = webview {
        if open {
            debug_log!("[open_devtools_tab] opening devtools for tab_id={}", tab_id);
            webview.open_devtools();
        } else {
            debug_log!("[open_devtools_tab] closing devtools for tab_id={}", tab_id);
            webview.close_devtools();
        }
    }
}

#[tauri::command]
fn get_settings(app: AppHandle) -> SessionSettings {
    let settings = app.state::<Arc<Mutex<SessionSettings>>>();
    let settings_guard = lock_settings(&settings);
    debug_log!("[DIAG:get_settings] return: auto_clear_on_exit={}", settings_guard.auto_clear_on_exit);
    settings_guard.clone()
}

#[tauri::command]
fn set_settings(settings: SessionSettings, app: AppHandle) {
    debug_log!("[DIAG:set_settings] ENTER: auto_clear_on_exit={}", settings.auto_clear_on_exit);
    let s = app.state::<Arc<Mutex<SessionSettings>>>();
    let mut s_guard = lock_settings(&s);
    *s_guard = settings;
}

#[tauri::command]
fn clear_browsing_data(app: AppHandle) {
    // 清除所有活跃 webview 的 localStorage/sessionStorage/cookies
    if !cfg!(target_os = "linux") {
        let pool = app.state::<Arc<Mutex<WebViewPool>>>();
        let webviews: Vec<tauri::Webview> = lock_pool(&pool).webviews.values().cloned().collect();
        for wv in webviews {
            let _ = wv.eval(CLEAR_ALL_JS);
        }
    } else if let Some(wv) = app.get_webview_window("main") {
        let _ = wv.eval(CLEAR_ALL_JS);
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct SessionSettings {
    auto_clear_on_exit: bool,
}

/// 清除浏览器数据的 JS 脚本：localStorage + sessionStorage + cookies
const CLEAR_ALL_JS: &str = r#"(function(){try{localStorage.clear();sessionStorage.clear();document.cookie.split(';').forEach(function(c){var n=c.split('=')[0].trim();if(n)document.cookie=n+'=;expires=Thu, 01 Jan 1970 00:00:00 UTC;path=/';});}catch(e){}})();"#;

#[derive(Clone, serde::Serialize)]
struct PopupPayload {
    url: String,
}

/// 前端 ResizeObserver 检测到内容区域尺寸变化时调用
/// 直接使用 viewport 坐标更新嵌入 webview 的位置和尺寸
#[tauri::command]
fn resize_content_area(x: f64, y: f64, width: f64, height: f64, app: AppHandle) {
    debug_log!("[diag] resize_content_area ENTER: pos={}x{} size={}x{}", x, y, width, height);

    // Linux: no child webviews (main webview is used directly)
    if cfg!(target_os = "linux") {
        debug_log!("[diag] resize_content_area: Linux no-op (no child webviews)");
        return;
    }

    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let active_id;
    let hide_all;
    let hide_target;
    {
        let mut pool_guard = lock_pool(&pool);
        debug_log!("[diag] resize_content_area: active_tab_id={:?}", pool_guard.active_tab_id);

        // 零/负尺寸为隐藏信号：隐藏活跃 webview 但不更新存储的 content 尺寸
        if width <= 0.0 || height <= 0.0 {
            hide_target = pool_guard
                .active_tab_id
                .and_then(|id| pool_guard.webviews.get(&id).cloned());
            if hide_target.is_none() {
                debug_log!("[diag] resize_content_area HIDE: active_tab_id is None, NO WEBVIEW TO HIDE");
            }
            // UI 调用放在锁外：主线程持锁做 UI 调用会阻塞兜底线程与页面回调
            if let Some(wv) = hide_target {
                let _ = wv.hide();
            }
            return;
        }

        debug_log!("[diag] resize_content_area SHOW: updating content={}x{} size={}x{}", x, y, width, height);
        pool_guard.content_x = x;
        pool_guard.content_y = y;
        pool_guard.content_width = width;
        pool_guard.content_height = height;
        active_id = pool_guard.active_tab_id;
        // active 标签是空白页（无 webview）时，本次 resize 应隐藏全部子 webview：
        // 否则残留的旧网页会盖住 React 欢迎页并抢走键盘焦点
        hide_all = active_id.map(|id| !pool_guard.webviews.contains_key(&id)).unwrap_or(true);
    }

    // 锁内只取句柄，UI 调用全部在锁外执行
    let webviews: Vec<(i32, tauri::Webview)> = {
        let g = lock_pool(&pool);
        g.webviews.iter().map(|(id, wv)| (*id, wv.clone())).collect()
    };
    for (tab_id, webview) in webviews {
        if !hide_all && active_id == Some(tab_id) {
            debug_log!("[diag] resize_content_area SHOW: showing+pos active tab_id={}", tab_id);
            let _ = webview.show();
            let _ = webview.set_position(LogicalPosition::new(x, y));
            let _ = webview.set_size(LogicalSize::new(width, height));
            // 不在这里 set_focus：窗口缩放/开关设置面板都会触发 resize，
            // 抢焦点会把用户正在输入的地址栏打回网页
        } else {
            debug_log!("[diag] resize_content_area SHOW: hiding inactive tab_id={}", tab_id);
            let _ = webview.hide();
        }
    }
}

/// Linux 工具栏注入：把当前标签状态写进页面并重建工具栏
/// 页面加载完成时调用；load 事件不触发的页面由周期性兜底线程补注入
fn inject_toolbar(app: &AppHandle) {
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let (tab_list, active_id) = {
        let g = lock_pool(&pool);
        let list: Vec<TabInfo> = g
            .tab_order
            .iter()
            .filter_map(|&id| {
                g.tabs.get(&id).map(|t| TabInfo {
                    id,
                    url: t.url.clone(),
                    can_back: t.can_back(),
                    can_forward: t.can_forward(),
                })
            })
            .collect();
        (list, g.active_tab_id.unwrap_or(0))
    };
    let tabs_json = serde_json::to_string(&tab_list).unwrap_or_default();
    let app_urls = serde_json::to_string(&lock_pool(&pool).app_url_prefixes).unwrap_or_default();
    let settings = app.state::<Arc<Mutex<SessionSettings>>>();
    let settings_json = serde_json::to_string(&lock_settings(&settings).clone()).unwrap_or_default();

    debug_log!(
        "[diag] toolbar: INJECTING tabs_json={} active_id={}",
        tabs_json,
        active_id
    );
    let script = format!(
        r#";(function(){{try{{window.__MB_STATE__={{tabs:{},active:{},settings:{},app_urls:{}}};{}}}catch(e){{}}}})();"#,
        tabs_json,
        active_id,
        settings_json,
        app_urls,
        CTX_MENU_SCRIPT.to_owned() + TOOLBAR_SCRIPT
    );
    if let Some(wv) = app.get_webview_window("main") {
        let _ = wv.eval(&script);
    }
}

/// Linux 协议命令：把工具栏的操作转交给 Rust 处理
/// 命令通过 about:blank?__mb_xxx=yyy 传递（外部页面无法使用 IPC）
fn handle_linux_command(app: &AppHandle, key: &str, value: &str) {
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    debug_log!("[diag] toolbar: COMMAND {}={}", key, value);

    // 当前 tab 的 URL（协议跳转后需要把页面还回来）
    let active_url = || -> String {
        let g = lock_pool(&pool);
        g.active_tab_id
            .and_then(|id| g.tab_url(id))
            .unwrap_or_else(|| "about:blank".to_string())
    };

    // 各命令返回要跳转的目标 URL；None 表示「本次不改变页面」，
    // 由结尾统一把页面恢复到当前 active tab，避免命令页残留导致原页面丢失
    let target: Option<String> = match key {
        "__mb_nav" => {
            let target = normalize_url_input(value);
            if target.is_empty() {
                None
            } else {
                let tab_id = {
                    let mut g = lock_pool(&pool);
                    // active 指向已不存在的标签时（异常状态）重建，避免状态脱节
                    let id = match g.active_tab_id {
                        Some(id) if g.tabs.contains_key(&id) => id,
                        _ => {
                            let new_id = NEXT_TAB_ID.fetch_add(1, Ordering::Relaxed);
                            g.insert_tab_only(new_id, &target);
                            new_id
                        }
                    };
                    g.set_active(id);
                    g.record_navigation(id, &target);
                    id
                };
                debug_log!("[diag] toolbar: NAV tab_id={} target={}", tab_id, target);
                Some(target)
            }
        }
        "__mb_back" => {
            let (target, fallback) = {
                let mut g = lock_pool(&pool);
                let id = g.active_tab_id.unwrap_or(0);
                (g.go_back(id), g.tab_url(id).unwrap_or_default())
            };
            let url = target.unwrap_or(fallback);
            if url.is_empty() {
                None
            } else {
                Some(url)
            }
        }
        "__mb_forward" => {
            let (target, fallback) = {
                let mut g = lock_pool(&pool);
                let id = g.active_tab_id.unwrap_or(0);
                (g.go_forward(id), g.tab_url(id).unwrap_or_default())
            };
            let url = target.unwrap_or(fallback);
            if url.is_empty() {
                None
            } else {
                Some(url)
            }
        }
        "__mb_switch" => {
            let switch_id = value.parse::<i32>().unwrap_or(0);
            let mut out = None;
            {
                let mut g = lock_pool(&pool);
                // 目标标签必须存在：否则 active 会指向不存在的 id，状态与页面彻底脱节
                if !g.tabs.contains_key(&switch_id) {
                    debug_log!("[diag] toolbar: SWITCH unknown tab_id={}", switch_id);
                } else {
                    g.set_active(switch_id);
                    let url = g.tab_url(switch_id).unwrap_or_else(|| "about:blank".to_string());
                    debug_log!("[diag] toolbar: SWITCH tab_id={} url={}", switch_id, url);
                    out = Some(url);
                }
            }
            out
        }
        "__mb_close" => {
            let close_id = value.parse::<i32>().unwrap_or(0);
            let mut out = None;
            {
                let mut g = lock_pool(&pool);
                if g.tab_count() <= 1 {
                    debug_log!("[diag] toolbar: refusing to close last tab");
                } else if !g.tabs.contains_key(&close_id) {
                    debug_log!("[diag] toolbar: CLOSE unknown tab_id={}", close_id);
                } else {
                    let was_active = g.active_tab_id == Some(close_id);
                    let pos = g.tab_order.iter().position(|&id| id == close_id);
                    g.remove(close_id);
                    let url = if was_active {
                        // 关闭的是 active tab：内容区跟随新 active，避免页面与工具栏状态脱节
                        let next = match pos {
                            Some(p) if p < g.tab_order.len() => Some(g.tab_order[p]),
                            Some(p) if p > 0 => Some(g.tab_order[p - 1]),
                            _ => g.tab_order.back().copied(),
                        };
                        g.active_tab_id = next;
                        next.and_then(|id| g.tab_url(id))
                            .filter(|u| !u.is_empty() && u != "about:blank")
                            .unwrap_or_else(|| "about:blank".to_string())
                    } else {
                        // 非 active tab 关闭：页面保持在当前 active tab 的 URL
                        // 注意：这里必须直接读 g，不能再调 active_url()（其内部会再次取同一把锁，同线程重入自死锁）
                        g.active_tab_id
                            .and_then(|id| g.tab_url(id))
                            .unwrap_or_else(|| "about:blank".to_string())
                    };
                    debug_log!("[diag] toolbar: CLOSE tab_id={} -> {}", close_id, url);
                    out = Some(url);
                }
            }
            out
        }
        "__mb_new" => {
            let mut out = None;
            {
                let mut g = lock_pool(&pool);
                if g.tab_count() >= MAX_TABS {
                    debug_log!("[diag] toolbar: refuse new tab, limit reached");
                } else {
                    let new_id = NEXT_TAB_ID.fetch_add(1, Ordering::Relaxed);
                    g.insert_tab_only(new_id, "about:blank");
                    g.set_active(new_id);
                    debug_log!("[diag] toolbar: NEW tab");
                    out = Some("about:blank".to_string());
                }
            }
            out
        }
        "__mb_devtools" => {
            if let Some(wv) = app.get_webview_window("main") {
                wv.open_devtools();
            }
            None
        }
        "__mb_clear_cache" => {
            clear_browsing_data(app.clone());
            None
        }
        "__mb_autoclear" => {
            let settings = app.state::<Arc<Mutex<SessionSettings>>>();
            {
                let mut s = lock_settings(&settings);
                s.auto_clear_on_exit = !s.auto_clear_on_exit;
                debug_log!("[diag] toolbar: auto_clear_on_exit={}", s.auto_clear_on_exit);
            }
            None
        }
        _ => None,
    };

    match target {
        Some(url) => navigate_main_replace(app, &url),
        // 没有页面变化时也要把页面还回来：协议命令页（about:blank?__mb_*）已经替换了原页面
        None => {
            let url = active_url();
            navigate_main_replace(app, &url);
        }
    }
}

/// 弹窗（window.open）转应用内标签页
/// Linux 上 React 界面在首次导航后已不存在，由 Rust 直接建 tab 并导航主 webview
fn open_popup_tab(app: &AppHandle, url: &str) {
    let target = normalize_url_input(url);
    if target.is_empty() {
        return;
    }
    let pool = app.state::<Arc<Mutex<WebViewPool>>>();
    let tab_id = {
        let mut g = lock_pool(&pool);
        if g.tab_count() >= MAX_TABS {
            debug_log!("[popup] refuse new tab, limit reached");
            return;
        }
        let new_id = NEXT_TAB_ID.fetch_add(1, Ordering::Relaxed);
        g.insert_tab_only(new_id, &target);
        g.set_active(new_id);
        new_id
    };
    debug_log!("[popup] open tab_id={} url={}", tab_id, target);
    navigate_main_replace(app, &target);
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
            get_settings,
            set_settings,
            clear_browsing_data,
        ])
        .plugin(
            tauri::plugin::Builder::<tauri::Wry>::new("toolbar")
                .on_page_load(move |webview, payload| {
                    debug_log!("[diag] toolbar: PLUGIN on_page_load event={:?} url={}", payload.event(), payload.url());
                    if !cfg!(target_os = "linux") {
                        return;
                    }
                    let url = payload.url().as_str().to_string();
                    let app = webview.app_handle().clone();
                    let is_app_url = {
                        let pool = app.state::<Arc<Mutex<WebViewPool>>>();
                        let prefixes = lock_pool(&pool).app_url_prefixes.clone();
                        prefixes.iter().any(|p| url.starts_with(p))
                    };

                    // 记录最近页面 URL：部分站点 load 事件不触发时，靠它定位当前页面做兜底注入
                    // 协议跳板（about:blank?__mb_*）不是真实页面，排除掉
                    if !is_app_url && parse_command(&url).is_none() {
                        let pool = app.state::<Arc<Mutex<WebViewPool>>>();
                        lock_pool(&pool).last_page_url = url.clone();
                    }

                    // 协议跳板不是真实页面，不进历史
                    if parse_command(&url).is_some() {
                        if matches!(payload.event(), PageLoadEvent::Finished) {
                            // 协议命令：处理完由 Rust 决定跳转目标，页面不再注入工具栏
                            if let Some((key, value)) = parse_command(&url) {
                                handle_linux_command(&app, &key, &value);
                            }
                        }
                        return;
                    }
                    if is_app_url {
                        if matches!(payload.event(), PageLoadEvent::Finished) {
                            debug_log!("[diag] toolbar: SKIP (app url) url={}", url);
                        }
                        return;
                    }

                    match payload.event() {
                        PageLoadEvent::Started => {
                            // 导航开始：写入临时历史条目（最终 URL 可能因重定向而不同）
                            let pool = app.state::<Arc<Mutex<WebViewPool>>>();
                            let mut pool_guard = lock_pool(&pool);
                            // 已有归属且 URL 一致 → 保持原归属（Rust 侧发起时已登记）
                            let owner = match &pool_guard.pending_nav {
                                Some((_, owner_url)) if urls_equivalent(owner_url, &url) => {
                                    pool_guard.pending_nav.clone()
                                }
                                _ => pool_guard.active_tab_id.map(|id| (id, url.clone())),
                            };
                            if let Some((tab_id, _)) = owner {
                                pool_guard.pending_nav = Some((tab_id, url.clone()));
                                pool_guard.begin_navigation(tab_id, &url);
                            }
                        }
                        PageLoadEvent::Finished => {
                            // 导航完成：用最终 URL 修正临时条目
                            {
                                let pool = app.state::<Arc<Mutex<WebViewPool>>>();
                                let mut pool_guard = lock_pool(&pool);
                                // 归属标签：优先用登记的导航归属，避免标签切换后记到别的标签上
                                let owner = pool_guard
                                    .pending_nav
                                    .as_ref()
                                    .map(|(id, owner_url)| {
                                        if urls_equivalent(owner_url, &url) {
                                            Some(*id)
                                        } else {
                                            pool_guard.active_tab_id
                                        }
                                    })
                                    .unwrap_or(pool_guard.active_tab_id);
                                pool_guard.pending_nav = None;
                                if url.starts_with("about:blank") {
                                    // 欢迎页：不记入历史，保持 tab 当前 URL
                                } else if let Some(tab_id) = owner {
                                    pool_guard.finish_navigation(tab_id, &url);
                                    let _ = app.emit(
                                        "browser://url-changed",
                                        UrlPayload { tab_id, url: url.clone() },
                                    );
                                }
                            }
                            inject_toolbar(&app);
                        }
                    }
                })
                .build()
        )
        .setup(|app| {
            debug_log!("[setup] initializing mini browser with native decorations");

            // 主窗口由 Rust 创建：需要设置伪装 UA，并拦截 window.open 转成应用内标签页
            let window_config = app
                .config()
                .app
                .windows
                .first()
                .cloned()
                .ok_or("主窗口配置缺失")?;
            let popup_app = app.handle().clone();
            let window = WebviewWindowBuilder::from_config(app.handle(), &window_config)?
                .user_agent(user_agent())
                .on_new_window(move |url, _features| {
                    let url_str = url.to_string();
                    debug_log!("[on_new_window] url={}", url_str);
                    if cfg!(target_os = "linux") {
                        // Linux: React 界面此时已不在，直接由 Rust 建 tab
                        open_popup_tab(&popup_app, &url_str);
                    } else {
                        let _ = popup_app.emit("popup://request", PopupPayload { url: url_str });
                    }
                    NewWindowResponse::Deny
                })
                .build()?;

            let close_app = app.handle().clone();
            window.on_window_event(move |event| {
                if let WindowEvent::CloseRequested { .. } = event {
                    debug_log!("[on_window_event::CloseRequested] cleaning up");

                    // 检查 auto_clear_on_exit 设置
                    let settings = close_app.state::<Arc<Mutex<SessionSettings>>>();
                    if lock_settings(&settings).auto_clear_on_exit {
                        debug_log!("[on_window_event::CloseRequested] auto_clear_on_exit enabled, clearing browsing data");
                        if cfg!(target_os = "linux") {
                            if let Some(wv) = close_app.get_webview_window("main") {
                                let _ = wv.eval(CLEAR_ALL_JS);
                            }
                        } else {
                            let pool = close_app.state::<Arc<Mutex<WebViewPool>>>();
                            let webviews: Vec<tauri::Webview> =
                                lock_pool(&pool).webviews.values().cloned().collect();
                            for w in webviews {
                                let _ = w.eval(CLEAR_ALL_JS);
                            }
                        }
                    }

                    // 关闭所有子 webview
                    let pool = close_app.state::<Arc<Mutex<WebViewPool>>>();
                    let webviews: Vec<tauri::Webview> =
                        lock_pool(&pool).webviews.values().cloned().collect();
                    for w in webviews {
                        let _ = w.close();
                    }
                }
            });

            // 应用自身页面的 URL 前缀：dev server 与自定义协议
            {
                let mut prefixes = vec![
                    "tauri://localhost".to_string(),
                    "http://tauri.localhost".to_string(),
                    "https://tauri.localhost".to_string(),
                ];
                if let Some(dev_url) = &app.config().build.dev_url {
                    prefixes.push(dev_url.to_string());
                }
                let pool = app.state::<Arc<Mutex<WebViewPool>>>();
                lock_pool(&pool).app_url_prefixes = prefixes;
            }

            // 兜底线程：部分站点在 WebKitGTK 下 load 事件不触发 Finished，
            // 工具栏就永远不会被注入（页面无地址栏/标签栏）。这里周期性补注入。
            let timer_app = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let pool = timer_app.state::<Arc<Mutex<WebViewPool>>>();
                let (url, prefixes) = {
                    let g = lock_pool(&pool);
                    (g.last_page_url.clone(), g.app_url_prefixes.clone())
                };
                if url.is_empty() || prefixes.iter().any(|p| url.starts_with(p)) {
                    continue;
                }
                // 只做工具栏兜底注入：历史由 Started/Finished 事件按 tab 记录，
                // 这里不能用全局 last_page_url 补记（切标签后会把别的标签的 URL 记进当前标签）
                inject_toolbar(&timer_app);
            });

            debug_log!("[setup] done");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error");
}

#[cfg(test)]
mod tests {
    use super::{
        js_string_literal, normalize_url_input, parse_command, urls_equivalent, TabState,
        QUERY_ENCODE_SET,
    };

    /// 对照组：旧的 replace('\'', "\\'") 转义不处理反斜杠，
    /// `http://x/\';alert(1);//` 会生成 `\\'`，JS 解析时先还原 `\\` 再闭合 `'` 导致注入。
    #[test]
    fn js_string_literal_blocks_quote_escape_attack() {
        let attack = "http://x/\\';alert(1);//";
        let lit = js_string_literal(attack);
        assert_eq!(lit, "\"http://x/\\\\';alert(1);//\"");
        // 字面量内不存在可提前闭合字符串的裸引号
        assert_eq!(lit.matches('"').count(), 2);
    }

    #[test]
    fn js_string_literal_keeps_plain_url() {
        assert_eq!(
            js_string_literal("https://example.com/a?q=1&x=2"),
            "\"https://example.com/a?q=1&x=2\""
        );
    }

    #[test]
    fn normalize_keeps_explicit_scheme() {
        assert_eq!(normalize_url_input("https://example.com"), "https://example.com");
        assert_eq!(normalize_url_input("http://example.com"), "http://example.com");
        assert_eq!(normalize_url_input("about:blank"), "about:blank");
    }

    #[test]
    fn normalize_adds_scheme_to_host() {
        assert_eq!(normalize_url_input("example.com"), "https://example.com");
        assert_eq!(
            normalize_url_input("example.com/path"),
            "https://example.com/path"
        );
    }

    #[test]
    fn normalize_uses_http_for_local_hosts() {
        assert_eq!(normalize_url_input("localhost:3000"), "http://localhost:3000");
        assert_eq!(normalize_url_input("127.0.0.1:8080"), "http://127.0.0.1:8080");
        assert_eq!(normalize_url_input("192.168.1.10"), "http://192.168.1.10");
    }

    #[test]
    fn normalize_treats_text_with_spaces_as_search() {
        assert_eq!(
            normalize_url_input("how to install node.js"),
            "https://www.google.com/search?q=how%20to%20install%20node.js"
        );
        assert_eq!(
            normalize_url_input("安装 教程"),
            "https://www.google.com/search?q=%E5%AE%89%E8%A3%85%20%E6%95%99%E7%A8%8B"
        );
    }

    #[test]
    fn normalize_empty_input() {
        assert_eq!(normalize_url_input("   "), "");
    }

    #[test]
    fn parse_command_decodes_value() {
        let (k, v) = parse_command("about:blank?__mb_nav=hello%20world").unwrap();
        assert_eq!(k, "__mb_nav");
        assert_eq!(v, "hello world");
    }

    #[test]
    fn parse_command_roundtrip_utf8_and_specials() {
        // 模拟工具栏 encodeURIComponent → Rust percent_decode 的往返
        let raw = "中文 空格 & ? = #";
        let encoded = percent_encoding::utf8_percent_encode(raw, QUERY_ENCODE_SET).to_string();
        let url = format!("about:blank?__mb_nav={}", encoded);
        let (k, v) = parse_command(&url).unwrap();
        assert_eq!(k, "__mb_nav");
        assert_eq!(v, raw);
    }

    #[test]
    fn parse_command_keeps_url_query_intact() {
        // & 与 = 必须原样保留（切分发生在解码之前）
        let url = "about:blank?__mb_nav=httpbin.org%2Fget%3Fa%3D1%26b%3D2";
        let (_, v) = parse_command(url).unwrap();
        assert_eq!(v, "httpbin.org/get?a=1&b=2");
        assert_eq!(
            normalize_url_input(&v),
            "https://httpbin.org/get?a=1&b=2"
        );
    }

    #[test]
    fn parse_command_ignores_non_command_url() {
        assert!(parse_command("https://example.com/?a=1").is_none());
        assert!(parse_command("https://example.com/?__mb_close=1").is_none());
    }

    #[test]
    fn urls_equivalent_ignores_trailing_slash_and_fragment() {
        assert!(urls_equivalent("https://example.com/", "https://example.com"));
        assert!(urls_equivalent("https://example.com/a#x", "https://example.com/a"));
        assert!(!urls_equivalent("http://example.com", "https://example.com"));
    }

    #[test]
    fn tab_history_back_and_forward() {
        let mut tab = TabState::new("https://a.com");
        tab.record("https://b.com");
        tab.record("https://c.com");
        assert!(tab.can_back());
        assert!(!tab.can_forward());
        assert_eq!(tab.go_back().unwrap(), "https://b.com");
        assert!(tab.can_forward());
        assert_eq!(tab.go_forward().unwrap(), "https://c.com");
        assert!(!tab.can_forward());
    }

    #[test]
    fn tab_history_duplicate_navigation_does_not_grow_stack() {
        let mut tab = TabState::new("https://a.com");
        tab.record("https://a.com/");
        assert_eq!(tab.history.len(), 1);
        assert!(!tab.can_back());
    }

    #[test]
    fn tab_history_finalize_replaces_provisional_entry() {
        // 重定向场景：Started 写入 http://a，Finished 是 https://a → 只保留一条
        let mut tab = TabState::new("http://a");
        tab.record_provisional("http://a");
        tab.finalize("https://a");
        assert_eq!(tab.history, vec!["https://a"]);
        assert_eq!(tab.history_index, 0);
        assert!(!tab.can_back());
    }

    #[test]
    fn tab_history_finalize_ignores_stale_event() {
        // 没有匹配的临时条目时，迟到的 Finished 不得改写历史（否则会覆盖当前条目）
        let mut tab = TabState::new("https://a");
        tab.record("https://a");
        tab.finalize("https://b");
        assert_eq!(tab.history, vec!["https://a"]);
        assert!(!tab.can_back());
    }

    #[test]
    fn tab_history_finalize_replaces_only_own_entry() {
        // 临时条目属于下标 1，但游标已回退到 0 → 迟到的 Finished 不能改写下标 0
        let mut tab = TabState::new("https://a");
        tab.record_provisional("https://b");
        tab.go_back();
        tab.finalize("https://b");
        assert_eq!(tab.history, vec!["https://a", "https://b"]);
        assert_eq!(tab.history_index, 0);
    }

    #[test]
    fn tab_history_truncates_forward_on_new_navigation() {
        let mut tab = TabState::new("https://a.com");
        tab.record("https://b.com");
        tab.record("https://c.com");
        tab.go_back();
        tab.record("https://d.com");
        assert_eq!(
            tab.history,
            vec!["https://a.com", "https://b.com", "https://d.com"]
        );
        assert!(!tab.can_forward());
    }
}

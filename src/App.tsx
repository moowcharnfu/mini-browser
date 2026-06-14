import { useState, useCallback, useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';

const appWindow = getCurrentWindow();
const isLinux = navigator.userAgent.includes('Linux');

const TABBAR_HEIGHT = 36;
const TOOLBAR_HEIGHT = 44;

interface Tab {
    id: number;
    title: string;
    url: string;
    history: string[];
    historyIndex: number;
    addressInput: string;
}

function newTabId(tabs: Tab[]): number {
    return Math.max(0, ...tabs.map((t) => t.id)) + 1;
}

function defaultTab(id: number): Tab {
    return { id, title: '新标签页', url: 'about:blank', history: ['about:blank'], historyIndex: 0, addressInput: '' };
}

export default function App() {
    const [tabs, setTabs] = useState<Tab[]>([]);
    const [activeTabId, setActiveTabId] = useState(0);
    const [loading, setLoading] = useState(false);

    const activeTab = tabs.find((t) => t.id === activeTabId);

    const tabsRef = useRef(tabs);
    tabsRef.current = tabs;
    const activeTabIdRef = useRef(activeTabId);
    activeTabIdRef.current = activeTabId;
    const contentRef = useRef<HTMLDivElement>(null);
    const initRef = useRef(false);

    // Listen for URL changes from content webviews
    useEffect(() => {
        const unlisten = listen<{ tab_id: number; url: string }>('browser://url-changed', (e) => {
            const { tab_id, url } = e.payload;
            setTabs((prev) =>
                prev.map((t) =>
                    t.id === tab_id
                        ? {
                              ...t,
                              url,
                              title: url === 'about:blank' ? '新标签页' : url,
                              history: t.history[t.historyIndex] === url
                                  ? t.history
                                  : [...t.history.slice(0, t.historyIndex + 1), url],
                              historyIndex: t.history[t.historyIndex] === url
                                  ? t.historyIndex
                                  : t.historyIndex + 1,
                          }
                        : t,
                ),
            );
        });
        return () => { unlisten.then((f) => f()); };
    }, []);

    // Listen for loading state
    useEffect(() => {
        const unlisten = listen<{ tab_id: number; loading: boolean }>('browser://loading', (e) => {
            const { tab_id, loading: isLoading } = e.payload;
            if (tab_id === activeTabIdRef.current) {
                setLoading(isLoading);
            }
        });
        return () => { unlisten.then((f) => f()); };
    }, []);

    // ResizeObserver: sync content area position to Rust
    useEffect(() => {
        const el = contentRef.current;
        if (!el) return;

        const sendContentSize = async () => {
            // Linux: 没有子 webview，无需发送 resize
            if (isLinux) return;
            const activeTab = tabsRef.current.find((t) => t.id === activeTabIdRef.current);
            // about:blank 时不发送 resize（欢迎页和 webview 互斥）
            if (activeTab && activeTab.url === 'about:blank') {
                console.log('[diag] sendContentSize: about:blank, skip');
                return;
            }
            const rect = el.getBoundingClientRect();
            console.log('[diag] sendContentSize: sending resize', { x: rect.left, y: rect.top, w: rect.width, h: rect.height });
            invoke('resize_content_area', {
                x: Math.round(rect.left),
                y: Math.round(rect.top),
                width: Math.round(rect.width),
                height: Math.round(rect.height),
            }).catch(console.error);
        };

        sendContentSize().catch(console.error);

        const observer = new ResizeObserver(() => {
            sendContentSize().catch(console.error);
            if (initRef.current) return;
            initRef.current = true;

            // 首次创建初始 tab — 不创建 webview，仅设置 React 状态
            // webview 在用户首次导航到真实 URL 时由 ensureWebview 创建
            requestAnimationFrame(() => {
                const id = newTabId([]);
                console.log('[diag] init: setting state for tab', { id });
                setTabs([defaultTab(id)]);
                setActiveTabId(id);
            });
        });
        observer.observe(el);
        return () => observer.disconnect();
    }, []);

    // 创建 tab 或重建被 LRU 淘汰的 WebView
    // about:blank 时不创建 webview（欢迎页和 webview 互斥）
    const ensureWebview = useCallback(async (tabId: number): Promise<boolean> => {
        // Linux 没有子 webview，所有页面在主 webview 中加载
        if (isLinux) return false;
        const tab = tabsRef.current.find((t) => t.id === tabId);
        if (tab && tab.url === 'about:blank') {
            console.log('[diag] ensureWebview: about:blank, skip webview creation');
            return false;
        }
        const result = await invoke<{ tab_id: number; needs_recreate: boolean }>('activate_tab', { activeTabId: tabId });
        if (result.needs_recreate) {
            const el = contentRef.current;
            const rect = el?.getBoundingClientRect();
            await invoke('create_tab', {
                tabId,
                url: tab?.url ?? 'about:blank',
                x: Math.round(rect?.left ?? 0),
                y: Math.round(rect?.top ?? 0),
                width: Math.round(rect?.width ?? 1200),
                height: Math.round(rect?.height ?? 800),
            });
            await invoke('activate_tab', { activeTabId: tabId });
            return true;
        }
        return false;
    }, []);

    const navigate = useCallback(async (url?: string) => {
        const tab = tabsRef.current.find((t) => t.id === activeTabIdRef.current);
        const raw = url ?? tab?.addressInput.trim();
        if (!raw || !tab) return;

        let target = raw;
        if (!raw.match(/^https?:\/\//)) {
            target = raw.includes('.')
                ? 'https://' + raw
                : 'https://www.google.com/search?q=' + encodeURIComponent(raw);
        }

        setTabs((prev) =>
            prev.map((t) =>
                t.id === activeTabIdRef.current ? { ...t, addressInput: target } : t,
            ),
        );
        setLoading(true);
        // Linux: 主 webview 直接导航，无需 ensureWebview
        if (!isLinux) {
            await ensureWebview(activeTabIdRef.current);
        }
        await invoke('navigate_to_url', { tabId: activeTabIdRef.current, url: target });
    }, []);

    const reload = useCallback(async () => {
        setLoading(true);
        await invoke('reload_tab', { tabId: activeTabIdRef.current });
    }, []);

    const goBack = useCallback(async () => {
        const tab = tabsRef.current.find((t) => t.id === activeTabIdRef.current);
        if (tab && tab.historyIndex > 0) {
            await invoke('go_back_tab', { tabId: activeTabIdRef.current });
            setTabs((prev) =>
                prev.map((t) =>
                    t.id === activeTabIdRef.current ? { ...t, historyIndex: t.historyIndex - 1 } : t,
                ),
            );
        }
    }, []);

    const goForward = useCallback(async () => {
        const tab = tabsRef.current.find((t) => t.id === activeTabIdRef.current);
        if (tab && tab.historyIndex < tab.history.length - 1) {
            await invoke('go_forward_tab', { tabId: activeTabIdRef.current });
            setTabs((prev) =>
                prev.map((t) =>
                    t.id === activeTabIdRef.current ? { ...t, historyIndex: t.historyIndex + 1 } : t,
                ),
            );
        }
    }, []);

    const newTab = useCallback(async () => {
        const id = newTabId(tabsRef.current);
        // about:blank 时不创建 webview（欢迎页和 webview 互斥）
        setTabs((prev) => [...prev, defaultTab(id)]);
        setActiveTabId(id);
        setLoading(false);
    }, []);

    const closeTab = useCallback(async (id: number) => {
        const currentTabs = tabsRef.current;
        if (currentTabs.length <= 1) return;
        const idx = currentTabs.findIndex((t) => t.id === id);
        await invoke('close_tab', { tabId: id });
        setTabs((prev) => prev.filter((t) => t.id !== id));
        if (id === activeTabIdRef.current) {
            const next = idx > 0 ? currentTabs[idx - 1] : currentTabs[idx + 1];
            if (next) {
                setActiveTabId(next.id);
                setLoading(next.url !== 'about:blank');
                if (isLinux) {
                    if (next.url !== 'about:blank') {
                        await invoke('navigate_to_url', { tabId: next.id, url: next.url });
                    } else if (window.location.href !== 'about:blank') {
                        window.location.href = 'about:blank';
                    }
                } else {
                    await ensureWebview(next.id);
                }
            }
        }
    }, [ensureWebview]);

    const switchTab = useCallback(async (id: number, url?: string) => {
        setActiveTabId(id);
        if (url !== undefined) {
            setLoading(url !== 'about:blank');
        }
        if (isLinux) {
            if (url && url !== 'about:blank') {
                await invoke('navigate_to_url', { tabId: id, url });
            }
        } else {
            await ensureWebview(id);
        }
    }, [ensureWebview]);

    const handleAddressInputChange = useCallback((e: React.ChangeEvent<HTMLInputElement>) => {
        const id = activeTabIdRef.current;
        setTabs((prev) =>
            prev.map((t) => (t.id === id ? { ...t, addressInput: e.target.value } : t)),
        );
    }, []);

    const openDevtools = useCallback(() => {
        invoke('open_devtools_tab', { tabId: activeTabIdRef.current });
    }, []);

    const clearAddress = useCallback(() => {
        const id = activeTabIdRef.current;
        setTabs((prev) =>
            prev.map((t) => (t.id === id ? { ...t, addressInput: '' } : t)),
        );
    }, []);

    // 监听主窗口尺寸变化，检测从最小化恢复
    useEffect(() => {
        if (isLinux) return; // Linux 无需处理子 webview 恢复
        const unlisten = appWindow.onResized(async () => {
            const minimized = await appWindow.isMinimized();
            if (!minimized && activeTabIdRef.current) {
                // about:blank 时不恢复 webview（欢迎页和 webview 互斥）
                const tab = tabsRef.current.find((t) => t.id === activeTabIdRef.current);
                if (tab && tab.url === 'about:blank') return;
                const el = contentRef.current;
                if (!el) return;
                const rect = el.getBoundingClientRect();
                invoke('resize_content_area', {
                    x: Math.round(rect.left),
                    y: Math.round(rect.top),
                    width: Math.round(rect.width),
                    height: Math.round(rect.height),
                }).catch(console.error);
                // 恢复时确保 active tab 显示
                const id = activeTabIdRef.current;
                if (id) {
                    invoke('activate_tab', { activeTabId: id }).catch(console.error);
                }
            }
        });
        return () => { unlisten.then((f) => f()); };
    }, []);

    // URL 变化时：about:blank → 隐藏 webview；真实 URL → 恢复 webview 显示
    useEffect(() => {
        if (!activeTab) {
            console.log('[diag] urlEffect: activeTab is undefined');
            return;
        }
        // Linux: 没有子 webview，无需 resize/ensureWebview
        if (isLinux) return;
        const el = contentRef.current;
        if (!el) return;
        const rect = el.getBoundingClientRect();
        const tabId = activeTab.id;
        console.log('[diag] urlEffect: url=' + activeTab.url + ' rect=' + JSON.stringify({ x: Math.round(rect.left), y: Math.round(rect.top), w: Math.round(rect.width), h: Math.round(rect.height) }));
        if (activeTab.url === 'about:blank') {
            console.log('[diag] urlEffect: sending resize(0,0) for about:blank');
            invoke('resize_content_area', {
                x: Math.round(rect.left),
                y: Math.round(rect.top),
                width: 0,
                height: 0,
            }).catch(console.error);
        } else {
            console.log('[diag] urlEffect: ensuring webview + sending full resize for real URL');
            ensureWebview(tabId).then(() => {
                const r = contentRef.current?.getBoundingClientRect();
                const rx = Math.round(r?.left ?? rect.left);
                const ry = Math.round(r?.top ?? rect.top);
                const rw = Math.round(r?.width ?? rect.width);
                const rh = Math.round(r?.height ?? rect.height);
                return invoke('resize_content_area', {
                    x: rx, y: ry, width: rw, height: rh,
                });
            }).catch(console.error);
        }
    }, [activeTab?.url, ensureWebview]);

    const handleAddressKeyDown = useCallback((e: React.KeyboardEvent) => {
        if (e.key === 'Enter') navigate();
    }, []);

    const btnStyle: React.CSSProperties = {
        width: 28,
        height: 28,
        border: 'none',
        background: 'transparent',
        color: '#a0a0c0',
        borderRadius: '50%',
        cursor: 'pointer',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
    };
    const navBtn: React.CSSProperties = { ...btnStyle, borderRadius: 6 };

    return (
        <div
            style={{
                display: 'flex',
                flexDirection: 'column',
                height: '100vh',
                background: '#0f0f1a',
                color: '#e8e8f0',
                fontFamily: 'system-ui, sans-serif',
                userSelect: 'none',
            }}
        >
            {/* 标签栏 */}
            <div
                style={{
                    display: 'flex',
                    alignItems: 'center',
                    height: TABBAR_HEIGHT,
                    background: '#1a1a2e',
                    borderBottom: '1px solid #2d2d4a',
                    flexShrink: 0,
                }}
            >
                <div
                    style={{
                        display: 'flex',
                        flex: 1,
                        overflowX: 'auto',
                        padding: '0 8px',
                        gap: 2,
                    }}
                >
                    {tabs.map((tab) => (
                        <div
                            key={tab.id}
                            onClick={() => switchTab(tab.id, tab.url)}
                            style={{
                                display: 'flex',
                                alignItems: 'center',
                                gap: 4,
                                padding: '4px 8px',
                                background: tab.id === activeTabId ? '#2d2d52' : 'transparent',
                                borderRadius: 6,
                                cursor: 'pointer',
                                minWidth: 100,
                                maxWidth: 180,
                                height: 28,
                            }}
                        >
                            <span style={{ fontSize: 12, flexShrink: 0 }}>🌐</span>
                            <span
                                style={{
                                    fontSize: 12,
                                    flex: 1,
                                    overflow: 'hidden',
                                    textOverflow: 'ellipsis',
                                    whiteSpace: 'nowrap',
                                }}
                            >
                                {tab.title}
                            </span>
                            <button
                                onClick={(e) => {
                                    e.stopPropagation();
                                    closeTab(tab.id);
                                }}
                                style={{
                                    width: 18,
                                    height: 18,
                                    border: 'none',
                                    background: 'transparent',
                                    color: '#606080',
                                    borderRadius: 4,
                                    cursor: 'pointer',
                                    fontSize: 12,
                                    flexShrink: 0,
                                    display: 'flex',
                                    alignItems: 'center',
                                    justifyContent: 'center',
                                }}
                            >
                                ×
                            </button>
                        </div>
                    ))}
                </div>
                <button
                    onClick={newTab}
                    style={{
                        width: 26,
                        height: 26,
                        border: 'none',
                        background: 'transparent',
                        color: '#a0a0c0',
                        borderRadius: 6,
                        cursor: 'pointer',
                        flexShrink: 0,
                        marginRight: 4,
                        fontSize: 16,
                        display: 'flex',
                        alignItems: 'center',
                        justifyContent: 'center',
                    }}
                >
                    +
                </button>
            </div>

            {/* 工具栏（内含地址栏进度条） */}
            <div
                style={{
                    display: 'flex',
                    alignItems: 'center',
                    gap: 6,
                    padding: '6px 10px',
                    background: '#1a1a2e',
                    borderBottom: '1px solid #2d2d4a',
                    flexShrink: 0,
                    height: TOOLBAR_HEIGHT,
                    marginBottom: 20,
                }}
            >
                <button onClick={goBack} style={{ ...navBtn, opacity: activeTab && activeTab.historyIndex > 0 ? 1 : 0.3 }}>
                    ←
                </button>
                <button onClick={goForward} style={{ ...navBtn, opacity: activeTab && activeTab.historyIndex < activeTab.history.length - 1 ? 1 : 0.3 }}>
                    →
                </button>
                <button onClick={reload} style={navBtn}>
                    ↻
                </button>
                <button onClick={openDevtools} style={navBtn}>
                    🔧
                </button>

                <div
                    style={{
                        display: 'flex',
                        flex: 1,
                        alignItems: 'center',
                        gap: 6,
                        height: 32,
                        background: '#0f0f1a',
                        border: '1.5px solid #2d2d4a',
                        borderRadius: 8,
                        padding: '0 10px',
                        position: 'relative',
                        overflow: 'hidden',
                    }}
                >
                    <span style={{ color: '#606080', fontSize: 13 }}>🔒</span>
                    <input
                        value={activeTab?.addressInput ?? ''}
                        onChange={handleAddressInputChange}
                        onKeyDown={handleAddressKeyDown}
                        placeholder="输入网址或搜索..."
                        style={{
                            flex: 1,
                            border: 'none',
                            background: 'transparent',
                            color: '#e8e8f0',
                            fontSize: 13,
                            outline: 'none',
                        }}
                    />
                    <button
                        onClick={navigate}
                        style={{
                            width: 28,
                            height: 28,
                            border: 'none',
                            background: '#6366f1',
                            color: '#fff',
                            borderRadius: 6,
                            cursor: 'pointer',
                            fontSize: 14,
                            display: 'flex',
                            alignItems: 'center',
                            justifyContent: 'center',
                        }}
                    >
                        →
                    </button>
                    {activeTab?.addressInput && (
                        <button
                            onClick={clearAddress}
                            style={{
                                width: 18,
                                height: 18,
                                border: 'none',
                                background: '#252542',
                                color: '#606080',
                                borderRadius: '50%',
                                cursor: 'pointer',
                                fontSize: 12,
                                display: 'flex',
                                alignItems: 'center',
                                justifyContent: 'center',
                            }}
                        >
                            ×
                        </button>
                    )}
                    {loading && (
                        <div
                            style={{
                                position: 'absolute',
                                bottom: 0,
                                left: 0,
                                height: 2,
                                background: 'linear-gradient(90deg, #6366f1 0%, #818cf8 50%, #6366f1 100%)',
                                backgroundSize: '200% 100%',
                                width: '100%',
                                animation: 'progress-slide 1.2s ease-in-out infinite',
                            }}
                        />
                    )}
                </div>
            </div>

            {/* 内容区域 — 嵌入的 webview 会精确覆盖此区域 */}
            <div
                ref={contentRef}
                style={{
                    flex: 1,
                    marginTop: 10,
                    marginLeft: 10,
                    marginRight: 10,
                    marginBottom: 10,
                    border: activeTab?.url === 'about:blank' ? '1px solid #2d2d4a' : 'none',
                    borderRadius: activeTab?.url === 'about:blank' ? 8 : 0,
                    background: activeTab?.url === 'about:blank' ? '#0f0f1a' : 'transparent',
                }}
            >
                {(!activeTab || activeTab.url === 'about:blank') && !loading && (
                    <div style={{
                        display: 'flex',
                        alignItems: 'center',
                        justifyContent: 'center',
                        height: '100%',
                    }}>
                        <div style={{ textAlign: 'center' }}>
                            <div style={{ fontSize: 48, opacity: 0.4, marginBottom: 16 }}>
                                🌐
                            </div>
                            <h2 style={{ fontSize: 18, fontWeight: 600, marginBottom: 8 }}>
                                迷你浏览器
                            </h2>
                            <p>输入网址开始浏览</p>
                        </div>
                    </div>
                )}
            </div>
        </div>
    );
}
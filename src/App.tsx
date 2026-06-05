import { useState, useCallback, useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';

const appWindow = getCurrentWindow();

const TITLEBAR_HEIGHT = 28;
const TABBAR_HEIGHT = 36;
const TOOLBAR_HEIGHT = 44;
const STATUSBAR_HEIGHT = 24;

interface Tab {
    id: number;
    title: string;
    url: string;
    history: string[];
    historyIndex: number;
    addressInput: string; // 每个 tab 独立的地址栏输入
}

function newTabId(tabs: Tab[]): number {
    return Math.max(0, ...tabs.map((t) => t.id)) + 1;
}

function defaultTab(id: number): Tab {
    return { id, title: '新标签页', url: 'about:blank', history: ['about:blank'], historyIndex: 0, addressInput: '' };
}

export default function App() {
    const [tabs, setTabs] = useState<Tab[]>([defaultTab(1)]);
    const [activeTabId, setActiveTabId] = useState(1);
    const [loading, setLoading] = useState(false);

    const activeTab = tabs.find((t) => t.id === activeTabId);

    // 使用 ref 持有最新状态，避免回调依赖重建
    const tabsRef = useRef(tabs);
    tabsRef.current = tabs;
    const activeTabIdRef = useRef(activeTabId);
    activeTabIdRef.current = activeTabId;

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
        try {
            // 创建新 WebView
            await invoke('create_tab', { tabId: id, url: 'about:blank' });
            // 添加到 React 状态
            setTabs((prev) => [...prev, defaultTab(id)]);
            setActiveTabId(id);
            setLoading(false);
            // 激活新 tab：隐藏其他 WebView，显示新 WebView
            await invoke('activate_tab', { activeTabId: id });
        } catch (err) {
            console.error('[newTab] failed:', err);
            alert(`无法创建新标签：${err}`);
        }
    }, []);

    const closeTab = useCallback(async (id: number) => {
        const tabs = tabsRef.current;
        if (tabs.length <= 1) return;
        const idx = tabs.findIndex((t) => t.id === id);
        // 先关闭 WebView
        await invoke('close_tab', { tabId: id });
        // 从 React 状态移除
        setTabs((prev) => prev.filter((t) => t.id !== id));
        if (id === activeTabIdRef.current) {
            const next = idx > 0 ? tabs[idx - 1] : tabs[idx + 1];
            if (next) {
                setActiveTabId(next.id);
                setLoading(next.url !== 'about:blank');
                // 激活 WebView（显示）
                await invoke('activate_tab', { activeTabId: next.id });
            }
        }
    }, []);

    const switchTab = useCallback(
        async (id: number, url?: string) => {
            setActiveTabId(id);
            if (url !== undefined) {
                setLoading(url !== 'about:blank');
            }
            await invoke('activate_tab', { activeTabId: id });
        },
        [],
    );

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

    const minimizeWindow = useCallback(() => {
        appWindow.minimize().catch(console.error);
    }, []);

    const toggleMaximize = useCallback(() => {
        appWindow.toggleMaximize().catch(console.error);
    }, []);

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
            {/* 标题栏 */}
            <div
                data-tauri-drag-region
                style={{
                    display: 'flex',
                    alignItems: 'center',
                    height: TITLEBAR_HEIGHT,
                    background: '#1a1a2e',
                    borderBottom: '1px solid #2d2d4a',
                    flexShrink: 0,
                    paddingLeft: 70,
                }}
            >
                <div style={{ flex: 1, height: '100%' }} />
                <div
                    data-tauri-drag-region="false"
                    style={{
                        display: 'flex',
                        gap: 8,
                        padding: '0 12px',
                    }}
                >
                    <button onClick={minimizeWindow} style={btnStyle}>
                        −
                    </button>
                    <button onClick={toggleMaximize} style={btnStyle}>
                        □
                    </button>
                </div>
            </div>

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

            {/* 工具栏 */}
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
                }}
            >
                <button
                    onClick={goBack}
                    disabled={!activeTab || activeTab.historyIndex <= 0}
                    style={{
                        ...navBtn,
                        opacity: activeTab && activeTab.historyIndex > 0 ? 1 : 0.3,
                    }}
                >
                    ←
                </button>
                <button
                    onClick={goForward}
                    disabled={
                        !activeTab ||
                        activeTab.historyIndex >= activeTab.history.length - 1
                    }
                    style={{
                        ...navBtn,
                        opacity:
                            activeTab &&
                            activeTab.historyIndex < activeTab.history.length - 1
                                ? 1
                                : 0.3,
                    }}
                >
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
                </div>
            </div>

            {/* 进度条 */}
            {loading && (
                <div
                    style={{
                        height: 2,
                        background: '#6366f1',
                        flexShrink: 0,
                    }}
                />
            )}

            {/* 内容区域 — 由多个 content webview 覆盖（透明，WebView 在下方显示） */}
            <div
                style={{
                    flex: 1,
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    background: 'transparent',
                    color: '#8080a0',
                    fontSize: 14,
                    overflow: 'hidden',
                    position: 'relative',
                }}
            >
                {/* 占位内容仅在 about:blank 且无加载时显示 */}
                {(!activeTab || activeTab.url === 'about:blank') && !loading && (
                    <div style={{ textAlign: 'center', pointerEvents: 'none' }}>
                        <div style={{ fontSize: 48, opacity: 0.4, marginBottom: 16 }}>
                            🌐
                        </div>
                        <h2 style={{ fontSize: 18, fontWeight: 600, marginBottom: 8 }}>
                            迷你浏览器
                        </h2>
                        <p>输入网址开始浏览</p>
                    </div>
                )}
            </div>

            {/* 状态栏 */}
            <div
                style={{
                    height: STATUSBAR_HEIGHT,
                    background: '#1a1a2e',
                    borderTop: '1px solid #2d2d4a',
                    display: 'flex',
                    alignItems: 'center',
                    padding: '0 12px',
                    fontSize: 12,
                    color: '#8080a0',
                    flexShrink: 0,
                }}
            >
                {loading ? (
                    <span>⏳ 加载中...</span>
                ) : activeTab && activeTab.url !== 'about:blank' ? (
                    <span>✅ {activeTab.url}</span>
                ) : (
                    <span>🌐 准备浏览</span>
                )}
            </div>
        </div>
    );
}
/**
 * 浏览器模式 IPC 垫片
 *
 * 前端既跑在 Tauri webview 里（真实 IPC），也跑在普通浏览器里（e2e 用）。
 * 浏览器里没有 window.__TAURI_INTERNALS__，这里用官方 mock 补一个最小实现：
 * 记录调用参数供测试断言，不做真实导航，避免测试页被跳走。
 *
 * 注意：本模块有副作用（导入即安装），必须在 App 之前导入
 * —— App 模块顶层会调用 getCurrentWindow()，那时还没有 __TAURI_INTERNALS__.metadata。
 */
import { mockIPC, mockWindows } from '@tauri-apps/api/mocks';
import { emit } from '@tauri-apps/api/event';

interface BrowserCall {
    cmd: string;
    args: Record<string, unknown>;
}

const calls: BrowserCall[] = [];

function installBrowserShim(): void {
    if ('__TAURI_INTERNALS__' in window) {
        return;
    }

    mockWindows('main');
    mockIPC(
        (cmd: string, payload?: unknown) => {
            const args = (payload ?? {}) as Record<string, unknown>;
            calls.push({ cmd, args });
            switch (cmd) {
                case 'get_settings':
                    return { auto_clear_on_exit: false };
                case 'activate_tab':
                    return {
                        tab_id: args.activeTabId,
                        needs_recreate: false,
                        exists: true,
                    };
                default:
                    return null;
            }
        },
        { shouldMockEvents: true },
    );

    // 模拟后端事件（测试驱动用）
    (window as unknown as Record<string, unknown>).__mbEmit = (
        event: string,
        payload: unknown,
    ) => {
        void emit(event, payload);
    };

    (window as unknown as Record<string, unknown>).__mbCalls = calls;
}

installBrowserShim();

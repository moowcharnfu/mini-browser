/** 空白页 URL：新标签页与欢迎页共用 */
export const BLANK_URL = 'about:blank';

export interface Tab {
    id: number;
    title: string;
    url: string;
    history: string[];
    historyIndex: number;
    addressInput: string;
}

/** 是否为空白页 */
export function isBlankUrl(url: string): boolean {
    return url === BLANK_URL;
}

/** 标签标题：空白页显示「新标签页」，其余显示 URL */
export function deriveTabTitle(url: string): string {
    return isBlankUrl(url) ? '新标签页' : url;
}

/** 新标签 ID：当前最大 ID + 1 */
export function nextTabId(tabs: Tab[]): number {
    return Math.max(0, ...tabs.map((t) => t.id)) + 1;
}

/** 创建默认标签 */
export function defaultTab(id: number): Tab {
    return {
        id,
        title: '新标签页',
        url: BLANK_URL,
        history: [BLANK_URL],
        historyIndex: 0,
        addressInput: '',
    };
}

/**
 * URL 等价比较：忽略大小写、末尾斜杠与 fragment
 * 与 Rust 侧 urls_equivalent 保持一致，避免两端对「同一条 URL」判定不同
 */
export function urlsEquivalent(a: string, b: string): boolean {
    /** 仅 scheme + host 大小写不敏感；路径与查询串大小写敏感（/Path 与 /path 常是两个页面） */
    const normalize = (u: string): string => {
        let s = u.trim();
        const hash = s.indexOf('#');
        if (hash >= 0) {
            s = s.slice(0, hash);
        }
        const sep = s.indexOf('://');
        if (sep >= 0) {
            const scheme = s.slice(0, sep).toLowerCase();
            const rest = s.slice(sep + 3);
            const end = rest.search(/[/?]/);
            const host = end >= 0 ? rest.slice(0, end) : rest;
            const tail = end >= 0 ? rest.slice(end) : '';
            s = `${scheme}://${host.toLowerCase()}${tail}`;
        }
        while (s.length > 1 && s.endsWith('/')) {
            s = s.slice(0, -1);
        }
        return s;
    };
    return normalize(a) === normalize(b);
}

/**
 * 往历史栈里追加一条记录
 * 与当前条目等价时不重复入栈，返回新的栈与游标
 */
export function pushHistory(
    history: string[],
    historyIndex: number,
    url: string,
): { history: string[]; historyIndex: number } {
    if (history[historyIndex] !== undefined && urlsEquivalent(history[historyIndex], url)) {
        return { history, historyIndex };
    }
    return {
        history: [...history.slice(0, historyIndex + 1), url],
        historyIndex: historyIndex + 1,
    };
}

/** 关闭标签后选中的下一个标签：优先前一个，否则后一个 */
export function pickNextActiveTab(tabs: Tab[], closedId: number): Tab | undefined {
    if (tabs.length <= 1) {
        return undefined;
    }
    const idx = tabs.findIndex((t) => t.id === closedId);
    if (idx < 0) {
        return undefined;
    }
    return idx > 0 ? tabs[idx - 1] : tabs[idx + 1];
}

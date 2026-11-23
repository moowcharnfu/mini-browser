import { describe, it, expect } from 'vitest';
import {
    BLANK_URL,
    defaultTab,
    deriveTabTitle,
    isBlankUrl,
    nextTabId,
    pickNextActiveTab,
    pushHistory,
    urlsEquivalent,
    type Tab,
} from '../../src/utils/tab';

function tab(id: number, url = BLANK_URL): Tab {
    return { ...defaultTab(id), url, title: deriveTabTitle(url) };
}

describe('标签页工具', () => {
    describe('isBlankUrl / deriveTabTitle', () => {
        it('应识别空白页', () => {
            expect(isBlankUrl(BLANK_URL)).toBe(true);
            expect(isBlankUrl('https://example.com')).toBe(false);
        });

        it('空白页标题为「新标签页」，其余为 URL', () => {
            expect(deriveTabTitle(BLANK_URL)).toBe('新标签页');
            expect(deriveTabTitle('https://example.com')).toBe('https://example.com');
        });
    });

    describe('nextTabId', () => {
        it('空列表从 1 开始', () => {
            expect(nextTabId([])).toBe(1);
        });

        it('取当前最大 ID + 1', () => {
            expect(nextTabId([tab(1), tab(7), tab(3)])).toBe(8);
        });
    });

    describe('urlsEquivalent', () => {
        it('忽略大小写、末尾斜杠与 fragment', () => {
            expect(urlsEquivalent('https://a.com/', 'https://a.com')).toBe(true);
            expect(urlsEquivalent('https://a.com/x#frag', 'https://a.com/x')).toBe(true);
            expect(urlsEquivalent('HTTPS://A.COM', 'https://a.com')).toBe(true);
        });

        it('协议不同则不等价', () => {
            expect(urlsEquivalent('http://a.com', 'https://a.com')).toBe(false);
        });

        it('路径大小写敏感（/Path 与 /path 是两个页面）', () => {
            expect(urlsEquivalent('https://a.com/Path', 'https://a.com/path')).toBe(false);
            expect(urlsEquivalent('https://a.com/?k=V', 'https://a.com/?k=v')).toBe(false);
        });
    });

    describe('pushHistory', () => {
        it('新 URL 应入栈并推进游标', () => {
            const r = pushHistory(['about:blank'], 0, 'https://a.com');
            expect(r.history).toEqual(['about:blank', 'https://a.com']);
            expect(r.historyIndex).toBe(1);
        });

        it('与当前条目相同的 URL 不重复入栈', () => {
            const r = pushHistory(['https://a.com'], 0, 'https://a.com');
            expect(r.history).toEqual(['https://a.com']);
            expect(r.historyIndex).toBe(0);
        });

        it('与当前条目等价（尾斜杠/fragment）的 URL 不重复入栈', () => {
            const r = pushHistory(['https://a.com'], 0, 'https://a.com/#x');
            expect(r.history).toEqual(['https://a.com']);
            expect(r.historyIndex).toBe(0);
        });

        it('后退后新导航应截断前向分支', () => {
            const r = pushHistory(['https://a.com', 'https://b.com'], 0, 'https://c.com');
            expect(r.history).toEqual(['https://a.com', 'https://c.com']);
            expect(r.historyIndex).toBe(1);
        });
    });

    describe('pickNextActiveTab', () => {
        it('关闭中间的标签应选前一个', () => {
            const tabs = [tab(1), tab(2), tab(3)];
            expect(pickNextActiveTab(tabs, 2)?.id).toBe(1);
        });

        it('关闭第一个标签应选后一个', () => {
            const tabs = [tab(1), tab(2), tab(3)];
            expect(pickNextActiveTab(tabs, 1)?.id).toBe(2);
        });

        it('只剩一个标签时不返回', () => {
            expect(pickNextActiveTab([tab(1)], 1)).toBeUndefined();
        });
    });
});

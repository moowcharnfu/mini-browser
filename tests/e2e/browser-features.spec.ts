import { test, expect } from '@playwright/test';

async function callsOf(page: import('@playwright/test').Page, cmd: string) {
    return page.evaluate((target: string) => {
        const calls = (window as unknown as { __mbCalls?: { cmd: string; args: Record<string, unknown> }[] }).__mbCalls ?? [];
        return calls.filter((c) => c.cmd === target).map((c) => c.args);
    }, cmd);
}

test.describe('浏览器核心功能', () => {
    test.beforeEach(async ({ page }) => {
        await page.goto('/');
        await expect(page.locator('[data-testid="address-bar"]')).toBeVisible();
    });

    test('设置面板可展开与收起', async ({ page }) => {
        const panel = page.locator('[data-testid="settings-panel"]');
        await expect(panel).toHaveCount(0);

        await page.locator('[data-testid="settings-toggle"]').click();
        await expect(panel).toHaveCount(1);

        await page.locator('[data-testid="settings-toggle"]').click();
        await expect(panel).toHaveCount(0);
    });

    test('退出自动清除开关应写回设置', async ({ page }) => {
        await page.locator('[data-testid="settings-toggle"]').click();
        await page.locator('[data-testid="auto-clear-toggle"]').click();

        await expect.poll(async () => (await callsOf(page, 'set_settings')).length).toBe(1);
        const args = (await callsOf(page, 'set_settings'))[0];
        expect(args.settings).toEqual({ auto_clear_on_exit: true });
    });

    test('清除缓存按钮应调用 clear_browsing_data', async ({ page }) => {
        await page.locator('[data-testid="settings-toggle"]').click();
        await page.locator('[data-testid="clear-cache"]').click();

        await expect.poll(async () => (await callsOf(page, 'clear_browsing_data')).length).toBe(1);
    });

    test('后退/前进按钮应触发对应命令', async ({ page }) => {
        // 先模拟一次 URL 变化，让当前标签有历史可退
        await page.evaluate(() => {
            (window as unknown as { __mbEmit?: (e: string, p: unknown) => void }).__mbEmit?.(
                'browser://url-changed',
                { tab_id: 1, url: 'https://example.com' },
            );
        });

        await page.locator('[data-testid="back-button"]').click();
        await expect.poll(async () => (await callsOf(page, 'go_back_tab')).length).toBe(1);

        await page.locator('[data-testid="forward-button"]').click();
        await expect.poll(async () => (await callsOf(page, 'go_forward_tab')).length).toBe(1);
    });

    test('开发者工具按钮应触发 open_devtools_tab', async ({ page }) => {
        await page.locator('[data-testid="devtools-button"]').click();
        await expect.poll(async () => (await callsOf(page, 'open_devtools_tab')).length).toBe(1);
    });
});

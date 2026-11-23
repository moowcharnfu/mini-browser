import { test, expect } from '@playwright/test';

/**
 * 前端在普通浏览器里跑（IPC 由 browserShim 垫片提供），
 * 这里断言「界面把用户输入原样交给 Rust」——
 * URL / 搜索词的归一化规则由 Rust 侧单测覆盖（cargo test）。
 */
async function lastNavigate(page: import('@playwright/test').Page) {
    return page.evaluate(() => {
        const calls = (window as unknown as { __mbCalls?: { cmd: string; args: Record<string, unknown> }[] }).__mbCalls ?? [];
        const found = [...calls].reverse().find((c) => c.cmd === 'navigate_to_url');
        return found?.args;
    });
}

test.describe('地址栏导航', () => {
    test.beforeEach(async ({ page }) => {
        await page.goto('/');
        await expect(page.locator('[data-testid="address-bar"]')).toBeVisible();
    });

    test('回车应把输入原样交给 navigate_to_url', async ({ page }) => {
        const addressBar = page.locator('[data-testid="address-bar"]');
        await addressBar.fill('example.com');
        await page.keyboard.press('Enter');

        await expect.poll(async () => (await lastNavigate(page))?.url).toBe('example.com');
    });

    test('点击 go 按钮同样触发导航', async ({ page }) => {
        const addressBar = page.locator('[data-testid="address-bar"]');
        await addressBar.fill('https://example.com/path');
        await page.locator('[data-testid="go-button"]').click();

        await expect.poll(async () => (await lastNavigate(page))?.url).toBe('https://example.com/path');
    });

    test('搜索词也应原样上送（判定由 Rust 归一化）', async ({ page }) => {
        const addressBar = page.locator('[data-testid="address-bar"]');
        await addressBar.fill('how to install node.js');
        await page.keyboard.press('Enter');

        await expect.poll(async () => (await lastNavigate(page))?.url).toBe('how to install node.js');
    });

    test('空输入不应触发导航', async ({ page }) => {
        await page.locator('[data-testid="address-bar"]').fill('   ');
        await page.keyboard.press('Enter');

        expect(await lastNavigate(page)).toBeUndefined();
    });

    test('地址栏输入不应被自动覆盖', async ({ page }) => {
        const addressBar = page.locator('[data-testid="address-bar"]');
        await addressBar.fill('example.com');
        await page.waitForTimeout(800);
        await expect(addressBar).toHaveValue('example.com');
    });
});

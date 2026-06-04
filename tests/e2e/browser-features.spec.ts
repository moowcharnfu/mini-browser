import { test, expect } from '@playwright/test';

test.describe('浏览器核心功能', () => {
  test('Bilibili 能正常加载', async ({ page }) => {
    const addressBar = page.locator('[data-testid="address-bar"]');
    await addressBar.fill('www.bilibili.com');
    await page.keyboard.press('Enter');

    // 等待页面加载（Bilibili 首页）
    await expect(page.locator('body')).toContainText('哔哩哔哩', { timeout: 30000 });
  });

  test('前进后退按钮能正常操作', async ({ page }) => {
    const addressBar = page.locator('[data-testid="address-bar"]');

    // 导航到 example.com
    await addressBar.fill('https://example.com');
    await page.keyboard.press('Enter');
    await expect(page.locator('h1')).toContainText('Example Domain', { timeout: 10000 });

    // 导航到另一个页面
    await addressBar.fill('https://httpbin.org/get');
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(/httpbin\.org/, { timeout: 10000 });

    // 点击后退按钮
    await page.locator('[data-testid="back-button"]').click();
    await expect(page.locator('h1')).toContainText('Example Domain', { timeout: 10000 });

    // 点击前进按钮
    await page.locator('[data-testid="forward-button"]').click();
    await expect(page).toHaveURL(/httpbin\.org/, { timeout: 10000 });
  });

  test('地址栏自动同步 WebView URL', async ({ page }) => {
    const addressBar = page.locator('[data-testid="address-bar"]');

    await addressBar.fill('https://example.com');
    await page.keyboard.press('Enter');

    await expect(async () => {
      const value = await addressBar.inputValue();
      expect(value).toContain('example.com');
    }).toPass({ timeout: 10000 });
  });

  test('标签页切换保持各自 URL', async ({ page }) => {
    const addressBar = page.locator('[data-testid="address-bar"]');
    const tabs = page.locator('[data-testid="tab"]');

    // 在第一个标签导航到 example.com
    await addressBar.fill('https://example.com');
    await page.keyboard.press('Enter');
    await expect(async () => {
      const value = await addressBar.inputValue();
      expect(value).toContain('example.com');
    }).toPass({ timeout: 10000 });

    // 新建第二个标签
    await page.locator('[data-testid="new-tab-button"]').click();
    await expect(tabs).toHaveCount(2);

    // 在第二个标签导航到 httpbin.org
    await addressBar.fill('https://httpbin.org/get');
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(/httpbin\.org/, { timeout: 10000 });

    // 切换回第一个标签
    await tabs.first().click();

    // 验证第一个标签的 URL 保持为 example.com
    await expect(async () => {
      const value = await addressBar.inputValue();
      expect(value).toContain('example.com');
    }).toPass({ timeout: 10000 });
  });
});

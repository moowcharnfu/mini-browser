import { test, expect } from '@playwright/test';

test.describe('地址栏导航', () => {
  test('输入完整URL应导航到目标页面', async ({ page }) => {
    const addressBar = page.locator('[data-testid="address-bar"]');
    await addressBar.fill('https://example.com');
    await page.keyboard.press('Enter');
    // 验证页面加载（使用内置超时，无需 waitForTimeout）
    await expect(page.locator('h1')).toContainText('Example Domain', { timeout: 10000 });
  });

  test('输入不带协议的域名应自动补全https', async ({ page }) => {
    const addressBar = page.locator('[data-testid="address-bar"]');
    await addressBar.fill('example.com');
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(/example\.com/);
  });

  test('输入搜索关键词应使用搜索引擎', async ({ page }) => {
    const addressBar = page.locator('[data-testid="address-bar"]');
    await addressBar.fill('Tauri framework');
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(/google\.com\/search/);
  });
});

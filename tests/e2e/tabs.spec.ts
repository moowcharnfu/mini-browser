import { test, expect } from '@playwright/test';

test.describe('标签页管理', () => {
  test('新建标签页应增加标签数量', async ({ page }) => {
    const tabs = page.locator('[data-testid="tab"]');
    const initialCount = await tabs.count();

    await page.locator('[data-testid="new-tab-button"]').click();
    await expect(tabs).toHaveCount(initialCount + 1);
  });

  test('点击标签页应切换激活状态', async ({ page }) => {
    await page.locator('[data-testid="new-tab-button"]').click();
    await expect(page.locator('[data-testid="tab"]')).toHaveCount(2);

    // 切换到第一个标签
    await page.locator('[data-testid="tab"]').first().click();
    await expect(page.locator('[data-testid="tab"]').first()).toHaveClass(/active/);
  });

  test('关闭标签页应减少标签数量', async ({ page }) => {
    await page.locator('[data-testid="new-tab-button"]').click();
    await expect(page.locator('[data-testid="tab"]')).toHaveCount(2);

    const tabs = page.locator('[data-testid="tab"]');

    // 关闭第二个标签
    await page.locator('[data-testid="tab"]').last().locator('[data-testid="close-tab"]').click();
    await expect(tabs).toHaveCount(1);
  });
});

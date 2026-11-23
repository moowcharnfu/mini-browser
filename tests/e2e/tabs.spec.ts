import { test, expect } from '@playwright/test';

test.describe('标签页管理', () => {
    test.beforeEach(async ({ page }) => {
        await page.goto('/');
        await expect(page.locator('[data-testid="address-bar"]')).toBeVisible();
    });

    test('新建标签页应增加标签数量', async ({ page }) => {
        const tabs = page.locator('[data-testid="tab"]');
        await expect(tabs).toHaveCount(1);

        await page.locator('[data-testid="new-tab-button"]').click();
        await expect(tabs).toHaveCount(2);
    });

    test('点击标签页应切换激活状态', async ({ page }) => {
        await page.locator('[data-testid="new-tab-button"]').click();
        await expect(page.locator('[data-testid="tab"]')).toHaveCount(2);

        await page.locator('[data-testid="tab"]').first().click();
        await expect(page.locator('[data-testid="tab"]').first()).toHaveClass(/active/);
        await expect(page.locator('[data-testid="tab"]').last()).not.toHaveClass(/active/);
    });

    test('关闭标签页应减少标签数量', async ({ page }) => {
        await page.locator('[data-testid="new-tab-button"]').click();
        await expect(page.locator('[data-testid="tab"]')).toHaveCount(2);

        await page.locator('[data-testid="tab"]').last().locator('[data-testid="close-tab"]').click();
        await expect(page.locator('[data-testid="tab"]')).toHaveCount(1);
    });

    test('只剩一个标签时不允许关闭', async ({ page }) => {
        await expect(page.locator('[data-testid="tab"]')).toHaveCount(1);
        await page.locator('[data-testid="tab"]').first().locator('[data-testid="close-tab"]').click();
        await expect(page.locator('[data-testid="tab"]')).toHaveCount(1);
    });
});

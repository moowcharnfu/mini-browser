import { test, expect } from '@playwright/test';

const BENCHMARKS = {
  coldStart: 3000,    // 冷启动 < 3s
  pageLoad: 5000,     // 页面加载 < 5s
};

test.describe('启动性能', () => {
  test('应用启动应在合理时间内完成', async ({ page }) => {
    const start = performance.now();
    await page.goto('http://localhost:1420');
    const end = performance.now();
    
    const duration = end - start;
    console.log(`启动时间: ${duration.toFixed(2)}ms`);
    expect(duration).toBeLessThan(BENCHMARKS.coldStart);
  });

  test('页面加载应在合理时间内完成', async ({ page }) => {
    const start = performance.now();
    await page.goto('https://example.com');
    await page.waitForLoadState('domcontentloaded');
    const end = performance.now();
    
    const loadTime = end - start;
    console.log(`页面加载时间: ${loadTime.toFixed(2)}ms`);
    expect(loadTime).toBeLessThan(BENCHMARKS.pageLoad);
  });
});

import { chromium } from 'playwright';

async function quickTest() {
  const browser = await chromium.launch({ headless: false });
  const page = await browser.newPage();

  console.log('Opening Tauri app...');
  await page.goto('http://localhost:1420');
  await page.waitForTimeout(5000);

  await page.screenshot({ path: 'test-results/quick-test.png' });
  console.log('Screenshot saved. Check test-results/quick-test.png');

  // Print all visible text
  const text = await page.locator('body').textContent();
  console.log('Page text:', text?.substring(0, 500));

  console.log('Waiting 60s for manual inspection...');
  await page.waitForTimeout(60000);
  await browser.close();
}

quickTest().catch(console.error);

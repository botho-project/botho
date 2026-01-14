import { chromium } from 'playwright';

async function debugTest() {
  const browser = await chromium.launch({ headless: false, devtools: true });
  const page = await browser.newPage();

  // Capture console logs
  page.on('console', msg => console.log('BROWSER:', msg.type(), msg.text()));
  page.on('pageerror', err => console.log('PAGE ERROR:', err.message));
  page.on('requestfailed', req => console.log('REQUEST FAILED:', req.url(), req.failure()?.errorText));

  console.log('Opening Tauri app...');
  await page.goto('http://localhost:1420');
  await page.waitForTimeout(5000);

  // Try a direct fetch to the node
  console.log('\\nTrying direct fetch to node...');
  const result = await page.evaluate(async () => {
    try {
      const response = await fetch('http://127.0.0.1:17101/rpc', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ jsonrpc: '2.0', method: 'node_getStatus', params: [], id: 1 })
      });
      const data = await response.json();
      return { success: true, data };
    } catch (e) {
      return { success: false, error: e.message };
    }
  });
  console.log('Direct fetch result:', JSON.stringify(result, null, 2));

  await page.screenshot({ path: 'test-results/debug-test.png' });
  console.log('\\nWaiting 60s for manual inspection...');
  await page.waitForTimeout(60000);
  await browser.close();
}

debugTest().catch(console.error);

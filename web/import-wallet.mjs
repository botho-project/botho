import { chromium } from 'playwright';

const MNEMONIC = 'damage vehicle jealous kidney honey cash employ tuition blossom cry mother bargain divert script bag stairs rally excuse sick cave palace inside rubber royal';
const PASSWORD = 'testpassword123';

async function importWalletAndSync() {
  console.log('Launching browser...');
  const browser = await chromium.launch({ headless: false });
  const context = await browser.newContext();
  const page = await context.newPage();

  console.log('Navigating to Tauri app at http://localhost:1420...');
  await page.goto('http://localhost:1420');
  await page.waitForLoadState('networkidle');
  await page.waitForTimeout(2000);

  await page.screenshot({ path: 'test-results/01-initial.png' });
  console.log('Screenshot: 01-initial.png');

  // Click refresh to trigger rescan with new ports
  const refreshButton = page.locator('[title="Refresh"]').or(page.locator('button:has-text("refresh")'));
  if (await refreshButton.count() > 0) {
    console.log('Clicking refresh...');
    await refreshButton.first().click();
    await page.waitForTimeout(3000);
    await page.screenshot({ path: 'test-results/02-after-refresh.png' });
  }

  // Wait for the scan to complete and check for nodes
  await page.waitForTimeout(5000);
  await page.screenshot({ path: 'test-results/03-after-scan.png' });

  // Check what buttons/elements are visible
  const buttons = await page.getByRole('button').allTextContents();
  console.log('Available buttons:', buttons);

  // Look for a node entry to click (discovered local node)
  const nodeEntry = page.locator('text=/testnet|17101|localhost|127.0.0.1/i');
  if (await nodeEntry.count() > 0) {
    console.log('Found local node entry, clicking to connect...');
    await nodeEntry.first().click();
    await page.waitForTimeout(3000);
    await page.screenshot({ path: 'test-results/04-after-node-click.png' });
  }

  // Check current state
  await page.screenshot({ path: 'test-results/05-current-state.png' });
  const pageContent = await page.content();

  // Look for wallet setup options
  console.log('Looking for wallet options...');
  const importButton = page.getByRole('button', { name: /import|restore|recover/i });
  const createButton = page.getByRole('button', { name: /create|new/i });

  if (await importButton.count() > 0) {
    console.log('Found import button, clicking...');
    await importButton.first().click();
    await page.waitForTimeout(1000);
    await page.screenshot({ path: 'test-results/06-import-clicked.png' });
  } else if (await createButton.count() > 0) {
    console.log('Found create button. Looking for import link...');
    const importLink = page.getByText(/import|restore|already have/i);
    if (await importLink.count() > 0) {
      await importLink.first().click();
      await page.waitForTimeout(1000);
      await page.screenshot({ path: 'test-results/06-import-clicked.png' });
    }
  } else {
    console.log('No wallet buttons found yet');
  }

  // Fill mnemonic
  const mnemonicInput = page.locator('textarea');
  if (await mnemonicInput.count() > 0) {
    console.log('Found mnemonic textarea, filling...');
    await mnemonicInput.first().fill(MNEMONIC);
    await page.screenshot({ path: 'test-results/07-mnemonic-filled.png' });

    // Click next/continue
    const nextButton = page.getByRole('button', { name: /next|continue|import/i });
    if (await nextButton.count() > 0) {
      await nextButton.first().click();
      await page.waitForTimeout(1000);
      await page.screenshot({ path: 'test-results/08-after-next.png' });
    }
  }

  // Fill password
  const passwordInputs = page.locator('input[type="password"]');
  const pwCount = await passwordInputs.count();
  if (pwCount >= 1) {
    console.log(`Found ${pwCount} password inputs, filling...`);
    await passwordInputs.first().fill(PASSWORD);
    if (pwCount >= 2) {
      await passwordInputs.nth(1).fill(PASSWORD);
    }
    await page.screenshot({ path: 'test-results/09-password-filled.png' });

    // Click final submit
    const submitButton = page.getByRole('button', { name: /create|import|confirm|done|submit/i });
    if (await submitButton.count() > 0) {
      await submitButton.first().click();
      await page.waitForTimeout(3000);
      await page.screenshot({ path: 'test-results/10-after-submit.png' });
    }
  }

  // Final state
  await page.screenshot({ path: 'test-results/11-final.png' });
  console.log('Final screenshot saved');

  // Look for balance
  const balanceEl = page.locator('text=/\\d+[.,]?\\d*\\s*BTH/i');
  if (await balanceEl.count() > 0) {
    const balanceText = await balanceEl.first().textContent();
    console.log('Balance found:', balanceText);
  }

  console.log('Done! Keeping browser open for 120s...');
  await page.waitForTimeout(120000);
  await browser.close();
}

importWalletAndSync().catch(console.error);

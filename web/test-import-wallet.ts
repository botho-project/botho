import { chromium } from 'playwright';

const MNEMONIC = 'damage vehicle jealous kidney honey cash employ tuition blossom cry mother bargain divert script bag stairs rally excuse sick cave palace inside rubber royal';
const PASSWORD = 'testpassword123';

async function importWalletAndSync() {
  const browser = await chromium.launch({ headless: false });
  const context = await browser.newContext();
  const page = await context.newPage();

  console.log('Navigating to Tauri app...');
  await page.goto('http://localhost:1420');
  await page.waitForLoadState('networkidle');

  // Take screenshot to see current state
  await page.screenshot({ path: 'test-results/01-initial.png' });
  console.log('Screenshot saved: test-results/01-initial.png');

  // Wait for app to load
  await page.waitForTimeout(2000);

  // Look for import/restore wallet button
  const importButton = page.getByRole('button', { name: /import|restore|recover/i });
  const createButton = page.getByRole('button', { name: /create|new/i });

  // Check what buttons are visible
  const buttons = await page.getByRole('button').allTextContents();
  console.log('Available buttons:', buttons);

  // Take another screenshot
  await page.screenshot({ path: 'test-results/02-buttons.png' });

  // Try to find and click import wallet
  try {
    if (await importButton.isVisible()) {
      console.log('Found import button, clicking...');
      await importButton.click();
      await page.waitForTimeout(1000);
      await page.screenshot({ path: 'test-results/03-after-import-click.png' });
    } else {
      console.log('Import button not visible, looking for other options...');

      // Maybe there's a menu or settings
      const menuButton = page.locator('[data-testid="menu"]').or(page.getByRole('button', { name: /menu|settings/i }));
      if (await menuButton.isVisible()) {
        await menuButton.click();
        await page.waitForTimeout(500);
        await page.screenshot({ path: 'test-results/03-menu.png' });
      }
    }
  } catch (e) {
    console.log('Error finding import button:', e);
  }

  // Look for mnemonic input field
  const mnemonicInput = page.locator('textarea').or(page.locator('input[type="text"]').first());

  if (await mnemonicInput.isVisible()) {
    console.log('Found mnemonic input, entering mnemonic...');
    await mnemonicInput.fill(MNEMONIC);
    await page.screenshot({ path: 'test-results/04-mnemonic-entered.png' });
  }

  // Look for password fields
  const passwordInputs = page.locator('input[type="password"]');
  const count = await passwordInputs.count();
  console.log(`Found ${count} password inputs`);

  if (count >= 1) {
    await passwordInputs.first().fill(PASSWORD);
    if (count >= 2) {
      await passwordInputs.nth(1).fill(PASSWORD);
    }
    await page.screenshot({ path: 'test-results/05-password-entered.png' });
  }

  // Look for submit/continue button
  const submitButton = page.getByRole('button', { name: /continue|submit|import|next|confirm/i });
  if (await submitButton.isVisible()) {
    console.log('Clicking submit button...');
    await submitButton.click();
    await page.waitForTimeout(2000);
    await page.screenshot({ path: 'test-results/06-after-submit.png' });
  }

  // Wait and take final screenshot
  await page.waitForTimeout(3000);
  await page.screenshot({ path: 'test-results/07-final.png' });

  // Get page content for debugging
  const content = await page.content();
  console.log('Page title:', await page.title());

  // Look for balance display
  const balanceText = await page.locator('text=/\\d+\\.?\\d*\\s*BTH/i').textContent().catch(() => null);
  if (balanceText) {
    console.log('Balance found:', balanceText);
  }

  console.log('Done! Check test-results/ for screenshots');

  // Keep browser open for manual inspection
  await page.waitForTimeout(30000);
  await browser.close();
}

importWalletAndSync().catch(console.error);

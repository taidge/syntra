import { test, expect, Page } from '@playwright/test';
import { createDocument, registerUser, uniqueEmail, uniqueUsername } from './helpers';

test.use({ locale: 'en-US', serviceWorkers: 'block' });

async function expectCalcToolbarFits(page: Page) {
  const toolbar = page.locator('.calc-toolbar');
  await expect(toolbar).toBeVisible();
  // Resize observation can briefly leave the previous set of inline actions.
  await expect.poll(async () => toolbar.evaluate(element => {
    const box = element as HTMLElement;
    const style = getComputedStyle(box);
    const horizontalScrollbar = box.offsetHeight - box.clientHeight
      - parseFloat(style.borderTopWidth) - parseFloat(style.borderBottomWidth);
    const verticalScrollbar = box.offsetWidth - box.clientWidth
      - parseFloat(style.borderLeftWidth) - parseFloat(style.borderRightWidth);
    return Math.max(box.scrollWidth - box.clientWidth,
      box.scrollHeight - box.clientHeight, horizontalScrollbar, verticalScrollbar);
  })).toBeLessThanOrEqual(1);

  const more = page.getByRole('button', { name: /^More tools \(/ });
  if (await more.isVisible()) {
    await more.click();
    const overflow = page.locator('.sapid-overflow-panel');
    await expect(overflow).toBeVisible();
    await overflow.getByRole('button', { name: 'Clear borders', exact: true }).click();
    await page.keyboard.press('Escape');
    await expect(overflow).toBeHidden();
  }
}

test('homepage keeps the Dioxus typography and four-column feature layout', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/');
  await expect(page.locator('.hero-title')).toHaveCSS('font-size', '48px');
  await expect(page.locator('.hero-subtitle')).toHaveCSS('font-size', '20px');
  await expect(page.locator('.section-title')).toHaveCSS('font-size', '32px');
  const cards = await page.locator('.feature-card').all();
  expect(cards).toHaveLength(4);
  const bounds = await Promise.all(cards.map(card => card.boundingBox()));
  expect(new Set(bounds.map(box => Math.round(box!.y))).size).toBe(1);
  expect(bounds[3]!.x + bounds[3]!.width).toBeLessThanOrEqual(1440);
  await page.screenshot({ path: test.info().outputPath('home-desktop.png'), fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.locator('.hero-title')).toHaveCSS('font-size', '32px');
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await page.screenshot({ path: test.info().outputPath('home-mobile.png'), fullPage: true });
});

test('account and file pages preserve their cards and fit narrow screens', async ({ page, request }) => {
  const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'LayoutTest@123');
  await page.addInitScript(auth => {
    localStorage.setItem('contrix.auth', JSON.stringify({ token: auth.token, user: {
      id: auth.userId, username: auth.username, email: auth.email, role: 'editor',
    } }));
    localStorage.setItem('locale', 'en');
  }, user);
  for (const width of [1440, 390]) {
    await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
    for (const route of ['login', 'register', 'dashboard', 'files', 'profile']) {
      await page.goto(`/${route}`);
      await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
      if (route === 'login' || route === 'register') {
        const card = await page.locator('.auth-card').boundingBox();
        expect(card!.width).toBeLessThanOrEqual(width);
        await expect(page.getByLabel('Email', { exact: true })).toHaveCSS('height', '42px');
        await expect(page.getByLabel('Email', { exact: true })).toHaveAttribute('placeholder', /\S/);
        await expect(page.getByLabel('Password', { exact: true })).toHaveAttribute('placeholder', /\S/);
      }
      if (route === 'profile') {
        await expect(page.locator('.totp-security')).toBeVisible();
        await expect(page.getByRole('button', { name: 'Verify and enable', exact: true })).toHaveCount(0);
      }
      if (route === 'files') {
        await expect(page.locator('.file-search')).toHaveAttribute('placeholder', 'Search files...');
        await expect(page.locator('.navbar-search input')).toHaveAttribute('placeholder', 'Search documents...');
      }
      await page.screenshot({ path: test.info().outputPath(`${route}-${width}.png`), fullPage: true });
    }
  }
});

for (const kind of ['writer', 'calc', 'slides', 'whiteboard'] as const) {
  test(`${kind} keeps its workspace visible on desktop and mobile`, async ({ page, request }) => {
    const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'LayoutTest@123');
    const { body, response } = await createDocument(request, user.token, 'Layout reference', kind);
    expect(response.ok(), JSON.stringify(body)).toBe(true);
    await page.addInitScript(auth => {
      localStorage.setItem('contrix.auth', JSON.stringify({ token: auth.token, user: {
        id: auth.userId, username: auth.username, email: auth.email, role: 'editor',
      } }));
      localStorage.setItem('locale', 'en');
      localStorage.setItem('theme', 'light');
    }, user);
    const failures: string[] = [];
    page.on('pageerror', error => failures.push(error.message));
    page.on('console', message => {
      if (message.type() === 'error' && message.text().includes('failed to start')) failures.push(message.text());
    });
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.goto(`/${kind}/${body.data.id}`);
    await expect(page.locator('.save-status')).not.toContainText('Loading');
    const canvas = page.locator({ writer: '.writer-page', calc: '.sheet-scroll',
      slides: '.slide-canvas', whiteboard: '.whiteboard-canvas' }[kind]);
    await expect(canvas).toBeVisible();
    const nav = await page.locator('.navbar').boundingBox();
    expect(nav!.y).toBe(0);
    expect(nav!.height).toBe(56);
    const desktop = await canvas.boundingBox();
    expect(desktop!.width).toBeGreaterThan(400);
    expect(desktop!.height).toBeGreaterThan(kind === 'slides' ? 300 : 500);
    if (kind === 'writer') expect(desktop!.width).toBe(816);
    if (kind === 'calc') {
      await expect(page.locator('.cell-button').first()).toHaveCSS('border-top-width', '0px');
      await expect(page.locator('.cell-button').first()).toHaveCSS('background-color', 'rgba(0, 0, 0, 0)');
      await expectCalcToolbarFits(page);
    }
    await page.screenshot({ path: test.info().outputPath(`${kind}-desktop.png`) });
    if (kind === 'calc') {
      await page.setViewportSize({ width: 1248, height: 1000 });
      await expectCalcToolbarFits(page);
      await page.screenshot({ path: test.info().outputPath('calc-1248.png') });
    }
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(canvas).toBeVisible();
    if (kind === 'calc') await expectCalcToolbarFits(page);
    await page.screenshot({ path: test.info().outputPath(`${kind}-mobile.png`) });
    const mobileNav = await page.locator('.navbar').boundingBox();
    const navbarEnd = await page.locator('.navbar-end').boundingBox();
    expect(navbarEnd!.y + navbarEnd!.height).toBeLessThanOrEqual(mobileNav!.y + mobileNav!.height);
    const mobile = await canvas.boundingBox();
    expect(mobile!.y).toBeLessThan(500);
    expect(mobile!.height).toBeGreaterThan(150);
    if (kind === 'whiteboard') {
      const tools = page.locator('.board-tools-strip');
      const selector = await page.getByRole('combobox', { name: 'Drawing tool', exact: true }).boundingBox();
      expect(selector!.width).toBeGreaterThanOrEqual(110);
      expect(await tools.evaluate(element => element.scrollWidth)).toBeGreaterThan(390);
      const remove = await page.getByRole('button', { name: 'Delete selected', exact: true }).boundingBox();
      const duplicate = await page.getByRole('button', { name: 'Duplicate selected', exact: true }).boundingBox();
      expect(remove!.width).toBeGreaterThan(80);
      expect(duplicate!.x).toBeGreaterThanOrEqual(remove!.x + remove!.width);
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    expect(failures).toEqual([]);
  });
}

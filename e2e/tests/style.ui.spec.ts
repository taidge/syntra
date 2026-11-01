import { test, expect, Page, Locator } from '@playwright/test';
import { createDocument, registerUser, uniqueEmail, uniqueUsername, AuthUser } from './helpers';

test.use({ locale: 'en-US', serviceWorkers: 'block' });

async function authenticate(page: Page, user: AuthUser) {
  await page.addInitScript(auth => {
    localStorage.setItem('contrix.auth', JSON.stringify({ token: auth.token, user: {
      id: auth.userId, username: auth.username, email: auth.email, role: 'editor',
    } }));
    localStorage.setItem('locale', 'en');
    localStorage.setItem('theme', 'light');
  }, user);
}

async function expectCenteredIcon(control: Locator) {
  await expect(control).toHaveCSS('height', '32px');
  await expect(control).toHaveCSS('width', '32px');
  const icon = control.locator('svg.ui-icon:visible');
  await expect(icon).toHaveCount(1);
  expect(await icon.locator('path').count()).toBeGreaterThan(0);
  const controlColor = await control.evaluate(element => getComputedStyle(element).color);
  await expect(icon.locator('path').first()).toHaveCSS('stroke', controlColor);
  const outer = (await control.boundingBox())!;
  const inner = (await icon.boundingBox())!;
  expect(Math.abs(inner.width - inner.height)).toBeLessThanOrEqual(0.5);
  expect(Math.abs(inner.x + inner.width / 2 - outer.x - outer.width / 2)).toBeLessThanOrEqual(1);
  expect(Math.abs(inner.y + inner.height / 2 - outer.y - outer.height / 2)).toBeLessThanOrEqual(1);
}

test('navbar SVG icons share aligned controls and files fit the mobile viewport', async ({ page, request }) => {
  const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'StyleTest@123');
  await authenticate(page, user);
  for (const width of [1440, 390]) {
    await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
    await page.goto('/files');
    await expect(page.getByRole('heading', { name: 'My Files', exact: true })).toBeVisible();
    await expectCenteredIcon(page.locator('.navbar .notification-button'));
    await expectCenteredIcon(page.locator('.navbar .theme-toggle'));
    await expect(page.locator('.navbar .lang-toggle')).toHaveCSS('height', '32px');
    await expect(page.locator('.navbar .brand-icon svg.ui-icon')).toBeVisible();
    expect(await page.locator('.navbar .theme-toggle').evaluate(element => parseFloat(getComputedStyle(element).fontSize)))
      .toBeGreaterThan(0);
    const navbar = (await page.locator('.navbar').boundingBox())!;
    const end = (await page.locator('.navbar-end').boundingBox())!;
    expect(end.x + end.width).toBeLessThanOrEqual(width);
    expect(end.y + end.height).toBeLessThanOrEqual(navbar.y + navbar.height);
    if (width === 390) {
      expect(await page.evaluate(() => document.body.scrollHeight)).toBeLessThanOrEqual(845);
      expect(await page.evaluate(() => document.body.scrollWidth)).toBeLessThanOrEqual(390);
      await expect(page.locator('.mobile-bar .ui-icon:visible')).toHaveCount(4);
    }
    await page.locator('.navbar .theme-toggle').focus();
    await page.keyboard.press('Tab');
    const focused = page.locator(':focus-visible');
    await expect(focused).toHaveCount(1);
    await expect(focused).toHaveCSS('outline-width', '2px');
    await expect(focused).toHaveCSS('outline-style', 'solid');
    await page.screenshot({ path: test.info().outputPath('files-icons-' + width + '.png') });
  }
});

test('mobile editor scrolling stays inside the workspace instead of adding a second page scrollbar', async ({ page, request }) => {
  const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'StyleTest@123');
  await authenticate(page, user);
  await page.setViewportSize({ width: 390, height: 844 });
  for (const kind of ['writer', 'calc', 'slides', 'whiteboard'] as const) {
    const { body, response } = await createDocument(request, user.token, 'Workspace scroll check', kind);
    expect(response.ok(), JSON.stringify(body)).toBe(true);
    await page.goto('/' + kind + '/' + body.data.id);
    const scroller = page.locator({
      writer: '.writer-canvas', calc: '.sheet-scroll',
      slides: '.slide-stage', whiteboard: '.whiteboard-viewport',
    }[kind]);
    await expect(scroller).toBeVisible();
    if (kind === 'calc') await expect(page.locator('.sheet-grid')).toBeVisible();
    await expect(scroller).toHaveCSS('scrollbar-width', 'thin');
    await expect(scroller).toHaveCSS('overflow-x', 'auto');
    await expect(scroller).toHaveCSS('overflow-y', 'auto');
    expect(await page.evaluate(() => document.body.scrollHeight)).toBeLessThanOrEqual(845);
    expect(await page.evaluate(() => document.body.scrollWidth)).toBeLessThanOrEqual(390);
    if (kind === 'calc' || kind === 'writer') {
      expect(await scroller.evaluate(element => element.scrollWidth - element.clientWidth)).toBeGreaterThan(1);
    }
    await page.screenshot({ path: test.info().outputPath(kind + '-mobile-scroll.png') });
  }
});

test('light and dark menus use the workspace scrollbar palette and readable text', async ({ page, request }) => {
  const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'StyleTest@123');
  const { body, response } = await createDocument(request, user.token, 'Theme scroll check', 'calc');
  expect(response.ok(), JSON.stringify(body)).toBe(true);
  await authenticate(page, user);
  await page.goto('/calc/' + body.data.id);
  await expect(page.locator('.sheet-grid')).toBeVisible();
  await expect(page.locator('.status-bar')).toContainText('Sum: 0');
  for (const theme of ['light', 'dark', 'light']) {
    const effectiveTheme = await page.locator('html').getAttribute('data-theme') ?? 'light';
    if (effectiveTheme !== theme) await page.getByRole('button', { name: 'Toggle dark mode', exact: true }).click();
    await expect.poll(async () => await page.locator('html').getAttribute('data-theme') ?? 'light').toBe(theme);
    await page.getByRole('button', { name: 'View', exact: true }).click();
    const menu = page.getByRole('menu');
    await expect(menu).toBeVisible();
    await expect(menu).toHaveCSS('scrollbar-width', 'thin');
    await expect(menu).toHaveCSS('scrollbar-gutter', 'stable');
    await expect(page.locator('.sheet-scroll')).toHaveCSS('scrollbar-gutter', 'stable');
    const colors = await menu.evaluate(element => {
      const menuStyle = getComputedStyle(element);
      const sheetStyle = getComputedStyle(document.querySelector('.sheet-scroll')!);
      const itemStyle = getComputedStyle(element.querySelector('[role="menuitem"]')!);
      const rgb = (color: string) => color.match(/[\d.]+/g)!.slice(0, 3).map(Number);
      const luminance = (color: string) => rgb(color).map(channel => channel / 255)
        .map(channel => channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4)
        .reduce((sum, channel, index) => sum + channel * [0.2126, 0.7152, 0.0722][index], 0);
      const levels = [luminance(itemStyle.color), luminance(menuStyle.backgroundColor)].sort((a, b) => a - b);
      return {
        menu: menuStyle.scrollbarColor, sheet: sheetStyle.scrollbarColor,
        text: itemStyle.color, bodyText: getComputedStyle(document.body).color,
        background: menuStyle.backgroundColor, sheetBackground: sheetStyle.backgroundColor,
        contrast: (levels[1] + 0.05) / (levels[0] + 0.05),
        webkitWidth: getComputedStyle(document.querySelector('.sheet-scroll')!, '::-webkit-scrollbar').width,
      };
    });
    expect(colors.menu).toBe(colors.sheet);
    expect(colors.menu).toBe(theme === 'dark'
      ? 'rgb(100, 116, 139) rgb(15, 23, 42)' : 'rgb(100, 116, 139) rgb(226, 232, 240)');
    expect(colors.webkitWidth).toBe('8px');
    expect(colors.text).toBe(colors.bodyText);
    expect(colors.background).toBe(colors.sheetBackground);
    expect(colors.contrast).toBeGreaterThanOrEqual(4.5);
    await page.screenshot({ path: test.info().outputPath('menu-scroll-' + theme + '.png') });
    await page.keyboard.press('Escape');
    await expect(menu).toBeHidden();
  }
});

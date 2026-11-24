import { test, expect } from '@playwright/test';
import { resolve } from 'node:path';
import { registerUser, uniqueEmail, uniqueUsername } from './helpers';

test('PPTX import retains large reused backgrounds and wrapped text @ui', async ({ page, request }, testInfo) => {
  test.setTimeout(90_000);
  const auth = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');
  const headers = { Authorization: `Bearer ${auth.token}` };
  const user = (await (await request.get('/api/v1/auth/me', { headers })).json()).data;
  await page.addInitScript(({ token, user }) => {
    localStorage.setItem('syntra.auth', JSON.stringify({ token, user }));
    localStorage.setItem('locale', 'en');
  }, { token: auth.token, user });
  await page.goto('/files');
  const picker = page.waitForEvent('filechooser');
  await page.getByRole('button', { name: 'Upload', exact: true }).click();
  const uploaded = page.waitForResponse(response => response.url().endsWith('/api/v1/files/upload') && response.request().method() === 'POST');
  await (await picker).setFiles(resolve(__dirname, '../../crates/export/tests/fixtures/reused-large-background.pptx'));
  const response = await uploaded;
  expect(response.status()).toBe(201);
  const upload = (await response.json()).data;
  expect(upload.document_id).toBeTruthy();
  expect(upload.import_warnings.join(' ')).not.toMatch(/media.*budget|unsupported or missing/i);
  const document = (await (await request.get(`/api/v1/documents/${upload.document_id}`, { headers })).json()).data;
  expect(document.version).toBe(1);
  expect(document.content.metadata.pptx_import_revision).toBe(2);
  expect(document.content.slides).toHaveLength(20);
  expect(Buffer.byteLength(JSON.stringify(document.content))).toBeLessThan(20 * 1024 * 1024);
  for (const slide of document.content.slides) {
    expect(slide.elements.filter((element: any) => element.type === 'Image')).toHaveLength(1);
  }

  await page.getByRole('link', { name: 'reused-large-background.pptx', exact: true }).click();
  await expect(page).toHaveURL(/\/slides\//);
  await expect(page.locator('.slide-thumbnail')).toHaveCount(20);
  for (const viewport of [{ width: 1280, height: 720 }, { width: 900, height: 700 }, { width: 390, height: 844 }]) {
    await page.setViewportSize(viewport);
    await expect.poll(() => page.locator('.slide-list').evaluate(list => {
      const cards = [...list.querySelectorAll('.slide-thumbnail')];
      return cards.every((card, index) => {
        const bounds = card.getBoundingClientRect();
        const picture = card.querySelector('img')!.getBoundingClientRect();
        const number = card.querySelector('.slide-thumb-number')!.getBoundingClientRect();
        const next = cards[index + 1]?.getBoundingClientRect();
        return picture.height > 0 && picture.top >= bounds.top && picture.bottom <= bounds.bottom + 1
          && number.right <= picture.left + 1 && (!next || next.top >= bounds.bottom + 8);
      }) && list.scrollHeight > list.clientHeight;
    }), { message: `thumbnail cards contain their previews and scroll at ${viewport.width}px` }).toBe(true);
  }
  await page.setViewportSize({ width: 1280, height: 720 });
  const raster = page.getByRole('img', { name: 'Reusable navy background', exact: true });
  await expect(raster).toBeVisible();
  await expect.poll(() => raster.evaluate((image: HTMLImageElement) => image.complete && image.naturalWidth > 0)).toBe(true);
  const pixels = await raster.evaluate((image: HTMLImageElement) => {
    const canvas = document.createElement('canvas');
    canvas.width = 1;
    canvas.height = 1;
    const context = canvas.getContext('2d')!;
    context.drawImage(image, image.naturalWidth / 2, image.naturalHeight / 2, 1, 1, 0, 0, 1, 1);
    return [...context.getImageData(0, 0, 1, 1).data];
  });
  expect(pixels.slice(0, 3)).toEqual([18, 36, 60]);
  const text = page.locator('.slide-canvas img[alt="TextBox"]');
  await expect(text).toHaveCount(2);
  const titleSource = await text.first().getAttribute('src');
  const bodySource = await text.nth(1).getAttribute('src');
  const titleSvg = Buffer.from(titleSource!.split(',')[1], 'base64').toString();
  const bodySvg = Buffer.from(bodySource!.split(',')[1], 'base64').toString();
  expect(titleSvg).toContain('Reusable background 1');
  expect(titleSvg).toContain('text-align:center');
  expect(bodySvg).toContain('text-align:right');
  expect(bodySvg).toContain('white-space:pre-wrap');
  await expect.poll(() => text.evaluateAll((images: HTMLImageElement[]) => images.every(image => image.complete && image.naturalWidth > 0))).toBe(true);
  await page.screenshot({ path: testInfo.outputPath('pptx-background-and-wrapped-text.png') });

  await page.getByRole('button', { name: /^Slide 20:/ }).click();
  await expect.poll(() => page.locator('.slide-list').evaluate(list => list.scrollTop)).toBeGreaterThan(0);
  await expect(raster).toBeVisible();
  await expect.poll(async () => {
    const source = await text.first().getAttribute('src');
    return Buffer.from(source!.split(',')[1], 'base64').toString();
  }).toContain('Reusable background 20');
  await expect.poll(() => raster.evaluate((image: HTMLImageElement) => image.complete && image.naturalWidth > 0)).toBe(true);
  await page.reload();
  await expect(page.locator('.slide-thumbnail')).toHaveCount(20);
  await expect(raster).toBeVisible();
  const reloaded = (await (await request.get(`/api/v1/documents/${upload.document_id}`, { headers })).json()).data;
  expect(reloaded.version).toBe(1);
  expect(reloaded.content.slides).toEqual(document.content.slides);
});

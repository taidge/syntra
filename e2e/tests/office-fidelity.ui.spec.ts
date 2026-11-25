import { test, expect, APIRequestContext, Page } from '@playwright/test';
import { readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { registerUser, uniqueEmail, uniqueUsername } from './helpers';

test.describe('Independent Office fidelity fixtures @ui', () => {
  test.setTimeout(120_000);
  test.use({ viewport: { width: 1600, height: 1100 }, locale: 'en-US' });

  async function imported(page: Page, request: APIRequestContext, extension: string) {
    const auth = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Fidelity@Test12345');
    const headers = { Authorization: `Bearer ${auth.token}` };
    const user = (await (await request.get('/api/v1/auth/me', { headers })).json()).data;
    await page.addInitScript(({ token, user }) => {
      localStorage.setItem('syntra.auth', JSON.stringify({ token, user }));
      localStorage.setItem('locale', 'en');
      localStorage.setItem('theme', 'light');
    }, { token: auth.token, user });
    const response = await request.post('/api/v1/files/upload', {
      headers, multipart: { file: { name: `render-audit.${extension}`, mimeType: 'application/octet-stream',
        buffer: await readFile(resolve(__dirname, `../../crates/export/tests/fixtures/render-audit.${extension}`)) } },
    });
    expect(response.status(), await response.text()).toBe(201);
    const file = (await response.json()).data;
    const document = (await (await request.get(`/api/v1/documents/${file.document_id}`, { headers })).json()).data;
    return { headers, file, document };
  }

  async function exportedPdf(page: Page, request: APIRequestContext, documentId: string, headers: Record<string, string>, output: string) {
    const response = await request.post(`/api/v1/documents/${documentId}/export/pdf`, { headers, timeout: 90_000 });
    expect(response.status(), response.ok() ? undefined : await response.text()).toBe(200);
    const bytes = await response.body();
    await writeFile(output, bytes);
    // Inspect the real generated PDF, including page placement and bitmap
    // operators. A successful response alone cannot detect clipping or loss.
    // Parse the artifact independently of the app's browser worker lifecycle.
    // This also keeps PDF decoding out of the editor's page/context.
    const canvas = require('@napi-rs/canvas');
    Object.assign(globalThis, { DOMMatrix: canvas.DOMMatrix, ImageData: canvas.ImageData, Path2D: canvas.Path2D });
    const pdfjs = await import(pathToFileURL(require.resolve('pdfjs-dist/legacy/build/pdf.mjs')).href);
    const task = pdfjs.getDocument({ data: new Uint8Array(bytes),
      cMapUrl: pathToFileURL(resolve(__dirname, '../node_modules/pdfjs-dist/cmaps/')).href + '/', cMapPacked: true,
      standardFontDataUrl: pathToFileURL(resolve(__dirname, '../node_modules/pdfjs-dist/standard_fonts/')).href + '/' });
    const document = await task.promise;
    try {
      const pages = [];
      let images = 0;
      for (let index = 1; index <= document.numPages; index++) {
        const page = await document.getPage(index);
        const content = await page.getTextContent();
        pages.push(content.items.map((item: any) => item.str || '').join('').replace(/\s+/g, ''));
        const operators = await page.getOperatorList();
        images += operators.fnArray.filter((op: number) => [pdfjs.OPS.paintImageXObject, pdfjs.OPS.paintInlineImageXObject, pdfjs.OPS.paintImageXObjectRepeat].includes(op)).length;
      }
      return { pages, images };
    } finally { await task.destroy(); }
  }

  test('DOCX preserves table visibility, merges, explicit pages and running text', async ({ page, request }, testInfo) => {
    const { headers, file, document } = await imported(page, request, 'docx');
    await page.goto(`/writer/${file.document_id}`);
    const editor = page.getByRole('textbox', { name: 'Document body', exact: true });
    await expect(editor).toContainText('中英文文档排版核对');
    const titleCell = editor.locator('td').filter({ hasText: '合并标题' });
    await expect(titleCell).toHaveAttribute('colspan', '3');
    await expect(titleCell).toHaveCSS('vertical-align', 'middle');
    await expect(titleCell).toHaveCSS('padding-top', '6.66667px');
    await expect(titleCell).toHaveCSS('padding-left', '6.66667px');
    expect(await titleCell.evaluate(element => {
      const color = getComputedStyle(element).backgroundColor;
      return color !== 'rgba(0, 0, 0, 0)' && color !== 'rgb(255, 255, 255)';
    })).toBe(true);
    await expect(editor.locator('td').filter({ hasText: '跨两行' })).toHaveAttribute('rowspan', '2');
    const figure = editor.locator('figure').first();
    await expect.poll(() => figure.evaluate(element => {
      const picture = element.querySelector('img')!;
      if (!picture.complete || !picture.naturalWidth) return Infinity;
      const frame = element.getBoundingClientRect();
      const image = picture.getBoundingClientRect();
      return Math.abs(frame.left + frame.width / 2 - image.left - image.width / 2);
    })).toBeLessThan(1.5);
    expect(document.content.body.some((block: any) => block.type === 'PageBreak')).toBe(true);
    // The source's hard page break starts this heading at the 19.05mm top
    // margin. A tiny pagination decoration must not add its heading margin.
    await expect.poll(() => page.evaluate(() => {
      const heading = [...document.querySelectorAll('.writer-body h1')]
        .find(element => element.textContent === '第二页表格与段落');
      const secondPage = document.querySelectorAll('.writer-physical-page')[1];
      return heading && secondPage
        ? Math.abs(heading.getBoundingClientRect().top - secondPage.getBoundingClientRect().top - 72)
        : Infinity;
    })).toBeLessThan(1);
    await page.screenshot({ path: testInfo.outputPath('docx-editor.png') });
    const pdf = await exportedPdf(page, request, file.document_id, headers, testInfo.outputPath('docx.pdf'));
    expect(pdf.pages).toHaveLength(2);
    expect(pdf.pages[0]).not.toContain('第二页表格与段落');
    expect(pdf.pages[1]).toContain('第二页表格与段落');
    expect(pdf.pages[1]).toContain('合并标题');
    for (const text of pdf.pages) expect(text).toContain('文档渲染核对');
    expect(pdf.pages[0]).toContain('页码1');
    expect(pdf.pages[1]).toContain('页码2');
    expect(pdf.images).toBeGreaterThan(0);
    await editor.click();
    await page.keyboard.press('Control+End');
    await page.keyboard.type(' 已编辑');
    await page.keyboard.press('Control+s');
    await expect.poll(async () => {
      const saved = (await (await request.get(`/api/v1/documents/${file.document_id}`, { headers })).json()).data;
      return { version: saved.version >= 2, language: saved.content.professional.language,
        eastAsia: saved.content.professional.east_asia_language };
    }, { timeout: 15_000 }).toEqual({ version: true, language: 'en-US', eastAsia: 'en-US' });
    await page.reload();
    await expect(editor).toContainText('已编辑');
  });

  test('XLSX displays imported formats, text layout and drawings and exports intact columns', async ({ page, request }, testInfo) => {
    const { headers, file } = await imported(page, request, 'xlsx');
    await page.goto(`/calc/${file.document_id}`);
    await page.locator('.sheet-grid').waitFor({ timeout: 30_000 });
    const cell = (address: string) => page.getByRole('button', { name: address, exact: true });
    await expect(cell('B5')).toHaveText('2026-11-01');
    await expect(cell('C5')).toHaveText('¥1,234.56');
    await expect(cell('D5')).toHaveText('5.0%');
    await expect(cell('C7')).toHaveText('(¥300.00)');
    await expect(cell('C6').locator('xpath=ancestor::td[1]')).toHaveCSS('background-color', 'rgb(220, 252, 231)');
    await expect(cell('C6')).toHaveCSS('background-color', 'rgba(0, 0, 0, 0)');
    expect(await cell('F5').evaluate(element => ['normal', 'pre-wrap', 'break-spaces'].includes(getComputedStyle(element).whiteSpace))).toBe(true);
    const fontSize = await cell('C5').evaluate(element => parseFloat(getComputedStyle(element).fontSize));
    expect(fontSize).toBeCloseTo(11 * 4 / 3, 1);
    const drawing = page.locator('img.calc-sheet-drawings[alt="Worksheet drawings"]');
    await expect(drawing).toBeVisible();
    const drawingSvg = await drawing.evaluate((image: HTMLImageElement) => decodeURIComponent(escape(atob(image.src.split(',')[1]))));
    expect(drawingSvg).toContain('<image');
    expect(drawingSvg).toContain('每月活动数量');
    expect(drawingSvg).toContain('worksheet-chart-background');
    await cell('A5').dblclick();
    await page.getByRole('textbox', { name: 'Edit A5', exact: true }).fill('南京培训（已编辑）');
    await page.keyboard.press('Control+s');
    await expect.poll(async () => {
      const saved = (await (await request.get(`/api/v1/documents/${file.document_id}`, { headers })).json()).data.content;
      return { text: saved.sheets[0].cells.find(([position]: any) => position.row === 4 && position.col === 0)?.[1].value.value,
        images: saved.sheets[0].images?.length, charts: saved.sheets[0].charts?.length };
    }).toEqual({ text: '南京培训（已编辑）', images: 1, charts: 1 });
    await page.reload();
    await expect(cell('C5')).toHaveText('¥1,234.56');
    await expect(drawing).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath('xlsx-editor.png') });
    const pdf = await exportedPdf(page, request, file.document_id, headers, testInfo.outputPath('xlsx.pdf'));
    expect(pdf.pages.length).toBeLessThanOrEqual(4);
    const text = pdf.pages.join('');
    expect(text).toContain('2026-11-01');
    expect(text).toContain('¥1,234.56');
    expect(text).toContain('5.0%');
    expect(text).toContain('净额（元）');
    expect(text).toContain('说明');
    expect(pdf.images).toBeGreaterThan(0);
  });

  test('PPTX retains table geometry and chart meaning in both editor and PDF', async ({ page, request }, testInfo) => {
    const { headers, file, document } = await imported(page, request, 'pptx');
    expect(document.content.slides).toHaveLength(4);
    const table = document.content.slides[1].elements.find((element: any) => element.type === 'Table');
    expect(table.cells[0].column_span).toBe(3);
    await page.goto(`/slides/${file.document_id}`);
    await page.locator('.slide-canvas').waitFor({ timeout: 30_000 });
    await expect(page.locator('.slide-thumbnail')).toHaveCount(4);
    await page.locator('.slide-thumbnail').nth(2).click();
    await expect.poll(async () => await page.locator('.slide-canvas img').evaluateAll((images: HTMLImageElement[]) => images.length > 0 && images.every(image => image.complete && image.naturalWidth > 0))).toBe(true);
    const svg = await page.locator('.slide-canvas img').evaluateAll((images: HTMLImageElement[]) => images.filter(image => image.src.startsWith('data:image/svg+xml;base64,')).map(image => decodeURIComponent(escape(atob(image.src.split(',')[1])))).join(''));
    expect(svg).toContain('收入对比');
    expect(svg).toContain('计划 Plan');
    expect(svg).toContain('实际 Actual');
    expect(svg).toContain('50%');
    await page.screenshot({ path: testInfo.outputPath('pptx-chart-editor.png') });
    const pdf = await exportedPdf(page, request, file.document_id, headers, testInfo.outputPath('pptx.pdf'));
    expect(pdf.pages).toHaveLength(4);
    expect(pdf.pages[1]).toContain('季度报告');
    expect(pdf.pages[2]).toContain('收入对比');
    expect(pdf.pages[2]).toContain('计划Plan');
    expect(pdf.pages[2]).toContain('50%');
    expect(pdf.images).toBeGreaterThan(0);
  });
});

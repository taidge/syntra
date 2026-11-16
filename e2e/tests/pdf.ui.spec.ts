import { test, expect, APIRequestContext, Page } from '@playwright/test';
import { PDFDocument, StandardFonts, rgb } from 'pdf-lib';
import { readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { registerUser, uniqueEmail, uniqueUsername, createDocument } from './helpers';

test.describe('PDF workflows @ui', () => {
  test.setTimeout(180_000);

  async function account(page: Page, request: APIRequestContext) {
    const auth = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');
    const headers = { Authorization: `Bearer ${auth.token}` };
    const user = (await (await request.get('/api/v1/auth/me', { headers })).json()).data;
    await page.addInitScript(({ token, user }) => {
      localStorage.setItem('contrix.auth', JSON.stringify({ token, user }));
      localStorage.setItem('locale', 'en');
    }, { token: auth.token, user });
    return { ...auth, headers };
  }

  async function sourcePdf() {
    const document = await PDFDocument.create();
    const font = await document.embedFont(StandardFonts.Helvetica);
    for (const text of ['PDF source first page', 'PDF source second page']) {
      const page = document.addPage([612, 792]);
      page.drawText(text, { x: 60, y: 700, size: 24, font, color: rgb(.1, .2, .4) });
    }
    return Buffer.from(await document.save());
  }

  async function upload(request: APIRequestContext, headers: Record<string, string>, buffer: Buffer, name: string, parent?: string) {
    const response = await request.post('/api/v1/files/upload', { headers, multipart: { file: { name, mimeType: 'application/pdf', buffer }, ...(parent ? { parent_id: parent } : {}) } });
    expect(response.status(), await response.text()).toBe(201);
    return (await response.json()).data;
  }

  test('PDF preview, Chinese annotations, page editing, original download and Writer conversion', async ({ page, request }, testInfo) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    const auth = await account(page, request);
    const original = await sourcePdf();
    const inputPath = testInfo.outputPath('PDF workflow.pdf');
    await writeFile(inputPath, original);
    await page.goto('/files');
    const picker = page.waitForEvent('filechooser');
    await page.getByRole('button', { name: 'Upload', exact: true }).click();
    const uploaded = page.waitForResponse(response => response.url().endsWith('/api/v1/files/upload') && response.request().method() === 'POST');
    await (await picker).setFiles(inputPath);
    const uploadResponse = await uploaded;
    expect(uploadResponse.status()).toBe(201);
    const file = (await uploadResponse.json()).data;
    await page.getByRole('link', { name: 'PDF workflow.pdf', exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`/pdf/${file.id}$`));
    await expect(page.locator('.textLayer')).toContainText('PDF source first page');
    await expect(page.locator('.pdf-page-count')).toHaveText('/ 2');
    const initial = await request.get(`/api/v1/files/${file.id}/pdf`, { headers: auth.headers });
    const originalEtag = initial.headers().etag;
    expect(initial.headers()['x-pdf-can-edit']).toBe('true');

    await page.getByRole('searchbox', { name: 'Find in PDF', exact: true }).fill('second');
    await page.getByRole('button', { name: 'Find', exact: true }).click();
    await expect(page.getByRole('spinbutton', { name: 'PDF page number', exact: true })).toHaveValue('2');
    await expect(page.locator('.textLayer')).toContainText('PDF source second page');
    await page.getByRole('textbox', { name: 'Text to add', exact: true }).fill('PDF annotation 中文');
    await page.getByRole('button', { name: 'Add text', exact: true }).click();
    await page.locator('.pdf-input-layer').click({ position: { x: 70, y: 150 } });
    await expect(page.locator('.textLayer')).toContainText('PDF annotation 中文', { timeout: 30_000 });
    await page.getByRole('button', { name: 'Rotate right', exact: true }).click();
    await page.getByRole('button', { name: 'Save PDF', exact: true }).click();
    await expect(page.locator('.pdf-message')).toHaveText('PDF saved.', { timeout: 30_000 });
    let current = await request.get(`/api/v1/files/${file.id}/pdf`, { headers: auth.headers });
    expect(current.headers().etag).not.toBe(originalEtag);
    let parsed = await PDFDocument.load(await current.body());
    expect(parsed.getPage(1).getRotation().angle).toBe(90);

    await page.getByRole('button', { name: 'Move page earlier', exact: true }).click();
    await expect(page.getByRole('spinbutton', { name: 'PDF page number', exact: true })).toHaveValue('1');
    await page.getByRole('button', { name: 'Delete page', exact: true }).click();
    await expect(page.locator('.pdf-page-count')).toHaveText('/ 1');
    await page.getByRole('button', { name: 'Undo', exact: true }).click();
    await expect(page.locator('.pdf-page-count')).toHaveText('/ 2');
    await page.getByRole('button', { name: 'Redo', exact: true }).click();
    await expect(page.locator('.pdf-page-count')).toHaveText('/ 1');
    await page.getByRole('button', { name: 'Undo', exact: true }).click();
    await expect(page.locator('.pdf-page-count')).toHaveText('/ 2');
    await page.getByRole('button', { name: 'Save PDF', exact: true }).click();
    await expect(page.locator('.pdf-message')).toHaveText('PDF saved.');
    await page.reload();
    await expect(page.locator('.textLayer')).toContainText('PDF annotation 中文');
    current = await request.get(`/api/v1/files/${file.id}/pdf`, { headers: auth.headers });
    parsed = await PDFDocument.load(await current.body());
    expect(parsed.getPageCount()).toBe(2);
    expect(parsed.getPage(0).getRotation().angle).toBe(90);
    const firstFontBytes = (await current.body()).length;
    await page.getByRole('textbox', { name: 'Text to add', exact: true }).fill('再次批注');
    await page.getByRole('button', { name: 'Add text', exact: true }).click();
    await page.locator('.pdf-input-layer').click({ position: { x: 110, y: 210 } });
    await expect(page.locator('.textLayer')).toContainText('再次批注', { timeout: 30_000 });
    await page.getByRole('button', { name: 'Save PDF', exact: true }).click();
    await expect(page.locator('.pdf-message')).toHaveText('PDF saved.');
    const secondFontBytes = await (await request.get(`/api/v1/files/${file.id}/pdf`, { headers: auth.headers })).body();
    expect(secondFontBytes.length).toBeLessThan(firstFontBytes + 512 * 1024);
    const downloaded = page.waitForEvent('download');
    await page.getByRole('button', { name: 'Download original', exact: true }).click();
    expect(await readFile((await (await downloaded).path())!)).toEqual(original);
    await page.screenshot({ path: testInfo.outputPath('pdf-edited-and-reopened.png') });
    await page.getByRole('button', { name: 'Convert to Writer', exact: true }).click();
    await expect(page).toHaveURL(/\/writer\//, { timeout: 30_000 });
    const body = page.getByRole('textbox', { name: 'Document body', exact: true });
    await expect(body).toContainText('PDF annotation 中文');
    await expect(body).toContainText('再次批注');
    await expect(body).toContainText('PDF source first page');
    await expect(body).toContainText('PDF source second page');
    expect(await (await request.get(`/api/v1/files/${file.id}/pdf?version=0`, { headers: auth.headers })).body()).toEqual(original);
    expect(errors).toEqual([]);
  });

  test('PDF saves enforce permissions and ETag conflicts without losing the original', async ({ page, request }) => {
    const auth = await account(page, request);
    const original = await sourcePdf();
    const file = await upload(request, auth.headers, original, 'CAS.pdf');
    const path = `/api/v1/files/${file.id}/pdf`;
    const source = await request.get(path, { headers: auth.headers });
    const etag = source.headers().etag;
    const other = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');
    const unauthorized = { Authorization: `Bearer ${other.token}` };
    expect((await request.get(path, { headers: unauthorized })).status()).toBe(404);
    expect((await request.put(path, { headers: { ...unauthorized, 'Content-Type': 'application/pdf', 'If-Match': etag }, data: original })).status()).toBe(404);
    expect((await request.post(`${path}/convert`, { headers: unauthorized, data: { expected_etag: etag } })).status()).toBe(404);
    expect((await request.put(path, { headers: { ...auth.headers, 'Content-Type': 'application/pdf' }, data: original })).status()).toBe(400);
    expect((await request.put(path, { headers: { ...auth.headers, 'Content-Type': 'application/pdf', 'If-Match': etag }, data: Buffer.from('not a PDF') })).status()).toBe(400);
    const changed = await PDFDocument.load(original);
    changed.getPage(0).setRotation({ type: 'degrees', angle: 90 } as any);
    const bytes = Buffer.from(await changed.save());
    const results = await Promise.all([0, 1].map(() => request.put(path, { headers: { ...auth.headers, 'Content-Type': 'application/pdf', 'If-Match': etag }, data: bytes })));
    expect(results.map(result => result.status()).sort()).toEqual([200, 409]);
    expect(await (await request.get(`${path}?version=0`, { headers: auth.headers })).body()).toEqual(original);
    expect(await (await request.get(path, { headers: auth.headers })).body()).toEqual(bytes);
    const currentEtag = (await request.get(path, { headers: auth.headers })).headers().etag;
    const restored = await request.post(`${path}/restore`, { headers: auth.headers, data: { version_number: 0, expected_etag: currentEtag } });
    expect(restored.status(), await restored.text()).toBe(200);
    expect((await restored.json()).data.version_number).toBe(2);
    expect(await (await request.get(path, { headers: auth.headers })).body()).toEqual(original);
    expect((await request.post(`${path}/restore`, { headers: auth.headers, data: { version_number: 0, expected_etag: currentEtag } })).status()).toBe(409);
    const roomResponse = await request.post('/api/v1/rooms', { headers: auth.headers, data: { name: 'PDF read-only room' } });
    expect(roomResponse.status()).toBe(201);
    const room = (await roomResponse.json()).data;
    expect((await request.post(`/api/v1/rooms/${room.id}/members`, { headers: auth.headers, data: { user_id: other.userId, role: 'viewer' } })).status()).toBe(201);
    const folderResponse = await request.post('/api/v1/folders', { headers: auth.headers, data: { name: 'Shared PDFs', room_id: room.id } });
    expect(folderResponse.status()).toBe(201);
    const shared = await upload(request, auth.headers, original, 'Shared PDF.pdf', (await folderResponse.json()).data.id);
    const sharedPath = `/api/v1/files/${shared.id}/pdf`;
    const viewed = await request.get(sharedPath, { headers: unauthorized });
    expect(viewed.status()).toBe(200);
    expect(viewed.headers()['x-pdf-can-edit']).toBe('false');
    expect((await request.put(sharedPath, { headers: { ...unauthorized, 'Content-Type': 'application/pdf', 'If-Match': viewed.headers().etag }, data: bytes })).status()).toBe(403);
    expect((await request.post(`${sharedPath}/restore`, { headers: unauthorized, data: { expected_etag: viewed.headers().etag, version_number: 0 } })).status()).toBe(403);
    const privateCopy = await request.post(`${sharedPath}/convert`, { headers: unauthorized, data: { expected_etag: viewed.headers().etag } });
    expect(privateCopy.status(), await privateCopy.text()).toBe(201);
    const copyId = (await privateCopy.json()).data.document_id;
    expect((await request.get(`/api/v1/documents/${copyId}`, { headers: auth.headers })).status()).toBe(404);
    expect((await request.get(`/api/v1/documents/${copyId}`, { headers: unauthorized })).status()).toBe(200);
  });

  test('password-protected PDF can be read while editing remains disabled', async ({ page, request }) => {
    const auth = await account(page, request);
    const bytes = await readFile(resolve(__dirname, '../../crates/export/tests/fixtures/password-reader.pdf'));
    const file = await upload(request, auth.headers, bytes, 'Password reader.pdf');
    const response = await request.get(`/api/v1/files/${file.id}/pdf`, { headers: auth.headers });
    expect(response.status(), await response.text()).toBe(200);
    expect(response.headers()['x-pdf-encrypted']).toBe('true');
    await page.goto(`/pdf/${file.id}`);
    await expect(page.getByLabel('PDF password', { exact: true })).toBeVisible();
    await page.getByLabel('PDF password', { exact: true }).fill('reader123');
    await page.getByRole('button', { name: 'Unlock PDF', exact: true }).click();
    await expect(page.locator('.textLayer')).toContainText('Password protected PDF reader');
    await expect(page.getByRole('button', { name: 'Add text', exact: true })).toBeDisabled();
    await expect(page.getByRole('button', { name: 'Save PDF', exact: true })).toBeDisabled();
    await expect(page.getByRole('button', { name: 'Convert to Writer', exact: true })).toBeDisabled();
  });

  test('PDF background conversion exports DOCX and preserves original PDF bytes', async ({ page, request }) => {
    const auth = await account(page, request);
    const original = await sourcePdf();
    const file = await upload(request, auth.headers, original, 'Background source.pdf');
    for (const target_format of ['docx', 'pdf']) {
      const created = await request.post('/api/v1/convert', { headers: auth.headers, data: { file_id: file.id, target_format } });
      expect(created.status(), await created.text()).toBe(201);
      const job = (await created.json()).data;
      await expect.poll(async () => (await (await request.get(`/api/v1/convert/${job.id}`, { headers: auth.headers })).json()).data.status, { timeout: 30_000 }).toBe('completed');
      const downloaded = await request.get(`/api/v1/convert/${job.id}/download`, { headers: auth.headers });
      expect(downloaded.status()).toBe(200);
      const bytes = await downloaded.body();
      if (target_format === 'pdf') expect(bytes).toEqual(original);
      else {
        expect(downloaded.headers()['content-disposition']).toContain('.docx');
        expect(bytes.subarray(0, 4)).toEqual(Buffer.from([0x50, 0x4b, 0x03, 0x04]));
        expect(bytes.includes(Buffer.from('word/document.xml'))).toBe(true);
      }
    }
  });

  test('all four editors export real PDFs and Chinese text converts back to Writer', async ({ page, request }, testInfo) => {
    const auth = await account(page, request);
    const image = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4//8/AAX+Av4N70a4AAAAAElFTkSuQmCC';
    for (const kind of ['writer', 'calc', 'slides', 'whiteboard'] as const) {
      const { response, body } = await createDocument(request, auth.token, `PDF ${kind}`, kind);
      expect(response.status()).toBe(201);
      const document = body.data;
      const content = document.content;
      const text = `PDF ${kind} 中文导出`;
      if (kind === 'writer') {
        content.body = [{ type: 'Paragraph', id: '11111111-1111-4111-8111-111111111111', runs: [{ text, font_size: 16 }], alignment: 'Left', indent_level: 0, spacing: { before: 0, after: 8, line_height: 1.5 } }, { type: 'PageBreak' }, { type: 'Paragraph', id: '11111111-1111-4111-8111-111111111112', runs: [{ text: 'Writer second page' }], alignment: 'Left', indent_level: 0, spacing: { before: 0, after: 0, line_height: 1 } }];
      } else if (kind === 'slides') {
        content.metadata.slide_width = 1280; content.metadata.slide_height = 720;
        content.slides = [0, 1].map(index => ({ id: `22222222-2222-4222-8222-22222222222${index}`, layout: 'Blank', notes: '', background: { Solid: index ? '#F3F4F6' : '#172554' }, animations: [], transition: null, elements: [
          { type: 'TextBox', id: `33333333-3333-4333-8333-33333333333${index}`, x: 100, y: 100, width: 1000, height: 100, rotation: 0, padding: 0, border: null, background_color: null, content: [{ text: index ? 'Second slide' : text, font_size: 32, color: index ? '#111827' : '#FFFFFF' }] },
          { type: 'Image', id: `44444444-4444-4444-8444-44444444444${index}`, x: 100, y: 250, width: 200, height: 100, rotation: 0, url: image, alt_text: 'PDF image', crop: null },
        ] }));
      } else if (kind === 'whiteboard') {
        content.layers[0].elements = [{ type: 'Text', id: '55555555-5555-4555-8555-555555555555', x: 80, y: 80, text, font_size: 28, font_family: 'Arial', color: '#172554', bold: false, italic: false }];
      } else {
        content.sheets[0].column_widths = { 0: 240 };
        content.sheets[0].cells = [[{ row: 0, col: 0 }, { value: { type: 'Text', value: text } }], [{ row: 1, col: 0 }, { value: { type: 'Number', value: 42 } }]];
      }
      const saved = await request.put(`/api/v1/documents/${document.id}`, { headers: auth.headers, data: { content, expected_version: 1 } });
      expect(saved.status(), await saved.text()).toBe(200);
      const exported = await request.post(`/api/v1/documents/${document.id}/export/pdf`, { headers: auth.headers, timeout: 60_000 });
      expect(exported.status(), await exported.text()).toBe(200);
      expect(exported.headers()['content-type']).toContain('application/pdf');
      const bytes = await exported.body();
      expect(bytes.subarray(0, 5).toString()).toBe('%PDF-');
      const parsed = await PDFDocument.load(bytes);
      expect(parsed.getPageCount()).toBe(kind === 'slides' || kind === 'writer' ? 2 : 1);
      const file = await upload(request, auth.headers, bytes, `${kind}-export.pdf`);
      const preview = await request.get(`/api/v1/files/${file.id}/pdf`, { headers: auth.headers });
      expect(preview.status(), await preview.text()).toBe(200);
      const converted = await request.post(`/api/v1/files/${file.id}/pdf/convert`, { headers: auth.headers, data: { expected_etag: preview.headers().etag } });
      expect(converted.status(), await converted.text()).toBe(201);
      const writer = (await (await request.get(`/api/v1/documents/${(await converted.json()).data.document_id}`, { headers: auth.headers })).json()).data;
      const extracted = writer.content.body.flatMap((block: any) => block.runs?.map((run: any) => run.text) || []).join('');
      expect(extracted.replace(/\s/g, '')).toContain(text.replace(/\s/g, ''));
      if (kind === 'slides') {
        await page.goto(`/pdf/${file.id}`);
        await expect(page.locator('.textLayer')).toContainText(text);
        await page.screenshot({ path: testInfo.outputPath('slides-exported-pdf.png') });
      }
    }
  });

  test('scanned PDF converts with local OCR and keeps the source unchanged', async ({ page, request }, testInfo) => {
    const auth = await account(page, request);
    const png = await page.evaluate(() => {
      const canvas = document.createElement('canvas'); canvas.width = 1400; canvas.height = 1800;
      const context = canvas.getContext('2d')!; context.fillStyle = '#FFFFFF'; context.fillRect(0, 0, canvas.width, canvas.height);
      context.fillStyle = '#000000'; context.font = 'bold 54px Arial'; context.fillText('SCANNED PDF 2026', 110, 200);
      context.font = '40px Arial'; context.fillText('Review and edit this document.', 110, 310);
      return canvas.toDataURL('image/png').split(',')[1];
    });
    const document = await PDFDocument.create();
    const image = await document.embedPng(Buffer.from(png, 'base64'));
    document.addPage([700, 900]).drawImage(image, { x: 0, y: 0, width: 700, height: 900 });
    const original = Buffer.from(await document.save());
    const file = await upload(request, auth.headers, original, 'Scanned.pdf');
    await page.goto(`/pdf/${file.id}`);
    await expect(page.locator('.pdf-page-count')).toHaveText('/ 1');
    await expect(page.locator('.pdf-page-wrap canvas')).toBeVisible();
    await page.getByRole('button', { name: 'Convert to Writer', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Cancel OCR', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Cancel OCR', exact: true }).click();
    await expect(page.locator('.pdf-message')).toContainText('cancelled');
    await expect(page.getByRole('button', { name: 'Convert to Writer', exact: true })).toBeEnabled();
    const converted = page.waitForResponse(response => response.url().endsWith(`/files/${file.id}/pdf/convert`) && response.request().method() === 'POST', { timeout: 120_000 });
    await page.getByRole('button', { name: 'Convert to Writer', exact: true }).click();
    const result = await converted;
    expect(result.status()).toBe(201);
    expect(result.request().postDataJSON().ocr_pages).toHaveLength(1);
    await expect(page).toHaveURL(/\/writer\//, { timeout: 120_000 });
    await expect(page.getByRole('textbox', { name: 'Document body', exact: true })).toContainText('SCANNED PDF 2026');
    await page.screenshot({ path: testInfo.outputPath('scanned-pdf-converted-to-writer.png') });
    expect(await (await request.get(`/api/v1/files/${file.id}/pdf`, { headers: auth.headers })).body()).toEqual(original);
  });
});

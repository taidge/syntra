import { test, expect } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { authHeaders, createDocument, registerUser, uniqueEmail, uniqueUsername } from './helpers';

test.use({ locale: 'en-US', serviceWorkers: 'block' });

test('Writer imports inherited DOCX fonts, indentation and Chinese numbering @ui', async ({ page, request }, testInfo) => {
  const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Fidelity@12345');
  const upload = await request.post('/api/v1/files/upload', {
    headers: authHeaders(user.token), multipart: { file: {
      name: 'writer-typography.docx', mimeType: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
      buffer: await readFile(resolve(__dirname, '../../crates/export/tests/fixtures/writer-typography-grid.docx')),
    } },
  });
  expect(upload.status()).toBe(201);
  const file = (await upload.json()).data;
  expect(file.document_id).toBeTruthy();
  const imported = await request.get(`/api/v1/documents/${file.document_id}`, { headers: authHeaders(user.token) });
  expect(imported.ok()).toBe(true);
  const document = (await imported.json()).data.content;
  expect(document.body[0].runs[0].font_size).toBe(16);
  expect(document.body[1].runs[0].font_family).toContain('SimSun');
  expect(document.body[1].runs[0].bold).toBe(false);
  expect(document.body[3].type).toBe('Paragraph');
  await page.addInitScript(auth => {
    localStorage.setItem('locale', 'en');
    localStorage.setItem('theme', 'dark');
    localStorage.setItem('syntra.auth', JSON.stringify({ token: auth.token, user: {
      id: auth.userId, username: auth.username, email: auth.email, role: 'editor',
    } }));
  }, user);
  await page.goto('/files');
  await page.getByRole('link', { name: 'writer-typography.docx', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/writer/${file.document_id}$`));
  const editor = page.getByRole('textbox', { name: 'Document body', exact: true });
  await expect(editor).toContainText('排版保真验证');
  await expect(editor).toContainText('二、');
  await expect(editor.locator('p').filter({ hasText: '排版保真验证' })).toHaveCSS('text-align', 'center');
  await expect(editor.locator('p').filter({ hasText: '中文段落保留原稿' })).toHaveCSS('text-indent', '32px');
  await expect(editor.locator('p').filter({ hasText: '中文段落保留原稿' })).toHaveCSS('line-height', '31.2px');
  await expect(editor.locator('li')).toHaveCount(0);
  await expect(page.locator('.writer-page')).toHaveCSS('background-color', 'rgb(255, 255, 255)');
  const text = await editor.innerText();
  expect(text.indexOf('特殊说明')).toBeLessThan(text.indexOf('1. 自愿原则'));
  await page.screenshot({ path: testInfo.outputPath('writer-import-fidelity.png') });
});

test('Writer preserves paper, typography and inline block order while editing @ui', async ({ page, request }, testInfo) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Fidelity@12345');
  const { response, body } = await createDocument(request, user.token, 'Writer fidelity', 'writer');
  expect(response.status()).toBe(201);
  const id = body.data.id;
  const content = body.data.content;
  const run = (text: string, bold = false, fontSize = 12) => ({
    text, bold, italic: false, underline: false, strikethrough: false,
    font_family: 'SimSun', font_size: fontSize, color: '#000000',
    background_color: null, link: null, superscript: false, subscript: false,
  });
  const paragraph = (blockId: string, text: string, alignment = 'Left', bold = false, fontSize = 12) => ({
    type: 'Paragraph', id: blockId, alignment, indent_level: 0,
    spacing: { before: 0, after: 8, line_height: 1.5 }, runs: [run(text, bold, fontSize)],
  });
  const titleId = 'e9f82622-e285-4df3-ac14-652493e90b52';
  const bodyId = 'bbf7fb2b-dcc1-4eb0-86dd-688bc0b18a50';
  content.body = [
    paragraph(titleId, 'VR 场景创意赛活动培训方案', 'Center', true, 16),
    paragraph(bodyId, '项目背景与价值，保持原稿的中文正文样式。'),
    { type: 'List', id: '0e7818fe-8f06-4630-8de0-c1e17d98d44d', ordered: false,
      items: [{ runs: [run('自愿参加的第一条说明')], children: [] }] },
    { type: 'Table', id: 'bcc8de16-66ed-4cf2-8577-31a71a83477d', column_widths: [240],
      rows: [{ cells: [{ col_span: 1, row_span: 1, background_color: null,
        blocks: [paragraph('6da46134-8e64-4d62-9f0d-7c697a42aa6f', '表格应位于正文中间')] }] }] },
    { type: 'Image', id: '58d5bc16-5ae2-4178-ab6f-622a8a94c693', alignment: 'Left',
      url: 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aZZkAAAAASUVORK5CYII=',
      alt_text: 'Embedded fixture', width: 32, height: 32 },
    paragraph('d4fc03f6-a145-44b3-ae64-9993481367d0', '文档末尾'),
  ];
  content.professional.paragraph_formats = [
    { block_id: bodyId, first_line_indent_chars: 2, first_line_indent_pt: 24,
      left_indent_pt: null, right_indent_pt: null, line_height_pt: null, numbering_label: null, alignment: null },
  ];
  const seeded = await request.put(`/api/v1/documents/${id}`, {
    headers: authHeaders(user.token), data: { content, expected_version: body.data.version },
  });
  expect(seeded.ok()).toBe(true);
  await page.addInitScript(auth => {
    localStorage.setItem('locale', 'en');
    localStorage.setItem('theme', 'dark');
    localStorage.setItem('syntra.auth', JSON.stringify({ token: auth.token, user: {
      id: auth.userId, username: auth.username, email: auth.email, role: 'editor',
    } }));
  }, user);
  await page.goto(`/writer/${id}`);
  const editor = page.getByRole('textbox', { name: 'Document body', exact: true });
  await expect(editor).toContainText('VR 场景创意赛活动培训方案');
  await expect(page.locator('.save-status')).toHaveText('All changes saved');
  const title = editor.locator('p').filter({ hasText: 'VR 场景创意赛活动培训方案' });
  const paragraphBody = editor.locator('p').filter({ hasText: '项目背景与价值' });
  await expect(title).toHaveCSS('text-align', 'center');
  const typography = await paragraphBody.evaluate(element => {
    const text = element.querySelector('span') || element;
    const style = getComputedStyle(text);
    const paragraphStyle = getComputedStyle(element);
    return { font: style.fontFamily, weight: style.fontWeight, size: style.fontSize,
      indent: parseFloat(paragraphStyle.textIndent), line: parseFloat(paragraphStyle.lineHeight) };
  });
  expect(typography.font).toContain('SimSun');
  expect(typography.weight).toBe('400');
  expect(parseFloat(typography.size)).toBeCloseTo(16, 0);
  expect(typography.indent).toBeCloseTo(32, 0);
  expect(typography.line).toBeCloseTo(24, 0);
  const titleStyle = await title.locator('span').first().evaluate(element => ({
    size: parseFloat(getComputedStyle(element).fontSize), weight: getComputedStyle(element).fontWeight,
  }));
  expect(titleStyle.size).toBeCloseTo(16 * 4 / 3, 0);
  expect(titleStyle.weight).toBe('700');
  for (const theme of ['dark', 'light']) {
    if (await page.locator('html').getAttribute('data-theme') !== theme) {
      await page.getByRole('button', { name: 'Toggle dark mode', exact: true }).click();
    }
    await expect(page.locator('.writer-page')).toHaveCSS('background-color', 'rgb(255, 255, 255)');
    await expect(editor).toHaveCSS('color', 'rgb(0, 0, 0)');
  }
  await expect(editor.locator('li')).toContainText(['自愿参加的第一条说明']);
  await expect(editor.locator('table')).toContainText('表格应位于正文中间');
  await expect(editor.getByRole('img', { name: 'Embedded fixture', exact: true })).toBeVisible();
  const positions = await editor.evaluate(element => ['项目背景与价值', '自愿参加的第一条说明', '表格应位于正文中间', '文档末尾']
    .map(text => element.textContent!.indexOf(text)));
  expect(positions.every(position => position >= 0)).toBe(true);
  expect([...positions].sort((a, b) => a - b)).toEqual(positions);
  await page.screenshot({ path: testInfo.outputPath('writer-fidelity.png') });

  await paragraphBody.click();
  await page.keyboard.press('End');
  await page.keyboard.type(' Added text');
  const readSaved = async () => (await (await request.get(`/api/v1/documents/${id}`, {
    headers: authHeaders(user.token),
  })).json()).data.content;
  await expect.poll(async () => (await readSaved()).body[1].runs.map((item: any) => item.text).join(''), { timeout: 15_000 })
    .toContain('Added text');
  const saved = await readSaved();
  expect(saved.body[1].id).toBe(bodyId);
  expect(saved.body.slice(2)).toEqual(content.body.slice(2));
  expect(saved.professional.paragraph_formats).toEqual(content.professional.paragraph_formats);
  expect(saved.body[1].runs.every((item: any) => item.font_family === 'SimSun' && item.font_size === 12 && !item.bold)).toBe(true);
  await page.reload();
  await expect(editor).toContainText('Added text');
  const reloadedParagraph = editor.locator('p').filter({ hasText: '项目背景与价值' });
  await expect(reloadedParagraph).toHaveCSS('text-indent', '32px');
  await reloadedParagraph.locator('span').first().evaluate(element => {
    const range = document.createRange();
    range.setStart(element.firstChild!, 0);
    range.setEnd(element.firstChild!, 4);
    const selection = document.getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);
  });
  await page.getByRole('button', { name: 'Bold', exact: true }).click();
  await expect.poll(async () => (await readSaved()).body[1].runs.some((item: any) => item.bold), { timeout: 15_000 }).toBe(true);
  const formatted = await readSaved();
  expect(formatted.body[1].runs.map((item: any) => item.text).join('')).toContain('Added text');
  expect(formatted.body.slice(2)).toEqual(content.body.slice(2));

  await reloadedParagraph.evaluate(element => {
    (element.closest('[contenteditable="true"]') as HTMLElement).focus();
    const range = document.createRange();
    range.selectNodeContents(element);
    range.collapse(false);
    const selection = document.getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);
  });
  await page.keyboard.press('Enter');
  await page.keyboard.type('新段落');
  await expect.poll(async () => (await readSaved()).body.some((block: any) =>
    block.runs?.some((item: any) => item.text.includes('新段落'))), { timeout: 15_000 }).toBe(true);
  const withParagraph = await readSaved();
  const newParagraph = withParagraph.body.find((block: any) => block.runs?.some((item: any) => item.text.includes('新段落')));
  expect(newParagraph.id).not.toBe(bodyId);
  expect(withParagraph.professional.paragraph_formats.find((item: any) => item.block_id === newParagraph.id).first_line_indent_chars).toBe(2);
  await page.reload();
  await expect(editor.locator('p').filter({ hasText: '新段落' })).toHaveCSS('text-indent', '32px');

  // Simulate clipboard input without modifying the user's OS clipboard.
  await editor.click();
  await page.keyboard.press('ControlOrMeta+a');
  await editor.evaluate(element => {
    const data = new DataTransfer();
    data.setData('text/plain', 'Replacement preserves embedded objects');
    element.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }));
  });
  await expect.poll(async () => (await readSaved()).body.filter((block: any) => block.runs)
    .map((block: any) => block.runs.map((item: any) => item.text).join('')).join('\n'), { timeout: 15_000 })
    .toBe('Replacement preserves embedded objects');
  await expect(page.locator('.save-status')).toHaveText('All changes saved');
  const pasted = await readSaved();
  expect(pasted.body.filter((block: any) => ['List', 'Table', 'Image'].includes(block.type)))
    .toEqual(content.body.filter((block: any) => ['List', 'Table', 'Image'].includes(block.type)));
  expect(errors).toEqual([]);
});

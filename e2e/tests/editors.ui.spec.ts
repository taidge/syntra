import { test, expect, Page, APIRequestContext } from '@playwright/test';

const APP_BASE = (process.env.API_URL || 'http://localhost:3000').replace(/\/$/, '');
const API_BASE = `${APP_BASE}/api/v1`;
type EditorKind = 'writer' | 'slides' | 'whiteboard';
interface Fixture { id: string; token: string; user: Record<string, unknown> }

async function createDocument(request: APIRequestContext, kind: EditorKind): Promise<Fixture> {
  const suffix = `${Date.now()}_${Math.random().toString(36).slice(2, 9)}`;
  const registered = await request.post(`${API_BASE}/auth/register`, {
    data: { username: `editor_${suffix}`, email: `editor_${suffix}@test.local`, password: 'Test1234!' },
  });
  expect(registered.ok()).toBeTruthy();
  const { token, user } = (await registered.json()).data;
  const created = await request.post(`${API_BASE}/documents`, {
    headers: { Authorization: `Bearer ${token}` }, data: { name: `Glory ${kind}`, document_type: kind },
  });
  expect(created.ok()).toBeTruthy();
  return { id: (await created.json()).data.id, token, user };
}
async function documentContent(request: APIRequestContext, fixture: Fixture) {
  const response = await request.get(`${API_BASE}/documents/${fixture.id}`, {
    headers: { Authorization: `Bearer ${fixture.token}` },
  });
  expect(response.ok()).toBeTruthy();
  return (await response.json()).data.content;
}
async function openEditor(page: Page, fixture: Fixture, kind: EditorKind) {
  await page.addInitScript(({ token, user }) => {
    localStorage.setItem('contrix.auth', JSON.stringify({ token, user }));
    localStorage.setItem('locale', 'en');
  }, fixture);
  await page.goto(`${APP_BASE}/${kind}/${fixture.id}`);
  await expect(page.getByRole('button', { name: 'Save', exact: true })).toBeEnabled({ timeout: 30_000 });
}
function writerText(content: any): string {
  return content.body.filter((block: any) => Array.isArray(block.runs))
    .map((block: any) => block.runs.map((run: any) => run.text).join('')).join('\n');
}
function slideText(content: any, index: number, element = 0): string | undefined {
  return content.slides[index]?.elements[element]?.content.map((run: any) => run.text).join('');
}
function boardElements(content: any): any[] {
  return content.layers.flatMap((layer: any) => layer.elements);
}

test.describe('Glory document editors @ui', () => {
  test('Writer edits survive autosave and reload while native styles and tables stay intact', async ({ page, request }) => {
    const fixture = await createDocument(request, 'writer');
    const content = await documentContent(request, fixture);
    content.body = [
      { type: 'Paragraph', id: '48c1539b-d723-4ca6-a601-c3d3e3787341', alignment: 'Left', indent_level: 0,
        spacing: { before: 0, after: 8, line_height: 1.15 },
        runs: [{ text: 'Legacy text', bold: false, italic: false, underline: true, strikethrough: false,
          font_family: 'Georgia', font_size: 22, color: '#123456', background_color: null,
          link: null, superscript: false, subscript: false }] },
      { type: 'Table', id: '7ab041ae-61e7-4146-9b52-9efef8e14f31', rows: [], column_widths: [120] },
    ];
    const seeded = await request.put(`${API_BASE}/documents/${fixture.id}`, {
      headers: { Authorization: `Bearer ${fixture.token}` }, data: { content },
    });
    expect(seeded.ok()).toBeTruthy();
    await openEditor(page, fixture, 'writer');
    const editor = page.getByRole('textbox', { name: 'Document body', exact: true });
    await expect(editor).toContainText('Legacy text');
    await editor.fill('Glory Writer saved 世界');
    await expect.poll(async () => writerText(await documentContent(request, fixture)), { timeout: 15_000 })
      .toBe('Glory Writer saved 世界');
    const saved = await documentContent(request, fixture);
    expect(saved.body[1]).toEqual(content.body[1]);
    expect(saved.body[0].id).toBe(content.body[0].id);
    for (const run of saved.body[0].runs) {
      expect(run.underline).toBe(true);
      expect(run.font_family).toBe('Georgia');
      expect(run.color).toBe('#123456');
    }
    await page.reload();
    await expect(page.getByRole('button', { name: 'Save', exact: true })).toBeEnabled();
    await expect(page.getByRole('textbox', { name: 'Document body', exact: true })).toContainText('Glory Writer saved 世界');
  });

  test('Slides saves title, body, notes and newly created slides', async ({ page, request }) => {
    const fixture = await createDocument(request, 'slides');
    await openEditor(page, fixture, 'slides');
    await page.getByRole('textbox', { name: 'Slide title', exact: true }).fill('Quarterly review');
    await page.getByRole('textbox', { name: 'Slide body', exact: true }).fill('Revenue grew 世界');
    await page.getByRole('textbox', { name: 'Speaker notes', exact: true }).fill('Discuss the margin');
    await expect.poll(async () => {
      const saved = await documentContent(request, fixture);
      return [slideText(saved, 0), slideText(saved, 0, 1), saved.slides[0]?.notes];
    }, { timeout: 15_000 }).toEqual(['Quarterly review', 'Revenue grew 世界', 'Discuss the margin']);
    await page.getByRole('button', { name: 'New slide', exact: true }).click();
    await page.getByRole('textbox', { name: 'Slide title', exact: true }).fill('Next quarter');
    await expect.poll(async () => {
      const saved = await documentContent(request, fixture);
      return [saved.slides.length, slideText(saved, 0), slideText(saved, 1)];
    }, { timeout: 15_000 }).toEqual([2, 'Quarterly review', 'Next quarter']);
    await page.reload();
    await expect(page.getByRole('button', { name: 'Save', exact: true })).toBeEnabled();
    await expect(page.getByRole('textbox', { name: 'Slide title', exact: true })).toHaveValue('Quarterly review');
    await page.getByRole('button', { name: 'Next', exact: true }).click();
    await expect(page.getByRole('textbox', { name: 'Slide title', exact: true })).toHaveValue('Next quarter');
  });

  test('Whiteboard drawing saves, undoes, redoes and reloads through the semantic drag host', async ({ page, request }) => {
    const fixture = await createDocument(request, 'whiteboard');
    await openEditor(page, fixture, 'whiteboard');
    await page.getByRole('combobox', { name: 'Drawing tool', exact: true }).selectOption('text');
    await page.getByRole('textbox', { name: 'Text to draw', exact: true }).fill('Saved board note');
    const canvas = page.getByRole('group', { name: 'Whiteboard drawing canvas', exact: true });
    await expect(canvas).toBeVisible();
    await canvas.click({ position: { x: 120, y: 90 } });
    await expect.poll(async () => boardElements(await documentContent(request, fixture)).map(element => element.text), { timeout: 15_000 })
      .toEqual(['Saved board note']);
    await page.getByRole('button', { name: 'Undo', exact: true }).click();
    await expect.poll(async () => boardElements(await documentContent(request, fixture)).length, { timeout: 15_000 }).toBe(0);
    await page.getByRole('button', { name: 'Redo', exact: true }).click();
    await expect.poll(async () => boardElements(await documentContent(request, fixture)).map(element => element.text), { timeout: 15_000 })
      .toEqual(['Saved board note']);
    await page.reload();
    await expect(page.getByRole('button', { name: 'Save', exact: true })).toBeEnabled();
    await expect(page.getByRole('img', { name: 'Drawing preview', exact: true })).toBeVisible();
    expect(boardElements(await documentContent(request, fixture))[0].text).toBe('Saved board note');
  });
});

for (const kind of ['slides', 'whiteboard'] as const) {
  test(`${kind} loads existing image objects through real positioned browser images`, async ({ page, request }) => {
    const fixture = await createDocument(request, kind);
    const content = await documentContent(request, fixture);
    const imageUrl = `${APP_BASE}/editor-image-fixture.svg`;
    await page.route(imageUrl, route => route.fulfill({
      contentType: 'image/svg+xml', body: '<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="#2563eb"/></svg>',
    }));
    const image = { type: 'Image', id: '7206d684-f0b6-4b8f-a427-d19fca91d68d', x: 120, y: 80,
      width: 400, height: 200, rotation: 0, url: imageUrl, alt_text: 'Remote fixture image' };
    if (kind === 'slides') {
      content.slides = [{ id: '2a456a6d-4ad1-4087-8e1b-2d556757f11f', elements: [image],
        background: { Solid: '#ffffff' }, transition: null, animations: [], notes: '', layout: 'Blank' }];
    } else {
      content.layers[0].elements = [image];
    }
    const seeded = await request.put(`${API_BASE}/documents/${fixture.id}`, {
      headers: { Authorization: `Bearer ${fixture.token}` }, data: { content },
    });
    expect(seeded.ok()).toBeTruthy();
    await openEditor(page, fixture, kind);
    const rendered = page.getByRole('img', { name: 'Remote fixture image', exact: true });
    await expect(rendered).toBeVisible();
    await expect.poll(() => rendered.evaluate((element: HTMLImageElement) => element.complete && element.naturalWidth === 40)).toBe(true);
    await expect(rendered).toHaveCSS('position', 'absolute');
    if (kind === 'whiteboard') {
      await page.getByRole('button', { name: 'Properties', exact: true }).click();
      await page.getByRole('button', { name: '1. Image', exact: true }).click();
      await page.getByRole('textbox', { name: 'Move X', exact: true }).fill('50');
      await page.getByRole('textbox', { name: 'Move Y', exact: true }).fill('0');
      await page.getByRole('button', { name: 'Move selected', exact: true }).click();
      await expect.poll(async () => boardElements(await documentContent(request, fixture))[0].x, { timeout: 15_000 }).toBe(170);
      await page.getByRole('checkbox', { name: 'Visible', exact: true }).uncheck();
      await expect(rendered).toHaveCount(0);
      await expect.poll(async () => (await documentContent(request, fixture)).layers[0].visible, { timeout: 15_000 }).toBe(false);
      await page.getByRole('checkbox', { name: 'Visible', exact: true }).check();
      await expect(rendered).toBeVisible();
      await expect.poll(async () => (await documentContent(request, fixture)).layers[0].visible, { timeout: 15_000 }).toBe(true);
    }
    await page.reload();
    await expect(page.getByRole('button', { name: 'Save', exact: true })).toBeEnabled();
    await expect(page.getByRole('img', { name: 'Remote fixture image', exact: true })).toBeVisible();
  });
}

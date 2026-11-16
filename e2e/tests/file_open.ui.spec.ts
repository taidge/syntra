import { test, expect, Page } from '@playwright/test';
import {
  authHeaders,
  createDocument,
  createFolder,
  registerUser,
  uniqueEmail,
  uniqueUsername,
} from './helpers';

type EditorKind = 'writer' | 'calc' | 'slides' | 'whiteboard';

function fileCard(page: Page, name: string) {
  return page.locator('.file-card').filter({
    has: page.locator('.file-name').filter({ hasText: name }),
  });
}

async function expectEditor(page: Page, kind: EditorKind, id: string, name: string) {
  await expect(page).toHaveURL(new RegExp(`/${kind}/${id}$`));
  await expect(page.locator('.save-status')).toHaveText(
    kind === 'calc' ? 'Saved' : 'All changes saved',
    { timeout: 30_000 },
  );
  await expect(page.locator(kind === 'calc' ? '.calc-document-name' : '.editor-title')).toHaveText(name);
  switch (kind) {
    case 'writer':
      await expect(page.getByRole('textbox', { name: 'Document body', exact: true }))
        .toContainText('Opened from the file card');
      break;
    case 'calc':
      await expect(page.locator('.sheet-grid')).toBeVisible();
      await expect(page.getByRole('button', { name: 'A1', exact: true })).toBeVisible();
      break;
    case 'slides':
      await expect(page.getByRole('button', { name: 'New slide', exact: true })).toBeVisible();
      await expect(page.locator('.slides-workspace')).toBeVisible();
      break;
    case 'whiteboard':
      await expect(page.getByRole('group', { name: 'Whiteboard drawing canvas', exact: true })).toBeVisible();
      await expect(page.getByRole('combobox', { name: 'Drawing tool', exact: true })).toBeVisible();
      break;
  }
}

test.describe('Open files from their cards @ui', () => {
  test('opens a folder from the card padding and shows its documents', async ({ page, request }) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'FileOpen@12345');
    const folderName = 'Open folder from card padding';
    const folder = await createFolder(request, user.token, folderName);
    expect(folder.response.status()).toBe(201);
    const folderId = folder.body.data.id as string;
    const childName = 'Writer inside the folder';
    const child = await createDocument(request, user.token, childName, 'writer', folderId);
    expect(child.response.status()).toBe(201);
    const childId = child.body.data.id as string;

    await page.addInitScript(auth => {
      localStorage.setItem('locale', 'en');
      localStorage.setItem('contrix.auth', JSON.stringify({
        token: auth.token,
        user: {
          id: auth.userId,
          username: auth.username,
          email: auth.email,
          display_name: null,
          role: 'editor',
        },
      }));
    }, user);

    await page.goto('/files');
    const folderCard = fileCard(page, folderName);
    await expect(folderCard).toBeVisible();
    await expect(fileCard(page, childName)).toHaveCount(0);
    const folderListing = page.waitForResponse(response => {
      const url = new URL(response.url());
      return response.request().method() === 'GET' &&
        url.pathname === '/api/v1/files' && url.searchParams.get('parent_id') === folderId;
    });
    await folderCard.locator('.file-open').click({ position: { x: 8, y: 8 } });
    const listed = await folderListing;
    expect(listed.ok()).toBe(true);
    const listing = await listed.json();
    expect(listing.data.some((file: { document_id: string }) => file.document_id === childId)).toBe(true);

    await expect(page).toHaveURL(/\/files$/);
    await expect(page.locator('.breadcrumb')).toContainText('My Files');
    await expect(page.locator('.breadcrumb')).toContainText(folderName);
    await expect(folderCard).toHaveCount(0);
    const childLink = fileCard(page, childName).locator('.file-card-body');
    await expect(childLink).toBeVisible();
    await expect(childLink).toHaveAttribute('href', `/documents/${childId}`);
    expect(errors).toEqual([]);
  });

  for (const kind of ['writer', 'calc', 'slides', 'whiteboard'] as const) {
    test(`opens ${kind} from the file icon`, async ({ page, request }, testInfo) => {
      test.setTimeout(60_000);
      const errors: string[] = [];
      page.on('pageerror', error => errors.push(error.message));
      const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'FileOpen@12345');
      const name = `Open ${kind} from card`;
      const { response, body } = await createDocument(request, user.token, name, kind);
      expect(response.status()).toBe(201);
      const id = body.data.id as string;

      if (kind === 'writer') {
        const content = body.data.content;
        content.body = [{
          type: 'Paragraph',
          id: 'a90545b8-2dce-47ab-8ca2-a8c15d5dbdb4',
          alignment: 'Left',
          indent_level: 0,
          spacing: { before: 0, after: 8, line_height: 1.15 },
          runs: [{
            text: 'Opened from the file card',
            bold: false, italic: false, underline: false, strikethrough: false,
            font_family: 'Arial', font_size: 12, color: '#000000',
            background_color: null, link: null, superscript: false, subscript: false,
          }],
        }];
        const seeded = await request.put(`/api/v1/documents/${id}`, {
          headers: authHeaders(user.token),
          data: { content, expected_version: body.data.version },
        });
        expect(seeded.ok()).toBe(true);
      }

      await page.addInitScript(auth => {
        localStorage.setItem('locale', 'en');
        localStorage.setItem('contrix.auth', JSON.stringify({
          token: auth.token,
          user: {
            id: auth.userId,
            username: auth.username,
            email: auth.email,
            display_name: null,
            role: 'editor',
          },
        }));
      }, user);

      await page.goto('/files');
      const card = fileCard(page, name);
      await expect(card.locator('.file-card-body')).toHaveAttribute('href', `/documents/${id}`);
      await card.locator('.file-icon').click();
      await expectEditor(page, kind, id, name);

      if (kind === 'writer') {
        await page.screenshot({ path: testInfo.outputPath('opened-writer.png') });

        // The unused space around the icon/name must activate the same document.
        await page.goto('/files');
        await card.locator('.file-card-body').click({ position: { x: 8, y: 8 } });
        await expectEditor(page, kind, id, name);

        await page.goto('/files');
        await card.locator('.file-card-body').dblclick({ position: { x: 8, y: 8 } });
        await expectEditor(page, kind, id, name);

        await page.goto('/files');
        await page.getByRole('button', { name: 'List', exact: true }).click();
        await expect(card).toHaveClass(/file-row/);
        await card.locator('.file-icon').click();
        await expectEditor(page, kind, id, name);
      }
      expect(errors).toEqual([]);
    });
  }
});

import { test, expect } from '@playwright/test';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname } from 'node:path';
import {
  authHeaders,
  createDocument,
  registerUser,
  uniqueEmail,
  uniqueUsername,
} from './helpers';

// Relative URLs inherit API_URL through playwright.config.ts. The application
// and JSON API must be served from the same origin; no transport is mocked.
test.use({ locale: 'en-US' });

test.describe('Glory application flows @ui', () => {
  test.beforeEach(async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('locale', 'en'));
  });

  test('registers with Enter, creates a real document, and loads files and profile', async ({ page, request }) => {
    test.setTimeout(60_000);
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    const username = uniqueUsername();
    const email = uniqueEmail();
    const password = 'GloryTest@12345';

    await page.goto('/register');
    await expect(page.getByRole('heading', { name: 'Create Account', exact: true })).toBeVisible();
    await expect(page.getByText('Email', { exact: true })).toBeVisible();
    await page.getByLabel('Username', { exact: true }).fill(username);
    await page.getByLabel('Email', { exact: true }).fill(email);
    await page.getByLabel('Password', { exact: true }).fill(password);
    await page.getByLabel('Confirm Password', { exact: true }).fill(password);

    const registration = page.waitForResponse(response =>
      response.request().method() === 'POST' &&
      new URL(response.url()).pathname === '/api/v1/auth/register',
    );
    await page.getByLabel('Confirm Password', { exact: true }).press('Enter');
    const registrationResponse = await registration;
    expect(registrationResponse.status()).toBe(201);
    await expect(page).toHaveURL(/\/dashboard$/);
    await expect(page.getByRole('heading', { name: 'Dashboard', exact: true })).toBeVisible();

    const stored = await page.evaluate(() => JSON.parse(localStorage.getItem('contrix.auth') || '{}'));
    expect(stored.token).toBeTruthy();
    expect(stored.user.username).toBe(username);
    const accountResponse = await request.get('/api/v1/auth/me', {
      headers: authHeaders(stored.token),
    });
    expect(accountResponse.ok()).toBe(true);
    const account = await accountResponse.json();
    expect(account.success).toBe(true);
    expect(account.data.id).toBe(stored.user.id);
    expect(account.data.email).toBe(email);
    expect(account.data.username).toBe(username);

    const creation = page.waitForResponse(response =>
      response.request().method() === 'POST' &&
      new URL(response.url()).pathname === '/api/v1/documents',
    );
    await page.getByRole('button', { name: 'Spreadsheet', exact: true }).click();
    const creationResponse = await creation;
    expect(creationResponse.status()).toBe(201);
    await expect(page).toHaveURL(/\/calc\/[0-9a-f]{8}-[0-9a-f-]{27}$/i);
    const documentId = new URL(page.url()).pathname.split('/').pop();
    expect(documentId).toBeTruthy();
    await expect(page.locator('.sheet-grid')).toBeVisible();

    const saved = await request.get('/api/v1/documents/' + documentId, {
      headers: authHeaders(stored.token),
    });
    expect(saved.ok()).toBe(true);
    const created = await saved.json();
    expect(created.success).toBe(true);
    expect(created.data.id).toBe(documentId);
    expect(created.data.document_type).toBe('calc');

    const filesResponse = page.waitForResponse(response =>
      response.request().method() === 'GET' &&
      new URL(response.url()).pathname === '/api/v1/files',
    );
    await page.goto('/files');
    expect((await filesResponse).ok()).toBe(true);
    await expect(page.getByRole('button', { name: created.data.name, exact: true })).toBeVisible();

    const profileResponse = page.waitForResponse(response =>
      response.request().method() === 'GET' &&
      new URL(response.url()).pathname === '/api/v1/auth/me',
    );
    await page.goto('/profile');
    expect((await profileResponse).ok()).toBe(true);
    await expect(page.getByRole('heading', { name: 'Profile', exact: true })).toBeVisible();
    await expect(page.locator('.profile-container')).toContainText(email);
    await expect(page.locator('.profile-container')).toContainText(username);
    await expect(page.getByLabel('Display name', { exact: true })).toBeVisible();

    await page.getByRole('button', { name: 'Logout', exact: true }).click();
    await expect(page).toHaveURL(/\/login$/);
    await page.getByLabel('Email', { exact: true }).fill(email);
    await page.getByLabel('Password', { exact: true }).fill(password);
    const login = page.waitForResponse(response =>
      response.request().method() === 'POST' &&
      new URL(response.url()).pathname === '/api/v1/auth/login',
    );
    await page.getByLabel('Password', { exact: true }).press('Enter');
    expect((await login).status()).toBe(200);
    await expect(page).toHaveURL(/\/dashboard$/);
    expect(errors).toEqual([]);
  });

  test('uploads through the file control, preserves MIME, and renames and deletes the file', async ({ page, request }, testInfo) => {
    test.setTimeout(60_000);
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'GloryTest@12345');
    await page.addInitScript(auth => {
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
    const filename = `glory-upload-${Date.now()}.txt`;
    const renamed = filename.replace('upload', 'renamed');
    // More than 64 KiB exercises the recursive read path as well as MIME forwarding.
    const bytes = Buffer.from('Glory FileController upload regression\n'.repeat(2_000), 'utf8');
    const uploadPath = testInfo.outputPath(filename);
    await mkdir(dirname(uploadPath), { recursive: true });
    await writeFile(uploadPath, bytes);

    const upload = page.waitForResponse(response =>
      response.request().method() === 'POST' &&
      new URL(response.url()).pathname === '/api/v1/files/upload',
    );
    // The host owns the active picker ticket and accepts trusted chooser events.
    // A real path uses the browser file protocol rather than synthetic FileList events.
    const chooser = page.waitForEvent('filechooser');
    await page.getByRole('button', { name: 'Upload', exact: true }).click();
    const picker = await chooser;
    expect(picker.isMultiple()).toBe(true);
    await picker.setFiles(uploadPath);
    expect((await upload).status()).toBe(201);
    await expect(page.getByRole('button', { name: filename, exact: true })).toBeVisible();

    const headers = authHeaders(user.token);
    const filesResponse = await request.get('/api/v1/files', { headers });
    expect(filesResponse.ok()).toBe(true);
    const files = await filesResponse.json();
    expect(files.success).toBe(true);
    const uploaded = files.data.find((file: { name: string }) => file.name === filename);
    expect(uploaded).toBeDefined();
    expect(uploaded.mime_type).toBe('text/plain');
    expect(uploaded.size).toBe(bytes.length);

    const originalCard = page.locator('.file-card').filter({
      has: page.getByRole('button', { name: filename, exact: true }),
    });
    page.once('dialog', async dialog => {
      expect(dialog.type()).toBe('prompt');
      await dialog.accept(renamed);
    });
    const rename = page.waitForResponse(response =>
      response.request().method() === 'PUT' &&
      new URL(response.url()).pathname === `/api/v1/files/${uploaded.id}`,
    );
    await originalCard.getByRole('button', { name: 'Rename', exact: true }).click();
    expect((await rename).ok()).toBe(true);
    await expect(page.getByRole('button', { name: filename, exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: renamed, exact: true })).toBeVisible();

    const renamedResponse = await request.get('/api/v1/files', { headers });
    expect(renamedResponse.ok()).toBe(true);
    const renamedFiles = await renamedResponse.json();
    const renamedFile = renamedFiles.data.find((file: { id: string }) => file.id === uploaded.id);
    expect(renamedFile.name).toBe(renamed);
    expect(renamedFile.mime_type).toBe('text/plain');

    const renamedCard = page.locator('.file-card').filter({
      has: page.getByRole('button', { name: renamed, exact: true }),
    });
    page.once('dialog', async dialog => {
      expect(dialog.type()).toBe('confirm');
      await dialog.accept();
    });
    const deletion = page.waitForResponse(response =>
      response.request().method() === 'DELETE' &&
      new URL(response.url()).pathname === `/api/v1/files/${uploaded.id}`,
    );
    await renamedCard.getByRole('button', { name: 'Delete', exact: true }).click();
    expect((await deletion).ok()).toBe(true);
    await expect(page.getByRole('button', { name: renamed, exact: true })).toHaveCount(0);
    const afterResponse = await request.get('/api/v1/files', { headers });
    expect(afterResponse.ok()).toBe(true);
    const after = await afterResponse.json();
    expect(after.data.some((file: { id: string }) => file.id === uploaded.id)).toBe(false);
    expect(errors).toEqual([]);
  });

  test('redirects a direct document URL to login without stored authentication', async ({ page, request }) => {
    const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'GloryTest@12345');
    const { response, body } = await createDocument(request, user.token, 'Protected Glory document', 'writer');
    expect(response.status()).toBe(201);
    await page.addInitScript(() => localStorage.removeItem('contrix.auth'));
    await page.goto('/writer/' + body.data.id);
    await expect(page).toHaveURL(/\/login$/);
    await expect(page.getByRole('heading', { name: 'Sign In', exact: true })).toBeVisible();
    await expect(page.getByLabel('Email', { exact: true })).toBeVisible();
  });

  test('shows the application 404 for unknown paths and malformed document IDs', async ({ page }) => {
    for (const path of ['/missing-glory-route', '/calc/not-a-document-id']) {
      await page.goto(path);
      await expect(page.locator('.not-found')).toHaveText(/Page not found/);
      await expect(page.locator('.not-found').getByRole('link', { name: 'Contrix Office' })).toHaveAttribute('href', '/');
      await expect(page).toHaveURL(new RegExp(path + '$'));
    }
  });
});

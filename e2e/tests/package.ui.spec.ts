import { test, expect } from '@playwright/test';
import { registerUser, uniqueEmail, uniqueUsername } from './helpers';

test('package smoke: real login saves and reloads Writer and Calc Worker results @ui', async ({page,request})=>{
  test.setTimeout(60_000);
  const email=uniqueEmail();const password='PackageSmoke@12345';
  await registerUser(request,uniqueUsername(),email,password);
  const errors:string[]=[];page.on('pageerror',error=>errors.push(error.message));
  await page.addInitScript(()=>localStorage.setItem('locale','en'));
  await page.goto('/login');
  await expect(page).toHaveTitle('Syntra Office');
  const loginLogo = page.locator('.auth-header img.syntra-mark');
  await expect(loginLogo).toBeVisible();
  await expect.poll(() => loginLogo.evaluate((image: HTMLImageElement) => image.naturalWidth)).toBeGreaterThan(0);
  await page.emulateMedia({ colorScheme: 'dark' });
  await expect(loginLogo).toHaveCSS('content', /syntra-dark\.svg/);
  await page.emulateMedia({ colorScheme: 'light' });
  await page.getByLabel('Email',{exact:true}).fill(email);
  await page.getByLabel('Password',{exact:true}).fill(password);
  const loggedIn=page.waitForResponse(response=>new URL(response.url()).pathname==='/api/v1/auth/login'&&response.request().method()==='POST');
  await page.getByRole('button',{name:'Sign In',exact:true}).click();
  const loginResponse = await loggedIn;
  expect(loginResponse.status()).toBe(200);
  await expect(page).toHaveURL(/\/dashboard$/);
  const token = await page.evaluate(() => JSON.parse(localStorage.getItem('syntra.auth') || 'null')?.token);
  expect(typeof token).toBe('string');
  await page.evaluate(() => {
    const session = localStorage.getItem('syntra.auth');
    if (!session) throw new Error('Missing session after login');
    localStorage.setItem('\x63ontrix.auth', session);
    localStorage.removeItem('syntra.auth');
  });
  await page.reload();
  await expect(page.getByRole('heading', { name: 'Dashboard', exact: true })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Syntra Office', exact: true })).toBeVisible();
  expect(await page.evaluate(() => localStorage.getItem('\x63ontrix.auth'))).toBeNull();
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem('syntra.auth') || 'null')?.token)).toBe(token);
  const headers = { Authorization: `Bearer ${token}` };
  await page.getByRole('button',{name:'Writer document',exact:true}).click();
  await expect(page).toHaveURL(/\/writer\/[0-9a-f-]+$/);
  await expect(page.locator('.save-status')).toHaveText('All changes saved');
  const content='Packaged Syntra Office saves 世界 without a source checkout';
  const saved=page.waitForResponse(response=>/\/api\/v1\/documents\/[0-9a-f-]+$/.test(new URL(response.url()).pathname)&&response.request().method()==='PUT');
  await page.getByRole('textbox',{name:'Document body',exact:true}).fill(content);
  const persisted=await saved;expect(persisted.status()).toBe(200);
  expect(persisted.request().postDataJSON().expected_version).toBeGreaterThan(0);
  await expect(page.locator('.save-status')).toHaveText('All changes saved');
  await page.reload();
  await expect(page.locator('.save-status')).toHaveText('All changes saved');
  await expect(page.getByRole('textbox',{name:'Document body',exact:true})).toContainText(content);
  await page.screenshot({path:test.info().outputPath('package-writer-saved.png')});

  await page.getByRole('link', { name: 'Dashboard', exact: true }).click();
  await expect(page).toHaveURL(/\/dashboard$/);
  await page.getByRole('button', { name: 'Spreadsheet', exact: true }).click();
  await expect(page).toHaveURL(/\/calc\/[0-9a-f-]+$/);
  await expect(page.locator('.save-status')).toHaveText('Saved');
  const calcId = new URL(page.url()).pathname.split('/').pop();
  const cell = (name: string) => page.getByRole('button', { name, exact: true });
  const input = page.getByRole('textbox', { name: 'Formula', exact: true });
  const apply = page.getByRole('button', { name: 'Apply formula', exact: true });
  await cell('B1').click();
  await input.fill('2');
  await apply.click();
  await expect(cell('B1')).toHaveText('2');
  await cell('A1').click();
  await expect(input).toHaveValue('');
  const formula = '=SUM(B1:B20000)';
  await input.fill(formula);
  const worker = page.waitForEvent('worker', worker => worker.url().endsWith('/assets/calc-worker.js'));
  await apply.click();
  await worker;
  await expect(cell('A1')).toHaveText('2');

  const calcSaved = page.waitForResponse(response => {
    if (new URL(response.url()).pathname !== `/api/v1/documents/${calcId}` || response.request().method() !== 'PUT') {
      return false;
    }
    const cells = response.request().postDataJSON().content.sheets[0].cells;
    return cells.some(([point, value]: [{ row: number; col: number }, { formula?: string; value?: { value?: number } }]) =>
      point.row === 0 && point.col === 0 && value.formula === formula && value.value?.value === 7);
  });
  await cell('B1').click();
  await expect(input).toHaveValue('2');
  await input.fill('7');
  await apply.click();
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  const calcReceipt = await calcSaved;
  expect(calcReceipt.status()).toBe(200);
  expect(calcReceipt.request().postDataJSON().expected_version).toBeGreaterThan(0);
  await expect(page.locator('.save-status')).toHaveText('Saved');
  const calcStored = await request.get(`/api/v1/documents/${calcId}`, { headers });
  expect(calcStored.ok()).toBeTruthy();
  expect((await calcStored.json()).data.content.sheets[0].cells).toEqual(expect.arrayContaining([
    [expect.objectContaining({ row: 0, col: 0 }), expect.objectContaining({ formula, value: { type: 'Number', value: 7 } })],
  ]));
  await page.reload();
  await expect(cell('A1')).toHaveText('7');
  await expect(cell('B1')).toHaveText('7');
  await cell('A1').click();
  await expect(input).toHaveValue(formula);
  await page.screenshot({ path: test.info().outputPath('package-calc-worker-saved.png') });
  expect(errors).toEqual([]);
});

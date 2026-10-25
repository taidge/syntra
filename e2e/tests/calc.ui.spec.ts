// Calc semantic-control UI tests.
// Modifier-click, double-click and drag-fill are represented by explicit
// selection controls and Fill actions in the semantic Glory host.
//
// These tests assume a contrix-server is already running at http://localhost:3000
// (it serves both the JSON API and the Glory + Sapid single-page app). Start it with
// `cargo run -p contrix-server` from the workspace root before invoking
// `npx playwright test calc.ui`. The playwright.config.ts `webServer` block
// will reuse the existing server when one is detected.

import { test, expect, Page, APIRequestContext } from '@playwright/test';

const APP_BASE = (process.env.API_URL || 'http://localhost:3000').replace(/\/$/, '');
const API_BASE = `${APP_BASE}/api/v1`;

// Must match `AUTH_STORAGE_KEY` in crates/frontend/src/state.rs.
const AUTH_STORAGE_KEY = 'contrix.auth';

interface CalcFixture {
    id: string;
    token: string;
    user: Record<string, unknown>;
}

async function createCalcDoc(request: APIRequestContext): Promise<CalcFixture> {
    // 1. Register a unique user so each test is independent.
    const suffix = `${Date.now()}_${Math.random().toString(36).slice(2, 8)}`;
    const reg = await request.post(`${API_BASE}/auth/register`, {
        data: {
            username: `user_${suffix}`,
            email: `user_${suffix}@test.local`,
            password: 'Test1234!',
        },
    });
    expect(reg.ok()).toBeTruthy();
    const regBody = await reg.json();
    const token: string = regBody.data.token;

    // 2. Create a calc document for that user.
    const doc = await request.post(`${API_BASE}/documents`, {
        headers: { Authorization: `Bearer ${token}` },
        data: { name: 'E2E Calc', document_type: 'calc' },
    });
    expect(doc.ok()).toBeTruthy();
    const docBody = await doc.json();
    return { id: docBody.data.id, token, user: regBody.data.user };
}

async function gotoCalc(page: Page, id: string, token: string, user: Record<string, unknown>) {
    // Pre-seed auth state so the Glory + Sapid app finds the token in localStorage
    // before its first render and skips the redirect to /login.
    await page.addInitScript(
        ({ key, tok, user }) => {
            localStorage.setItem(
                key,
                JSON.stringify({
                    token: tok,
                    user,
                }),
            );
        },
        { key: AUTH_STORAGE_KEY, tok: token, user },
    );
    await page.goto(`${APP_BASE}/calc/${id}`);
    // Wait for the spreadsheet to actually render before any interaction.
    await page.locator('.sheet-grid').waitFor({ state: 'visible', timeout: 15000 });
}

/** Locate a body cell by zero-based (rowIdx, colIdx). */
function cellAt(page: Page, row: number, col: number) {
    return page.locator('.sheet-grid tbody tr').nth(row).locator('.cell').nth(col).locator('.cell-button');
}

test.describe('Calc page UI @ui', () => {
    let docId: string;
    let token: string;

    test.beforeEach(async ({ request, page }) => {
        const fixture = await createCalcDoc(request);
        docId = fixture.id;
        token = fixture.token;
        await gotoCalc(page, docId, token, fixture.user);
    });

    test('renders an empty spreadsheet grid', async ({ page }) => {
        await expect(page.locator('.sheet-grid')).toBeVisible();

        // Column headers A-Z (26 of them).
        const colHeaders = page.locator('.sheet-grid thead .col-header');
        await expect(colHeaders).toHaveCount(26);
        await expect(colHeaders.first()).toHaveText('A');
        await expect(colHeaders.nth(25)).toHaveText('Z');

        // 50 row headers in the body.
        const rowHeaders = page.locator('.sheet-grid tbody .row-header');
        await expect(rowHeaders).toHaveCount(50);
        await expect(rowHeaders.first()).toHaveText('1');
        await expect(rowHeaders.nth(49)).toHaveText('50');
    });

    test('single-click selects a cell and shows ref in status bar', async ({ page }) => {
        // (row=2, col=3) → D3
        await cellAt(page, 2, 3).click();

        await expect(page.locator('.cell.active')).toHaveCount(1);
        await expect(page.locator('.status-bar')).toContainText('Cell: D3');
    });

    test('selecting a cell allows its formula editor to receive focus', async ({ page }) => {
        await cellAt(page, 1, 1).click();
        await page.getByRole('textbox', { name: 'Formula', exact: true }).focus();

        await expect
            .poll(async () => await page.evaluate(() => document.activeElement?.id ?? ''))
            .toBe('calc-formula-input');
    });

    test('formula editing commits the selected cell and advances on Enter', async ({ page }) => {
        await cellAt(page, 0, 0).click();

        await page.getByRole('textbox', { name: 'Formula', exact: true }).fill('5');

        const formula = page.locator('#calc-formula-input');
        await expect(formula).toHaveValue('5');

        await page.keyboard.press('Enter');

        // Cell A1 shows the committed value...
        await expect(cellAt(page, 0, 0)).toHaveText('5');
        // ...and the active cell has moved down to A2.
        await expect(page.locator('.status-bar')).toContainText('Cell: A2');
    });

    test('Fill down extends the anchor value through a selected range', async ({ page }) => {
        await cellAt(page, 0, 0).click();
        await page.getByRole('textbox', { name: 'Formula', exact: true }).fill('7');
        await page.getByRole('button', { name: 'Apply formula', exact: true }).click();
        await page.getByRole('textbox', { name: 'Selection range', exact: true }).fill('A1:A3');
        await page.getByRole('button', { name: 'Select range', exact: true }).click();
        await page.getByRole('button', { name: 'Fill down', exact: true }).click();

        await expect(cellAt(page, 0, 0)).toHaveText('7');
        await expect(cellAt(page, 1, 0)).toHaveText('7');
        await expect(cellAt(page, 2, 0)).toHaveText('7');
    });

    test('Add cells to selection retains a non-contiguous selection', async ({ page }) => {
        await cellAt(page, 0, 0).click();
        await page.getByRole('checkbox', { name: 'Add cells to selection', exact: true }).check();
        await cellAt(page, 2, 2).click();

        // C3 became the active cell while additive selection was enabled; A1 stays in the
        // extra-selection set. The visual contract is that both cells render
        // as part of the selection (active or selected).
        await expect(page.locator('.status-bar')).toContainText('Cell: C3');
        const highlighted = page.locator('.cell.active, .cell.selected');
        // At minimum: the new active (C3) plus the original anchor (A1).
        expect(await highlighted.count()).toBeGreaterThanOrEqual(2);
    });

    test('Extend selection highlights the rectangle from the anchor', async ({ page }) => {
        await cellAt(page, 0, 0).click();
        // (row=2, col=1) → B3. Selection rect is A1..B3 → 3 rows × 2 cols = 6 cells.
        await page.getByRole('checkbox', { name: 'Extend selection', exact: true }).check();
        await cellAt(page, 2, 1).click();

        const highlighted = page.locator('.cell.active, .cell.selected');
        expect(await highlighted.count()).toBeGreaterThanOrEqual(6);
    });

    test('arrow keys move the active cell', async ({ page }) => {
        await cellAt(page, 0, 0).click();

        await page.keyboard.press('ArrowDown');
        await page.keyboard.press('ArrowDown');

        await expect(page.locator('.status-bar')).toContainText('Cell: A3');
    });

    test('Escape clears editing state', async ({ page }) => {
        await cellAt(page, 0, 0).click();
        await page.getByRole('textbox', { name: 'Formula', exact: true }).focus();
        await page.keyboard.type('xyz');
        await page.keyboard.press('Escape');

        // Formula bar should be restored to the cell's original (empty) value.
        await expect(page.locator('#calc-formula-input')).toHaveValue('');
        // And the cell should still display nothing.
        await expect(cellAt(page, 0, 0)).toHaveText('');
    });

    test('clicking f(x) prepends = and focuses the formula bar', async ({ page }) => {
        await cellAt(page, 0, 0).click();
        await page.locator('.formula-icon').click();

        await expect(page.locator('#calc-formula-input')).toHaveValue('=');
        const focusedId = await page.evaluate(() => document.activeElement?.id);
        expect(focusedId).toBe('calc-formula-input');
    });

    test('clicking f(x) when formula already starts with = leaves value unchanged', async ({ page }) => {
        await cellAt(page, 0, 0).click();
        await page.getByRole('textbox', { name: 'Formula', exact: true }).focus();
        await page.keyboard.type('=SUM(A1:A3)');
        await page.locator('.formula-icon').click();

        await expect(page.locator('#calc-formula-input')).toHaveValue('=SUM(A1:A3)');
    });

    test('saves values, formatting and merged ranges and restores them on reload', async ({ page, request }) => {
        await cellAt(page, 0, 0).click();
        await page.getByRole('textbox', { name: 'Formula', exact: true }).fill('42');
        await page.getByRole('button', { name: 'Apply formula', exact: true }).click();
        await page.getByRole('button', { name: 'Bold', exact: true }).click();
        await page.getByRole('textbox', { name: 'Selection range', exact: true }).fill('A1:B2');
        await page.getByRole('button', { name: 'Select range', exact: true }).click();
        await page.getByRole('button', { name: 'Merge cells', exact: true }).click();

        await expect(page.locator('.cell.active')).toHaveAttribute('rowspan', '2');
        await expect(page.locator('.cell.active')).toHaveAttribute('colspan', '2');
        await expect.poll(async () => {
            const response = await request.get(`${API_BASE}/documents/${docId}`, {
                headers: { Authorization: `Bearer ${token}` },
            });
            const body = await response.json();
            const sheet = body.data?.content?.sheets?.[0];
            return {
                value: sheet?.cells?.[0]?.[1]?.value?.value,
                bold: sheet?.cells?.[0]?.[1]?.format?.bold,
                merged: sheet?.merged_cells?.length,
            };
        }, { timeout: 15000 }).toEqual({ value: 42, bold: true, merged: 1 });

        await page.reload();
        await page.locator('.sheet-grid').waitFor({ state: 'visible' });
        await expect(cellAt(page, 0, 0)).toHaveText('42');
        await expect(page.locator('.cell.active')).toHaveClass(/cell-bold/);
        await expect(page.locator('.cell.active')).toHaveAttribute('colspan', '2');
    });
});

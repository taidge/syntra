// Calc page UI tests.
//
// These tests assume a contrix-server is already running at http://localhost:3000
// (it serves both the JSON API and the Dioxus single-page app). Start it with
// `cargo run -p contrix-server` from the workspace root before invoking
// `npx playwright test calc.ui`. The playwright.config.ts `webServer` block
// will reuse the existing server when one is detected.

import { test, expect, Page, APIRequestContext } from '@playwright/test';

const API_BASE = 'http://localhost:3000/api/v1';
const APP_BASE = 'http://localhost:3000';

// Must match `AUTH_STORAGE_KEY` in crates/frontend/src/state.rs.
const AUTH_STORAGE_KEY = 'contrix.auth';

interface CalcFixture {
    id: string;
    token: string;
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
    return { id: docBody.data.id, token };
}

async function gotoCalc(page: Page, id: string, token: string) {
    // Pre-seed auth state so the Dioxus app finds the token in localStorage
    // before its first render and skips the redirect to /login.
    await page.addInitScript(
        ({ key, tok }) => {
            localStorage.setItem(
                key,
                JSON.stringify({
                    token: tok,
                    user: {
                        id: '00000000-0000-0000-0000-000000000000',
                        username: 'u',
                        email: 'u@u',
                        display_name: null,
                        role: 'editor',
                    },
                }),
            );
        },
        { key: AUTH_STORAGE_KEY, tok: token },
    );
    await page.goto(`${APP_BASE}/calc/${id}`);
    // Wait for the spreadsheet to actually render before any interaction.
    await page.locator('.sheet-grid').waitFor({ state: 'visible', timeout: 15000 });
}

/** Locate a body cell by zero-based (rowIdx, colIdx). */
function cellAt(page: Page, row: number, col: number) {
    return page.locator('.sheet-grid tbody tr').nth(row).locator('.cell').nth(col);
}

test.describe('Calc page UI @ui', () => {
    let docId: string;
    let token: string;

    test.beforeEach(async ({ request, page }) => {
        const fixture = await createCalcDoc(request);
        docId = fixture.id;
        token = fixture.token;
        await gotoCalc(page, docId, token);
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

    test('double-click focuses the formula bar', async ({ page }) => {
        await cellAt(page, 1, 1).dblclick();

        await expect
            .poll(async () => await page.evaluate(() => document.activeElement?.id ?? ''))
            .toBe('calc-formula-input');
    });

    test('typing with a cell selected routes to the formula bar', async ({ page }) => {
        await cellAt(page, 0, 0).click();

        await page.keyboard.press('5');

        const formula = page.locator('#calc-formula-input');
        await expect(formula).toHaveValue('5');

        await page.keyboard.press('Enter');

        // Cell A1 shows the committed value...
        await expect(cellAt(page, 0, 0)).toHaveText('5');
        // ...and the active cell has moved down to A2.
        await expect(page.locator('.status-bar')).toContainText('Cell: A2');
    });

    test('fill handle is visible on the active cell', async ({ page }) => {
        await cellAt(page, 0, 0).click();

        const handle = page.locator('.cell.active .autofill-handle');
        await expect(handle).toBeVisible();
    });

    test('Ctrl-click adds a non-contiguous cell to the selection', async ({ page }) => {
        await cellAt(page, 0, 0).click();
        await cellAt(page, 2, 2).click({ modifiers: ['Control'] });

        // C3 became the active cell after the Ctrl-click; A1 stays in the
        // extra-selection set. The visual contract is that both cells render
        // as part of the selection (active or selected).
        await expect(page.locator('.status-bar')).toContainText('Cell: C3');
        const highlighted = page.locator('.cell.active, .cell.selected');
        // At minimum: the new active (C3) plus the original anchor (A1).
        expect(await highlighted.count()).toBeGreaterThanOrEqual(2);
    });

    test('Shift-click extends selection from anchor to clicked cell', async ({ page }) => {
        await cellAt(page, 0, 0).click();
        // (row=2, col=1) → B3. Selection rect is A1..B3 → 3 rows × 2 cols = 6 cells.
        await cellAt(page, 2, 1).click({ modifiers: ['Shift'] });

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
        await cellAt(page, 0, 0).dblclick();
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
        await cellAt(page, 0, 0).dblclick();
        await page.keyboard.type('=SUM(A1:A3)');
        await page.locator('.formula-icon').click();

        await expect(page.locator('#calc-formula-input')).toHaveValue('=SUM(A1:A3)');
    });
});

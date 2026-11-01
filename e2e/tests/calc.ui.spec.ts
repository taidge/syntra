// Calc semantic-control UI tests.
// Explicit selection controls and Fill actions cover spreadsheet range editing.
// Browser gestures also support double-click, F2, and typed-character inline edits.
//
// These tests assume a contrix-server is already running at http://localhost:3000
// (it serves both the JSON API and the Glory + Sapid single-page app). Start it with
// `cargo run -p contrix-server` from the workspace root before invoking
// `npx playwright test calc.ui`. The playwright.config.ts `webServer` block
// will reuse the existing server when one is detected.

import { test, expect, Page, Locator, APIRequestContext } from '@playwright/test';

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

/** Toolbar commands remain real buttons when moved into the overflow popover. */
async function clickToolbarAction(page: Page, name: string) {
    const action = page.getByRole('button', { name, exact: true });
    const more = page.getByRole('button', { name: /^More tools \(/ });
    if (!await action.isVisible()) {
        await more.click();
    }
    await expect(action).toBeVisible();
    await action.click();
    if (await more.isVisible() && await more.getAttribute('aria-expanded') === 'true') {
        await page.keyboard.press('Escape');
    }
}

async function expectReadableText(control: Locator) {
    const contrast = await control.evaluate(element => {
        const rgba = (value: string) => {
            const parts = value.match(/[\d.]+/g)!.map(Number);
            return [parts[0], parts[1], parts[2], parts[3] ?? 1];
        };
        const backgrounds: number[][] = [];
        for (let node: Element | null = element; node; node = node.parentElement) {
            backgrounds.push(rgba(getComputedStyle(node).backgroundColor));
        }
        const compose = (front: number[], back: number[]) => front.slice(0, 3)
            .map((channel, index) => channel * front[3] + back[index] * (1 - front[3]));
        let background = [255, 255, 255];
        for (const layer of backgrounds.reverse()) background = compose(layer, background);
        const foreground = compose(rgba(getComputedStyle(element).color), background);
        const luminance = (rgb: number[]) => rgb.map(channel => channel / 255)
            .map(channel => channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4)
            .reduce((sum, channel, index) => sum + channel * [0.2126, 0.7152, 0.0722][index], 0);
        const levels = [luminance(foreground), luminance(background)].sort((a, b) => a - b);
        return (levels[1] + 0.05) / (levels[0] + 0.05);
    });
    expect(contrast, `${await control.getAttribute('aria-label') ?? 'Control'} text contrast`).toBeGreaterThanOrEqual(4.5);
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

    test('double-click edits a cell inline and Enter commits into the next row', async ({ page }) => {
        await cellAt(page, 0, 0).dblclick();
        const editor = page.getByRole('textbox', { name: 'Edit A1', exact: true });
        await expect(editor).toBeVisible();
        await expect(editor).toBeFocused();
        await editor.fill('12');
        await editor.press('Enter');

        await expect(editor).toBeHidden();
        await expect(cellAt(page, 0, 0)).toHaveText('12');
        await expect(page.locator('.status-bar')).toContainText('Cell: A2');
    });

    test('Escape cancels inline edits and restores the committed value', async ({ page }) => {
        await cellAt(page, 0, 0).click();
        await page.getByRole('textbox', { name: 'Formula', exact: true }).fill('Original');
        await page.getByRole('button', { name: 'Apply formula', exact: true }).click();
        await cellAt(page, 0, 0).dblclick();
        const editor = page.getByRole('textbox', { name: 'Edit A1', exact: true });
        await expect(editor).toHaveValue('Original');
        await editor.fill('Discarded');
        await editor.press('Escape');

        await expect(editor).toBeHidden();
        await expect(cellAt(page, 0, 0)).toHaveText('Original');
        await expect(page.getByRole('textbox', { name: 'Formula', exact: true })).toHaveValue('Original');
        await expect(page.locator('.status-bar')).toContainText('Cell: A1');
    });

    test('F2 opens inline editing for the active cell', async ({ page }) => {
        await cellAt(page, 0, 0).click();
        await page.keyboard.press('F2');
        const editor = page.getByRole('textbox', { name: 'Edit A1', exact: true });
        await expect(editor).toBeVisible();
        await expect(editor).toBeFocused();
        await editor.fill('Keyboard edit');
        await editor.press('Enter');

        await expect(cellAt(page, 0, 0)).toHaveText('Keyboard edit');
        await expect(page.locator('.status-bar')).toContainText('Cell: A2');
    });

    test('Tab commits inline editing and moves to the next column', async ({ page }) => {
        await cellAt(page, 0, 0).dblclick();
        const editor = page.getByRole('textbox', { name: 'Edit A1', exact: true });
        await expect(editor).toBeFocused();
        await editor.fill('Across');
        await editor.press('Tab');

        await expect(editor).toBeHidden();
        await expect(cellAt(page, 0, 0)).toHaveText('Across');
        await expect(page.locator('.status-bar')).toContainText('Cell: B1');
        await expect(cellAt(page, 0, 1)).toBeFocused();
        await page.keyboard.press('Shift+Tab');
        await expect(page.locator('.status-bar')).toContainText('Cell: A1');
    });

    test('clicking another cell commits the inline draft into its original cell', async ({ page }) => {
        await cellAt(page, 0, 0).dblclick();
        const editor = page.getByRole('textbox', { name: 'Edit A1', exact: true });
        await expect(editor).toBeFocused();
        await editor.fill('Committed on blur');
        await cellAt(page, 1, 1).click();

        await expect(editor).toBeHidden();
        await expect(cellAt(page, 0, 0)).toHaveText('Committed on blur');
        await expect(cellAt(page, 1, 1)).toHaveText('');
        await expect(page.locator('.status-bar')).toContainText('Cell: B2');
    });

    test('typing on a selected cell starts inline editing with the typed character', async ({ page }) => {
        await cellAt(page, 0, 0).click();
        await page.getByRole('textbox', { name: 'Formula', exact: true }).fill('Old value');
        await page.getByRole('button', { name: 'Apply formula', exact: true }).click();
        await cellAt(page, 0, 0).click();
        await page.keyboard.press('x');
        const editor = page.getByRole('textbox', { name: 'Edit A1', exact: true });
        await expect(editor).toBeFocused();
        await expect(editor).toHaveValue('x');
        await page.keyboard.type('yz');
        await expect(editor).toHaveValue('xyz');
        await editor.press('Enter');

        await expect(cellAt(page, 0, 0)).toHaveText('xyz');
        await expect(page.locator('.status-bar')).toContainText('Cell: A2');
    });

    test('rapid typing preserves every character before inline focus settles', async ({ page }) => {
        await cellAt(page, 0, 0).click();
        await page.keyboard.type('rapid');
        await page.keyboard.press('Enter');

        await expect(page.getByRole('textbox', { name: 'Edit A1', exact: true })).toBeHidden();
        await expect(cellAt(page, 0, 0)).toHaveText('rapid');
        await expect(page.locator('.status-bar')).toContainText('Cell: A2');
    });

    test('numeric validation keeps an invalid inline draft editable without saving it', async ({ page, request }) => {
        const savedSheet = async () => {
            const response = await request.get(`${API_BASE}/documents/${docId}`, {
                headers: { Authorization: `Bearer ${token}` },
            });
            expect(response.ok()).toBeTruthy();
            return (await response.json()).data.content.sheets[0];
        };
        await cellAt(page, 0, 0).click();
        await page.getByRole('button', { name: 'Tools', exact: true }).click();
        await page.getByRole('menuitem', { name: 'Data tools', exact: true }).click();
        await page.getByRole('button', { name: 'Validate numbers only', exact: true }).click();
        await expect.poll(async () => (await savedSheet()).data_validations.length).toBe(1);
        await page.getByRole('button', { name: 'Tools', exact: true }).click();
        await page.getByRole('menuitem', { name: 'Data tools', exact: true }).click();

        await cellAt(page, 0, 0).dblclick();
        const editor = page.getByRole('textbox', { name: 'Edit A1', exact: true });
        await expect(editor).toBeFocused();
        await editor.fill('invalid');
        await editor.press('Enter');
        await expect(editor).toBeVisible();
        await expect(editor).toBeFocused();
        await expect(editor).toHaveValue('invalid');
        await expect(page.locator('.status-bar')).toContainText('Cell: A1');
        expect((await savedSheet()).cells).toEqual([]);
        await editor.press('Escape');
        await expect(editor).toBeHidden();
        await expect(cellAt(page, 0, 0)).toHaveText('');
        await expect(page.getByRole('textbox', { name: 'Formula', exact: true })).toHaveValue('');

        await cellAt(page, 0, 0).dblclick();
        await expect(editor).toBeFocused();
        await editor.fill('42');
        await editor.press('Enter');
        await expect(editor).toBeHidden();
        await expect(cellAt(page, 0, 0)).toHaveText('42');
        await expect(page.locator('.status-bar')).toContainText('Cell: A2');
        await expect.poll(async () => (await savedSheet()).cells[0]?.[1]?.value?.value).toBe(42);
    });

    test('text fields and menus stay readable in light and dark themes', async ({ page }) => {
        for (const theme of ['light', 'dark']) {
            const currentTheme = await page.locator('html').getAttribute('data-theme') ?? 'light';
            if (currentTheme !== theme) {
                await page.getByRole('button', { name: 'Toggle dark mode', exact: true }).click();
            }
            await expect.poll(async () => await page.locator('html').getAttribute('data-theme') ?? 'light').toBe(theme);
            await expect(page.locator('.sheet-grid')).toBeVisible();
            await expectReadableText(page.getByRole('textbox', { name: 'Formula', exact: true }));
            await expectReadableText(page.getByRole('textbox', { name: 'Font family', exact: true }));
            await cellAt(page, 0, 0).dblclick();
            const editor = page.getByRole('textbox', { name: 'Edit A1', exact: true });
            await expect(editor).toBeVisible();
            await expectReadableText(editor);
            await page.screenshot({ path: test.info().outputPath(`calc-${theme}-inline.png`) });
            await editor.press('Escape');
            await page.getByRole('button', { name: 'View', exact: true }).click();
            const selectionTools = page.getByRole('menuitem', { name: 'Selection tools', exact: true });
            await expect(selectionTools).toBeVisible();
            await expectReadableText(selectionTools);
            await page.screenshot({ path: test.info().outputPath(`calc-${theme}-menu.png`) });
            await page.keyboard.press('Escape');
        }
    });

    test('Fill down extends the anchor value through a selected range', async ({ page }) => {
        await cellAt(page, 0, 0).click();
        await page.getByRole('textbox', { name: 'Formula', exact: true }).fill('7');
        await page.getByRole('button', { name: 'Apply formula', exact: true }).click();
        await page.getByRole('button', { name: 'View', exact: true }).click();
        await page.getByRole('menuitem', { name: 'Selection tools', exact: true }).click();
        await page.getByRole('textbox', { name: 'Selection range', exact: true }).fill('A1:A3');
        await page.getByRole('button', { name: 'Select range', exact: true }).click();
        await clickToolbarAction(page, 'Fill down');

        await expect(cellAt(page, 0, 0)).toHaveText('7');
        await expect(cellAt(page, 1, 0)).toHaveText('7');
        await expect(cellAt(page, 2, 0)).toHaveText('7');
    });

    test('Add cells to selection retains a non-contiguous selection', async ({ page }) => {
        await cellAt(page, 0, 0).click();
        await page.getByRole('button', { name: 'View', exact: true }).click();
        await page.getByRole('menuitem', { name: 'Selection tools', exact: true }).click();
        await page.getByRole('checkbox', { name: 'Add cells to selection', exact: true }).check();
        await cellAt(page, 2, 2).click();

        // C3 became the active cell while additive selection was enabled; A1 stays in the
        // extra-selection set. The visual contract is that both cells render
        // as part of the selection (active or selected).
        await expect(page.locator('.status-bar')).toContainText('Cell: C3');
        const highlighted = page.locator('.cell.active, .cell.selected');
        // At minimum: the new active (C3) plus the original anchor (A1).
        await expect.poll(() => highlighted.count()).toBeGreaterThanOrEqual(2);
    });

    test('Extend selection highlights the rectangle from the anchor', async ({ page }) => {
        await cellAt(page, 0, 0).click();
        // (row=2, col=1) → B3. Selection rect is A1..B3 → 3 rows × 2 cols = 6 cells.
        await page.getByRole('button', { name: 'View', exact: true }).click();
        await page.getByRole('menuitem', { name: 'Selection tools', exact: true }).click();
        await page.getByRole('checkbox', { name: 'Extend selection', exact: true }).check();
        await cellAt(page, 2, 1).click();

        const highlighted = page.locator('.cell.active, .cell.selected');
        await expect.poll(() => highlighted.count()).toBeGreaterThanOrEqual(6);
    });

    test('arrow keys move the active cell', async ({ page }) => {
        await cellAt(page, 0, 0).click();

        await page.keyboard.press('ArrowDown');
        await page.keyboard.press('ArrowDown');

        await expect(page.locator('.status-bar')).toContainText('Cell: A3');
    });

    test('Shift+Arrow selects a rectangle and Delete clears every selected value', async ({ page }) => {
        for (const [row, col, value] of [[0, 0, 'A'], [0, 1, 'B'], [1, 0, 'C'], [1, 1, 'D']] as const) {
            await cellAt(page, row, col).click();
            await page.getByRole('textbox', { name: 'Formula', exact: true }).fill(value);
            await page.getByRole('button', { name: 'Apply formula', exact: true }).click();
            await expect(cellAt(page, row, col)).toHaveText(value);
        }
        await cellAt(page, 0, 0).click();
        await page.keyboard.press('Shift+ArrowRight');
        await page.keyboard.press('Shift+ArrowDown');
        await expect.poll(() => page.locator('.cell.active, .cell.selected').count()).toBeGreaterThanOrEqual(4);
        await page.keyboard.press('Delete');

        for (const [row, col] of [[0, 0], [0, 1], [1, 0], [1, 1]]) {
            await expect(cellAt(page, row, col)).toHaveText('');
        }
    });

    test('ArrowRight skips the full width of a merged cell', async ({ page }) => {
        await page.getByRole('button', { name: 'View', exact: true }).click();
        await page.getByRole('menuitem', { name: 'Selection tools', exact: true }).click();
        await page.getByRole('textbox', { name: 'Selection range', exact: true }).fill('A1:B2');
        await page.getByRole('button', { name: 'Select range', exact: true }).click();
        await clickToolbarAction(page, 'Merge cells');
        await expect(page.locator('.cell.active')).toHaveAttribute('colspan', '2');
        await page.getByRole('button', { name: 'A1', exact: true }).click();
        await page.keyboard.press('ArrowRight');

        await expect(page.locator('.status-bar')).toContainText('Cell: C1');
        await expect(page.getByRole('button', { name: 'C1', exact: true })).toBeFocused();
    });

    test('adding and renaming sheets preserves their saved identities', async ({ page, request }) => {
        const savedSheets = async () => {
            const response = await request.get(`${API_BASE}/documents/${docId}`, {
                headers: { Authorization: `Bearer ${token}` },
            });
            expect(response.ok()).toBeTruthy();
            return (await response.json()).data.content.sheets as { id: string; name: string }[];
        };
        await page.getByRole('button', { name: 'View', exact: true }).click();
        await page.getByRole('menuitem', { name: 'Sheet tools', exact: true }).click();
        const name = page.getByRole('textbox', { name: 'Sheet name', exact: true });
        await expect(name).toHaveValue('Sheet1');
        const initialId = (await savedSheets())[0].id;

        await page.getByRole('button', { name: 'Add sheet', exact: true }).click();
        await expect(page.locator('.sheet-tab.active')).toHaveAccessibleName('Sheet2');
        await expect(name).toHaveValue('Sheet2');
        await expect.poll(async () => (await savedSheets()).length).toBe(2);
        const firstIds = (await savedSheets()).map(sheet => sheet.id);
        expect(firstIds[0]).toBe(initialId);
        expect(firstIds[1]).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i);

        await name.fill('Forecast');
        await page.getByRole('button', { name: 'Rename sheet', exact: true }).click();
        await expect(page.locator('.sheet-tab.active')).toHaveAccessibleName('Forecast');
        await page.getByRole('button', { name: 'Sheet1', exact: true }).click();
        await expect(name).toHaveValue('Sheet1');
        await page.getByRole('button', { name: 'Forecast', exact: true }).click();
        await expect(name).toHaveValue('Forecast');
        await page.getByRole('button', { name: 'Add sheet', exact: true }).click();
        await expect(page.locator('.sheet-tab.active')).toHaveAccessibleName('Sheet3');
        await expect(name).toHaveValue('Sheet3');

        await expect.poll(async () => (await savedSheets()).map(sheet => sheet.name))
            .toEqual(['Sheet1', 'Forecast', 'Sheet3']);
        const finalIds = (await savedSheets()).map(sheet => sheet.id);
        expect(finalIds.slice(0, 2)).toEqual(firstIds);
        expect(new Set(finalIds).size).toBe(3);
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
        await clickToolbarAction(page, 'Bold');
        await page.getByRole('button', { name: 'View', exact: true }).click();
        await page.getByRole('menuitem', { name: 'Selection tools', exact: true }).click();
        await page.getByRole('textbox', { name: 'Selection range', exact: true }).fill('A1:B2');
        await page.getByRole('button', { name: 'Select range', exact: true }).click();
        await clickToolbarAction(page, 'Merge cells');

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

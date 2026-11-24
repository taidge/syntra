import { test, expect, APIRequestContext, Page } from '@playwright/test';
import { resolve } from 'node:path';
import { readFile } from 'node:fs/promises';
import { registerUser, createDocument, uniqueEmail, uniqueUsername } from './helpers';
const BASE = (process.env.API_URL || 'http://localhost:3000').replace(/\/$/, '');
async function authenticate(page: Page, token: string, user: unknown) {
  await page.addInitScript(({token,user}) => { localStorage.setItem('syntra.auth', JSON.stringify({token,user})); localStorage.setItem('locale','en'); }, {token,user});
}
async function editCell(page:Page, address:string, text:string) {
  await page.getByRole('button',{name:address,exact:true}).click();
  await page.getByRole('textbox',{name:'Formula',exact:true}).fill(text);
  await page.getByRole('button',{name:'Apply formula',exact:true}).click();
  await expect(page.getByRole('button',{name:address,exact:true})).toHaveText(text);
}
test.describe('Review browser regressions @ui', () => {
  test('Calc saves and downloads recalculated CSV text, errors and dependent formulas', async ({ page, request }) => {
    const auth = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');
    const headers = { Authorization: `Bearer ${auth.token}` };
    const user = (await (await request.get('/api/v1/auth/me', { headers })).json()).data;
    const { body } = await createDocument(request, auth.token, 'Saved CSV values', 'calc');
    const id = body.data.id;
    await authenticate(page, auth.token, user);
    await page.goto(`${BASE}/calc/${id}`);
    await expect(page.locator('.save-status')).toHaveText('Saved');
    const cells = [
      ['A1', '1', '1'], ['A2', '2', '2'], ['A3', '=SUM(A1:A2)', '3'],
      ['B1', 'quoted, "text"', 'quoted, "text"'], ['B2', '=B1', 'quoted, "text"'],
      ['B3', '=A3*2', '6'], ['C1', '=1/0', '#ERR: #DIV/0!'], ['C2', '=C1', '#ERR: #DIV/0!'],
      ['D1', '=SUM(C1:C2)', '#ERR: #DIV/0!'], ['D2', '=AVERAGE(C1:C2)', '#ERR: #DIV/0!'],
      ['D3', '=MIN(C1:C2)', '#ERR: #DIV/0!'], ['E1', '=MAX(C1:C2)', '#ERR: #DIV/0!'],
      ['E2', '=IF(C1,1,2)', '#ERR: #DIV/0!'], ['E3', '=IF(1,7,C1)', '7'],
      ['D4', '=IF(TRUE,42,D4)', '42'], ['E4', '=D4+1', '43'],
    ];
    for (const [address, input, displayed] of cells) {
      await page.getByRole('button', { name: address, exact: true }).click();
      await page.getByRole('textbox', { name: 'Formula', exact: true }).fill(input);
      await page.getByRole('button', { name: 'Apply formula', exact: true }).click();
      await expect(page.getByRole('button', { name: address, exact: true })).toHaveText(displayed);
    }
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.locator('.save-status')).toHaveText('Saved');
    const persisted = (await (await request.get(`/api/v1/documents/${id}`, { headers })).json()).data;
    const saved = persisted.content.sheets[0].cells;
    expect(saved.find((pair: any) => pair[0].row === 2 && pair[0].col === 0)[1].formula).toBe('=SUM(A1:A2)');
    expect(saved.find((pair: any) => pair[0].row === 1 && pair[0].col === 2)[1].formula).toBe('=C1');
    await page.reload();
    await expect(page.locator('.save-status')).toHaveText('Saved');
    for (const [address, , displayed] of cells) {
      await expect(page.getByRole('button', { name: address, exact: true })).toHaveText(displayed);
    }
    const response = page.waitForResponse(value => value.url().endsWith(`/api/v1/documents/${id}/export/csv`) && value.request().method() === 'POST');
    const download = page.waitForEvent('download');
    await page.getByRole('button', { name: 'Export csv', exact: true }).click();
    const exported = await response;
    expect(exported.status()).toBe(200);
    expect(exported.request().headers().authorization).toBe(headers.Authorization);
    const path = await (await download).path();
    expect(path).not.toBeNull();
    expect(await readFile(path!, 'utf8')).toBe('1,"quoted, ""text""",#DIV/0!,#DIV/0!,#DIV/0!\r\n2,"quoted, ""text""",#DIV/0!,#DIV/0!,#DIV/0!\r\n3,6,,#DIV/0!,7\r\n,,,42,43\r\n');
  });

  test('two browser sessions keep local dirty edits until a persistent snapshot conflict is resolved', async ({browser, request}) => {
    const auth=await registerUser(request,uniqueUsername(),uniqueEmail(),'Test@12345');
    const user=(await (await request.get('/api/v1/auth/me',{headers:{Authorization:`Bearer ${auth.token}`}})).json()).data;
    const {body}=await createDocument(request,auth.token,'Two browsers','calc');
    const id=body.data.id;
    const first=await browser.newContext(); const second=await browser.newContext();
    let release:()=>void=()=>{};
    try {
      const local=await first.newPage(); const peer=await second.newPage();
      await authenticate(local,auth.token,user); await authenticate(peer,auth.token,user);
      await local.goto(`${BASE}/calc/${id}`); await peer.goto(`${BASE}/calc/${id}`);
      await expect(local.locator('.sheet-grid')).toBeVisible(); await expect(peer.locator('.sheet-grid')).toBeVisible();
      await expect(local.locator('.save-status')).toHaveText('Saved'); await expect(peer.locator('.save-status')).toHaveText('Saved');
      await expect(local.getByText('Connected',{exact:true})).toBeVisible(); await expect(peer.getByText('Connected',{exact:true})).toBeVisible();
      const gate=new Promise<void>(resolve=>release=resolve); let blocked=false;
      await local.route(`**/api/v1/documents/${id}`,async route=> { if(route.request().method()==='PUT' && !blocked){blocked=true;await gate;} await route.continue(); });
      await editCell(local,'A1','local pending');
      await local.getByRole('button',{name:'Save',exact:true}).click();
      await expect.poll(()=>blocked).toBe(true);
      await editCell(peer,'A1','remote committed');
      await peer.getByRole('button',{name:'Save',exact:true}).click();
      await expect(peer.locator('.save-status')).toHaveText('Saved');
      await expect(local.getByRole('button',{name:'Use server version',exact:true})).toBeVisible();
      await expect(local.getByRole('button',{name:'A1',exact:true})).toHaveText('local pending');
      release(); await expect(local.locator('.save-status')).toContainText('Save failed');
      await expect(local.getByRole('button',{name:'A1',exact:true})).toHaveText('local pending');
      await local.getByRole('button',{name:'Use server version',exact:true}).click();
      await expect(local.getByRole('button',{name:'A1',exact:true})).toHaveText('remote committed');
    } finally { release(); await first.close(); await second.close(); }
  });

  test('two browser sessions automatically merge disjoint dirty cells and retry a stale in-flight save',async({browser,request})=>{
    const auth=await registerUser(request,uniqueUsername(),uniqueEmail(),'Test@12345');
    const headers={Authorization:`Bearer ${auth.token}`};
    const user=(await (await request.get('/api/v1/auth/me',{headers})).json()).data;
    const {body}=await createDocument(request,auth.token,'Disjoint snapshots','calc');const id=body.data.id;
    const first=await browser.newContext();const second=await browser.newContext();let release:()=>void=()=>{};
    try {
      const local=await first.newPage();const peer=await second.newPage();
      await authenticate(local,auth.token,user);await authenticate(peer,auth.token,user);
      await local.goto(`${BASE}/calc/${id}`);await peer.goto(`${BASE}/calc/${id}`);
      for(const page of [local,peer]) {await expect(page.locator('.save-status')).toHaveText('Saved');await expect(page.getByText('Connected',{exact:true})).toBeVisible();}
      let blocked=false;const gate=new Promise<void>(resolve=>release=resolve);
      await local.route(`**/api/v1/documents/${id}`,async route=>{if(route.request().method()==='PUT'&&!blocked){blocked=true;await gate;}await route.continue();});
      await editCell(local,'A1','local pending');await local.getByRole('button',{name:'Save',exact:true}).click();await expect.poll(()=>blocked).toBe(true);
      await editCell(peer,'B1','remote committed');await peer.getByRole('button',{name:'Save',exact:true}).click();await expect(peer.locator('.save-status')).toHaveText('Saved');
      await expect(local.getByRole('button',{name:'B1',exact:true})).toHaveText('remote committed');
      await expect(local.getByRole('button',{name:'A1',exact:true})).toHaveText('local pending');
      await expect(local.getByRole('button',{name:'Use server version',exact:true})).toHaveCount(0);
      release();
      await expect.poll(async()=>{
        const document=(await (await request.get(`/api/v1/documents/${id}`,{headers})).json()).data;
        const cells=document.content.sheets[0].cells;
        return [cells.find((pair:any)=>pair[0].row===0&&pair[0].col===0)?.[1].value.value,cells.find((pair:any)=>pair[0].row===0&&pair[0].col===1)?.[1].value.value];
      },{timeout:15000}).toEqual(['local pending','remote committed']);
      await expect(local.locator('.save-status')).toHaveText('Saved');
      await expect(peer.getByRole('button',{name:'A1',exact:true})).toHaveText('local pending');
    } finally {release();await first.close();await second.close();}
  });

  test('external PPTX import displays fidelity warnings and downloads the original with authentication',async({page,request})=>{
    const auth=await registerUser(request,uniqueUsername(),uniqueEmail(),'Test@12345');
    const user=(await (await request.get('/api/v1/auth/me',{headers:{Authorization:`Bearer ${auth.token}`}})).json()).data;
    await authenticate(page,auth.token,user);await page.goto(`${BASE}/files`);
    const picker=page.waitForEvent('filechooser');
    await page.getByRole('button',{name:'Upload',exact:true}).click();
    const uploaded=page.waitForResponse(response=>response.url().endsWith('/api/v1/files/upload')&&response.request().method()==='POST');
    await (await picker).setFiles(resolve(__dirname,'../../crates/export/tests/fixtures/external-rich-reordered.pptx'));
    expect((await uploaded).status()).toBe(201);
    await expect(page.locator('.import-warning')).toContainText('External media');
    const download=page.waitForEvent('download');
    await page.getByRole('button',{name:'Download original',exact:true}).click();
    const original = await download;
    expect(original.suggestedFilename()).toBe('external-rich-reordered.pptx');
    const originalPath = await original.path();
    expect(originalPath).not.toBeNull();
    expect(await readFile(originalPath!)).toEqual(await readFile(resolve(__dirname, '../../crates/export/tests/fixtures/external-rich-reordered.pptx')));
    await page.getByRole('link',{name:'external-rich-reordered.pptx',exact:true}).click();
    await expect(page).toHaveURL(/\/slides\//);
    await expect(page.locator('.slide-thumbnail')).toHaveCount(2);
    const leadingText = page.locator('.slide-canvas img[alt="TextBox"]').first();
    const leadingSvg = await leadingText.getAttribute('src');
    expect(Buffer.from(leadingSvg!.split(',')[1], 'base64').toString()).toContain('Second file, first slide');
    await page.getByRole('button', { name: /^Slide 2:/ }).click();
    const background=page.getByAltText('Slide preview',{exact:true});
    await expect.poll(()=>background.evaluate((image:HTMLImageElement)=>[image.naturalWidth,image.naturalHeight])).toEqual([1280,720]);
    const textObject=page.locator('.slide-canvas img[alt="TextBox"]').first();
    await expect.poll(async () => {
      const source = await textObject.getAttribute('src');
      return source ? Buffer.from(source.split(',')[1], 'base64').toString() : '';
    }).toContain('font-family="Georgia"');
    const svg=await textObject.getAttribute('src');
    const xml=Buffer.from(svg!.split(',')[1],'base64').toString();
    expect(xml).toContain('font-family="Georgia"');expect(xml).toContain('font-weight="bold"');expect(xml).toContain('#336699');
    await expect(page.locator('.slide-canvas img[alt="Chart"]')).toHaveCount(1);

  });
});

test.describe('Ambiguous document saves @ui', () => {
  test.use({ serviceWorkers: 'block' });

  const unavailable = {
    status: 503,
    json: { success: false, error: { code: 503, message: 'Service unavailable' } },
  };
  const unconfirmed = /Save could not be confirmed/;

  async function openEditor(page: Page, request: APIRequestContext, kind: 'writer' | 'calc') {
    const auth = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');
    const headers = { Authorization: `Bearer ${auth.token}` };
    const user = (await (await request.get('/api/v1/auth/me', { headers })).json()).data;
    const { response, body } = await createDocument(request, auth.token, 'Recover a save', kind);
    expect(response.status()).toBe(201);
    await authenticate(page, auth.token, user);
    // The actual REST write must be verified even when no collaboration echo arrives.
    await page.routeWebSocket('**/api/v1/ws/collab/**', () => {});
    await page.goto(`${BASE}/${kind}/${body.data.id}`);
    await expect(page.locator('.save-status')).toHaveText(kind === 'writer' ? 'All changes saved' : 'Saved');
    return { document: body.data, headers, endpoint: `/api/v1/documents/${body.data.id}` };
  }

  test('Writer confirms a committed 503 and keeps edits typed during verification', async ({ page, request }) => {
    const fixture = await openEditor(page, request, 'writer');
    const writes: { expected_version: number; content: unknown }[] = [];
    let reads = 0;
    let release = () => {};
    const gate = new Promise<void>(resolve => { release = resolve; });
    await page.route(`**${fixture.endpoint}`, async route => {
      if (route.request().method() === 'PUT') {
        writes.push(route.request().postDataJSON());
        if (writes.length === 1) {
          expect((await route.fetch({ maxRetries: 0 })).status()).toBe(200);
          await route.fulfill(unavailable);
          return;
        }
      } else if (route.request().method() === 'GET') {
        const response = await route.fetch({ maxRetries: 0 });
        expect(response.status()).toBe(200);
        const body = await response.text();
        // Swap only the keys. JSON.stringify would also change values such as
        // Writer's 8.0 spacing from a floating-point token to an integer token.
        const reordered = body.replace(/("after":[0-9.eE+-]+),("before":[0-9.eE+-]+)/, '$2,$1');
        expect(reordered).not.toBe(body);
        expect(JSON.parse(reordered).data.content).toEqual(writes[0].content);
        reads++;
        await gate;
        await route.fulfill({ response, body: reordered });
        return;
      }
      await route.continue();
    });

    try {
      await page.clock.install();
      const editor = page.getByRole('textbox', { name: 'Document body', exact: true });
      const status = page.locator('.save-status');
      await editor.fill('First save was committed');
      await expect(status).toHaveText('Unsaved changes');
      await page.clock.fastForward(1_100);
      await expect.poll(() => reads).toBe(1);
      await editor.fill('Newer draft typed during verification');
      await expect(status).toHaveText('Unsaved changes');
      await page.clock.fastForward(1_100);
      expect(writes).toHaveLength(1);
      release();

      await expect.poll(() => writes.length).toBe(2);
      await expect(status).toHaveText('All changes saved');
      await page.clock.fastForward(2_000);
      expect(writes).toHaveLength(2);
      expect(writes[1].expected_version).toBe(writes[0].expected_version + 1);
      expect(writes[1].content).not.toEqual(writes[0].content);
      const persisted = await request.get(fixture.endpoint, { headers: fixture.headers });
      expect(persisted.status()).toBe(200);
      const body = (await persisted.json()).data;
      expect(body.version).toBe(writes[0].expected_version + 2);
      expect(body.content).toEqual(writes[1].content);
    } finally {
      release();
    }
  });

  test('Calc verifies a committed save after the transport drops the response', async ({ page, request }) => {
    const fixture = await openEditor(page, request, 'calc');
    let writes = 0;
    let reads = 0;
    await page.route(`**${fixture.endpoint}`, async route => {
      if (route.request().method() === 'PUT' && ++writes === 1) {
        expect((await route.fetch({ maxRetries: 0 })).status()).toBe(200);
        await route.abort('failed');
        return;
      }
      if (route.request().method() === 'GET') reads++;
      await route.continue();
    });
    await editCell(page, 'A1', 'Committed before disconnect');
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.locator('.save-status')).toHaveText('Saved');
    expect({ writes, reads }).toEqual({ writes: 1, reads: 1 });
    const persisted = await request.get(fixture.endpoint, { headers: fixture.headers });
    const body = (await persisted.json()).data;
    expect(body.version).toBe(fixture.document.version + 1);
    expect(body.content.sheets[0].cells[0][1].value.value).toBe('Committed before disconnect');
  });

  test('Calc merges a newer snapshot after a 503 and waits for an explicit save', async ({ page, request }) => {
    const fixture = await openEditor(page, request, 'calc');
    const writes: number[] = [];
    await page.route(`**${fixture.endpoint}`, async route => {
      if (route.request().method() === 'PUT') {
        writes.push(route.request().postDataJSON().expected_version);
        if (writes.length === 1) {
          const content = structuredClone(fixture.document.content);
          content.sheets[0].cells.push([{ row: 0, col: 1 }, { formula: null, value: { type: 'Text', value: 'Remote B1' } }]);
          const peer = await request.put(fixture.endpoint, {
            headers: fixture.headers,
            data: { content, expected_version: fixture.document.version },
          });
          expect(peer.status()).toBe(200);
          await route.fulfill(unavailable);
          return;
        }
      }
      await route.continue();
    });

    await page.clock.install();
    await editCell(page, 'A1', 'Local A1');
    const save = page.getByRole('button', { name: 'Save', exact: true });
    await save.click();
    await expect(page.locator('.save-status')).toHaveText(unconfirmed);
    await expect(page.getByRole('button', { name: 'A1', exact: true })).toHaveText('Local A1');
    await expect(page.getByRole('button', { name: 'B1', exact: true })).toHaveText('Remote B1');
    await page.clock.fastForward(2_000);
    expect(writes).toEqual([fixture.document.version]);

    await save.click();
    await expect(page.locator('.save-status')).toHaveText('Saved');
    expect(writes).toEqual([fixture.document.version, fixture.document.version + 1]);
  });

  for (const revoked of [false, true]) {
    test(`Writer retains edits when verification ${revoked ? 'loses authorization' : 'sees the same version'}`, async ({ page, request }) => {
      const fixture = await openEditor(page, request, 'writer');
      let writes = 0;
      let reads = 0;
      await page.route(`**${fixture.endpoint}`, async route => {
        if (route.request().method() === 'PUT' && ++writes === 1) {
          if (revoked) {
            expect((await route.fetch({ maxRetries: 0 })).status()).toBe(200);
            const current = await request.get('/api/v1/sessions/current', { headers: fixture.headers });
            expect(current.status()).toBe(200);
            const session = (await current.json()).data.id;
            expect((await request.delete(`/api/v1/sessions/${session}`, { headers: fixture.headers })).status()).toBe(200);
          }
          await route.fulfill(unavailable);
          return;
        }
        if (route.request().method() === 'GET') {
          reads++;
          const response = await route.fetch({ maxRetries: 0 });
          expect(response.status()).toBe(revoked ? 401 : 200);
          if (!revoked) expect((await response.json()).data.version).toBe(fixture.document.version);
          await route.fulfill({ response });
          return;
        }
        await route.continue();
      });

      await page.clock.install();
      const editor = page.getByRole('textbox', { name: 'Document body', exact: true });
      const status = page.locator('.save-status');
      await editor.fill('Local draft');
      await expect(status).toHaveText('Unsaved changes');
      await page.clock.fastForward(1_100);
      await expect(status).toHaveText(unconfirmed);
      await editor.fill('Still editing locally');
      await page.clock.fastForward(2_000);
      await expect(status).toHaveText(unconfirmed);
      expect({ writes, reads }).toEqual({ writes: 1, reads: 1 });
    });
  }
});

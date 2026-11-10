import { test, expect, Page } from '@playwright/test';
import { resolve } from 'node:path';
import { registerUser, createDocument, uniqueEmail, uniqueUsername } from './helpers';
const BASE = (process.env.API_URL || 'http://localhost:3000').replace(/\/$/, '');
async function authenticate(page: Page, token: string, user: unknown) {
  await page.addInitScript(({token,user}) => { localStorage.setItem('contrix.auth', JSON.stringify({token,user})); localStorage.setItem('locale','en'); }, {token,user});
}
async function editCell(page:Page, address:string, text:string) {
  await page.getByRole('button',{name:address,exact:true}).click();
  await page.getByRole('textbox',{name:'Formula',exact:true}).fill(text);
  await page.getByRole('button',{name:'Apply formula',exact:true}).click();
  await expect(page.getByRole('button',{name:address,exact:true})).toHaveText(text);
}
test.describe('Review browser regressions @ui', () => {
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
    await (await picker).setFiles(resolve(__dirname,'../../crates/export/tests/fixtures/external-rich.pptx'));
    expect((await uploaded).status()).toBe(201);
    await expect(page.locator('.import-warning')).toContainText('External media');
    const download=page.waitForEvent('download');
    await page.getByRole('button',{name:'Download original',exact:true}).click();
    expect((await download).suggestedFilename()).toBe('external-rich.pptx');
    await page.getByRole('button',{name:'external-rich.pptx',exact:true}).click();
    await expect(page).toHaveURL(/\/slides\//);
    const background=page.getByAltText('Slide preview',{exact:true});
    await expect.poll(()=>background.evaluate((image:HTMLImageElement)=>[image.naturalWidth,image.naturalHeight])).toEqual([1280,720]);
    const textObject=page.locator('.slide-canvas img[alt="TextBox"]').first();
    const svg=await textObject.getAttribute('src');
    const xml=Buffer.from(svg!.split(',')[1],'base64').toString();
    expect(xml).toContain('font-family="Georgia"');expect(xml).toContain('font-weight="bold"');expect(xml).toContain('#336699');
    await expect(page.locator('.slide-canvas img[alt="Chart"]')).toHaveCount(1);

  });
});

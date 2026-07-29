import { test, expect } from '@playwright/test';
import { registerUser, authHeaders, createDocument, uniqueEmail, uniqueUsername } from './helpers';

test.describe('Search API @api', () => {
  let token: string;

  test.beforeEach(async ({ request }) => {
    const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');
    token = user.token;
  });

  test('should search documents by name', async ({ request }) => {
    // Create documents with distinctive names
    await createDocument(request, token, 'Alpha Report', 'writer');
    await createDocument(request, token, 'Beta Analysis', 'calc');
    await createDocument(request, token, 'Alpha Slides', 'slides');

    const response = await request.get('/api/v1/search?q=Alpha', {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.success).toBe(true);
    expect(body.data.results.length).toBe(2);
    expect(body.data.total).toBe(2);
    expect(body.data.page).toBe(1);
  });

  test('should search with doc type filter', async ({ request }) => {
    await createDocument(request, token, 'Gamma Report', 'writer');
    await createDocument(request, token, 'Gamma Sheet', 'calc');

    const response = await request.get('/api/v1/search?q=Gamma&document_type=writer', {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.data.results.length).toBe(1);
    expect(body.data.results[0].document_type).toBe('writer');
  });

  test('should return empty results for non-matching query', async ({ request }) => {
    await createDocument(request, token, 'Some Doc', 'writer');

    const response = await request.get('/api/v1/search?q=zzzznonexistent', {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.data.results.length).toBe(0);
    expect(body.data.total).toBe(0);
  });

  test('should reject search without query parameter', async ({ request }) => {
    const response = await request.get('/api/v1/search?q=', {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(400);
  });

  test('should support pagination in search', async ({ request }) => {
    // Create enough docs for pagination
    for (let i = 0; i < 5; i++) {
      await createDocument(request, token, `Paginated Item ${i}`, 'writer');
    }

    const response = await request.get('/api/v1/search?q=Paginated&per_page=2&page=1', {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.data.results.length).toBe(2);
    expect(body.data.per_page).toBe(2);
    expect(body.data.total).toBe(5);

    // Get page 2
    const page2Resp = await request.get('/api/v1/search?q=Paginated&per_page=2&page=2', {
      headers: authHeaders(token),
    });
    const page2Body = await page2Resp.json();
    expect(page2Body.data.results.length).toBe(2);
    expect(page2Body.data.page).toBe(2);
  });

  test('should not return trashed documents in search', async ({ request }) => {
    const { body: doc } = await createDocument(request, token, 'TrashedSearchDoc', 'writer');
    const docId = doc.data.id;

    // Delete the document
    await request.delete(`/api/v1/documents/${docId}`, {
      headers: authHeaders(token),
    });

    const response = await request.get('/api/v1/search?q=TrashedSearchDoc', {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.data.results.length).toBe(0);
  });
});

import { test, expect } from '@playwright/test';
import { registerUser, authHeaders, createDocument, uniqueEmail, uniqueUsername } from './helpers';

test.describe('Review persistence regressions @api', () => {
  test('save and restore require the version the caller actually observed', async ({ request }) => {
    const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');
    const { body } = await createDocument(request, user.token, 'CAS', 'writer');
    const id = body.data.id;
    const url = `/api/v1/documents/${id}`;
    const headers = authHeaders(user.token);
    const content = { ...body.data.content, title: 'Committed once' };
    expect((await request.put(url, { headers, data: { content } })).status()).toBe(400);
    expect((await request.put(url, { headers, data: { content, expected_version: 1 } })).status()).toBe(200);
    expect((await request.put(url, { headers, data: { content: { title: 'stale' }, expected_version: 1 } })).status()).toBe(409);
    const versions = (await (await request.get(`${url}/versions`, { headers })).json()).data;
    const restore = `${url}/restore/${versions[0].id}`;
    expect((await request.post(restore, { headers, data: { expected_version: 1 } })).status()).toBe(409);
    expect((await request.post(restore, { headers, data: { expected_version: 2 } })).status()).toBe(200);
    expect((await (await request.get(url, { headers })).json()).data.version).toBe(3);
  });

  test('chunk_index is explicit and retrying an identical chunk is idempotent', async ({ request }) => {
    const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');
    const headers = authHeaders(user.token);
    const created = await request.post('/api/v1/upload/sessions', { headers, data: { file_name: 'chunks.txt', total_size: 4, mime_type: 'text/plain' } });
    expect(created.status()).toBe(201);
    const id = (await created.json()).data.id;
    const chunk = { name: 'chunk.bin', mimeType: 'application/octet-stream', buffer: Buffer.from('test') };
    const url = `/api/v1/upload/sessions/${id}/chunks`;
    expect((await request.post(url, { headers, multipart: { chunk } })).status()).toBe(400);
    const first = await request.post(`${url}?chunk_index=1`, { headers, multipart: { chunk } });
    expect(first.status()).toBe(200);
    expect((await first.json()).data.uploaded_size).toBe(4);
    const retried = await request.post(`${url}?chunk_index=1`, { headers, multipart: { chunk } });
    expect(retried.status()).toBe(200);
    expect((await retried.json()).data.uploaded_size).toBe(4);
    expect((await request.post(`${url}?chunk_index=1`, { headers, multipart: { chunk: { ...chunk, buffer: Buffer.from('edit') } } })).status()).toBe(409);
    expect((await request.post(`/api/v1/upload/sessions/${id}/complete`, { headers })).status()).toBe(201);
  });
});

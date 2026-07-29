import { test, expect } from '@playwright/test';
import { registerUser, authHeaders, createDocument, uniqueEmail, uniqueUsername } from './helpers';

test.describe('Documents API @api', () => {
  let token: string;

  test.beforeEach(async ({ request }) => {
    const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');
    token = user.token;
  });

  test('should create a writer document', async ({ request }) => {
    const { response, body } = await createDocument(request, token, 'My Document', 'writer');
    expect(response.status()).toBe(201);
    expect(body.success).toBe(true);
    expect(body.data.document_type).toBe('writer');
    expect(body.data.name).toBe('My Document');
    expect(body.data.version).toBe(1);
    expect(body.data.id).toBeTruthy();
    expect(body.data.file_id).toBeTruthy();
  });

  test('should create a calc document', async ({ request }) => {
    const { response, body } = await createDocument(request, token, 'My Spreadsheet', 'calc');
    expect(response.status()).toBe(201);
    expect(body.data.document_type).toBe('calc');
  });

  test('should create a slides document', async ({ request }) => {
    const { response, body } = await createDocument(request, token, 'My Presentation', 'slides');
    expect(response.status()).toBe(201);
    expect(body.data.document_type).toBe('slides');
  });

  test('should create a whiteboard document', async ({ request }) => {
    const { response, body } = await createDocument(request, token, 'My Whiteboard', 'whiteboard');
    expect(response.status()).toBe(201);
    expect(body.data.document_type).toBe('whiteboard');
  });

  test('should get document by id', async ({ request }) => {
    const { body: created } = await createDocument(request, token, 'Test Doc', 'writer');
    const docId = created.data.id;

    const response = await request.get(`/api/v1/documents/${docId}`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.data.id).toBe(docId);
    expect(body.data.name).toBe('Test Doc');
    expect(body.data.document_type).toBe('writer');
  });

  test('should return 404 for nonexistent document', async ({ request }) => {
    const fakeId = '00000000-0000-0000-0000-000000000000';
    const response = await request.get(`/api/v1/documents/${fakeId}`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(404);
  });

  test('should get document content', async ({ request }) => {
    const { body: created } = await createDocument(request, token, 'Content Doc', 'writer');
    const docId = created.data.id;

    const response = await request.get(`/api/v1/documents/${docId}/content`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.success).toBe(true);
    expect(body.data.content).toBeTruthy();
    expect(body.data.version).toBe(1);
  });

  test('should update document content', async ({ request }) => {
    const { body: created } = await createDocument(request, token, 'Test Doc', 'writer');
    const docId = created.data.id;

    const newContent = { blocks: [{ type: 'Paragraph', text: 'Hello World' }] };
    const response = await request.put(`/api/v1/documents/${docId}`, {
      headers: authHeaders(token),
      data: { content: newContent },
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.data.version).toBe(2);

    // Verify content was updated
    const getResp = await request.get(`/api/v1/documents/${docId}/content`, {
      headers: authHeaders(token),
    });
    const getBody = await getResp.json();
    expect(getBody.data.content).toEqual(newContent);
  });

  test('should create version on update', async ({ request }) => {
    const { body: created } = await createDocument(request, token, 'Test Doc', 'writer');
    const docId = created.data.id;

    // Update document to trigger version snapshot
    await request.put(`/api/v1/documents/${docId}`, {
      headers: authHeaders(token),
      data: { content: { text: 'Version 2' } },
    });

    // List versions
    const response = await request.get(`/api/v1/documents/${docId}/versions`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.success).toBe(true);
    expect(body.data.length).toBeGreaterThanOrEqual(1);
    expect(body.data[0].version_number).toBe(1);
  });

  test('should delete document (soft delete)', async ({ request }) => {
    const { body: created } = await createDocument(request, token, 'To Delete', 'writer');
    const docId = created.data.id;
    const fileId = created.data.file_id;

    const response = await request.delete(`/api/v1/documents/${docId}`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);

    // Verify the underlying file is trashed
    const fileResp = await request.get(`/api/v1/files/${fileId}`, {
      headers: authHeaders(token),
    });
    expect(fileResp.status()).toBe(200);
    const fileBody = await fileResp.json();
    expect(fileBody.data.is_trashed).toBe(true);
  });

  test('should export writer document as HTML', async ({ request }) => {
    const { body: created } = await createDocument(request, token, 'Export Test', 'writer');
    const docId = created.data.id;

    const response = await request.post(`/api/v1/documents/${docId}/export/html`, {
      headers: authHeaders(token),
    });
    // Should be 200 with HTML content
    expect(response.status()).toBe(200);
    const contentType = response.headers()['content-type'];
    expect(contentType).toContain('text/html');
  });

  test('should export document as JSON', async ({ request }) => {
    const { body: created } = await createDocument(request, token, 'JSON Export', 'writer');
    const docId = created.data.id;

    const response = await request.post(`/api/v1/documents/${docId}/export/json`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);
    const contentType = response.headers()['content-type'];
    expect(contentType).toContain('application/json');
  });

  test('should reject unauthenticated document creation', async ({ request }) => {
    const response = await request.post('/api/v1/documents', {
      data: { name: 'Test', document_type: 'writer', parent_id: null },
    });
    expect(response.status()).toBe(401);
  });

  test('should have default content after creation', async ({ request }) => {
    const { body } = await createDocument(request, token, 'Default Content', 'writer');
    expect(body.data.content).toBeTruthy();
    // Writer doc should have some default structure
    expect(typeof body.data.content).toBe('object');
  });
});

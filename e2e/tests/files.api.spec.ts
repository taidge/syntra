import { test, expect } from '@playwright/test';
import { registerUser, authHeaders, createDocument, createFolder, uniqueEmail, uniqueUsername } from './helpers';

test.describe('Files API @api', () => {
  let token: string;
  let userId: string;

  test.beforeEach(async ({ request }) => {
    const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');
    token = user.token;
    userId = user.userId;
  });

  test('should list files (empty for new user)', async ({ request }) => {
    const response = await request.get('/api/v1/files', {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.success).toBe(true);
    expect(body.data).toEqual([]);
  });

  test('should list files after creating documents', async ({ request }) => {
    await createDocument(request, token, 'File Doc 1', 'writer');
    await createDocument(request, token, 'File Doc 2', 'calc');

    const response = await request.get('/api/v1/files', {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.data.length).toBe(2);
  });

  test('should list files in a specific folder', async ({ request }) => {
    const { body: folder } = await createFolder(request, token, 'Filter Folder');
    const folderId = folder.data.id;

    await createDocument(request, token, 'In Folder', 'writer', folderId);
    await createDocument(request, token, 'At Root', 'writer');

    const response = await request.get(`/api/v1/files?parent_id=${folderId}`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.data.length).toBe(1);
    expect(body.data[0].name).toBe('In Folder');
  });

  test('should rename a file', async ({ request }) => {
    const { body: doc } = await createDocument(request, token, 'Old File Name', 'writer');
    const docFileId = doc.data.file_id;

    const response = await request.put(`/api/v1/files/${docFileId}`, {
      headers: authHeaders(token),
      data: { name: 'New File Name' },
    });
    expect(response.status()).toBe(200);

    // Verify rename
    const getResp = await request.get(`/api/v1/files/${docFileId}`, {
      headers: authHeaders(token),
    });
    const body = await getResp.json();
    expect(body.data.name).toBe('New File Name');
  });

  test('should delete (trash) a file', async ({ request }) => {
    const { body: doc } = await createDocument(request, token, 'Trash Me', 'writer');
    const docFileId = doc.data.file_id;

    const response = await request.delete(`/api/v1/files/${docFileId}`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);

    // Trashed file should not appear in list
    const listResp = await request.get('/api/v1/files', {
      headers: authHeaders(token),
    });
    const listBody = await listResp.json();
    const found = listBody.data.find((f: any) => f.id === docFileId);
    expect(found).toBeUndefined();
  });

  test('should move a file to another folder', async ({ request }) => {
    const { body: folder } = await createFolder(request, token, 'Target Folder');
    const folderId = folder.data.id;

    const { body: doc } = await createDocument(request, token, 'Move Me', 'writer');
    const docFileId = doc.data.file_id;

    const response = await request.post(`/api/v1/files/${docFileId}/move`, {
      headers: authHeaders(token),
      data: { target_folder_id: folderId },
    });
    expect(response.status()).toBe(200);

    // Verify the file is now in the target folder
    const getResp = await request.get(`/api/v1/files/${docFileId}`, {
      headers: authHeaders(token),
    });
    const body = await getResp.json();
    expect(body.data.parent_id).toBe(folderId);
  });

  test('should get a single file by id', async ({ request }) => {
    const { body: doc } = await createDocument(request, token, 'Single File', 'writer');
    const docFileId = doc.data.file_id;

    const response = await request.get(`/api/v1/files/${docFileId}`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.data.id).toBe(docFileId);
    expect(body.data.name).toBe('Single File');
    expect(body.data.is_folder).toBe(false);
  });

  test('should return 404 for nonexistent file', async ({ request }) => {
    const fakeId = '00000000-0000-0000-0000-000000000000';
    const response = await request.get(`/api/v1/files/${fakeId}`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(404);
  });

  test('should reject unauthenticated file listing', async ({ request }) => {
    const response = await request.get('/api/v1/files');
    expect(response.status()).toBe(401);
  });
});

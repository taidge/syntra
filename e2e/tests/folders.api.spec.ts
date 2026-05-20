import { test, expect } from '@playwright/test';
import { registerUser, authHeaders, createFolder, createDocument, uniqueEmail, uniqueUsername } from './helpers';

test.describe('Folders API @api', () => {
  let token: string;
  let userId: string;

  test.beforeEach(async ({ request }) => {
    const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');
    token = user.token;
    userId = user.userId;
  });

  test('should create a folder', async ({ request }) => {
    const { response, body } = await createFolder(request, token, 'My Folder');
    expect(response.status()).toBe(201);
    expect(body.success).toBe(true);
    expect(body.data.name).toBe('My Folder');
    expect(body.data.is_folder).toBe(true);
    expect(body.data.id).toBeTruthy();
    expect(body.data.size).toBe(0);
    expect(body.data.is_trashed).toBe(false);
  });

  test('should get folder by id', async ({ request }) => {
    const { body: created } = await createFolder(request, token, 'Fetch Folder');
    const folderId = created.data.id;

    const response = await request.get(`/api/v1/folders/${folderId}`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.data.id).toBe(folderId);
    expect(body.data.name).toBe('Fetch Folder');
    expect(body.data.is_folder).toBe(true);
  });

  test('should return 404 for nonexistent folder', async ({ request }) => {
    const fakeId = '00000000-0000-0000-0000-000000000000';
    const response = await request.get(`/api/v1/folders/${fakeId}`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(404);
  });

  test('should create nested folder', async ({ request }) => {
    const { body: parent } = await createFolder(request, token, 'Parent Folder');
    const parentId = parent.data.id;

    const { response, body } = await createFolder(request, token, 'Child Folder', parentId);
    expect(response.status()).toBe(201);
    expect(body.data.name).toBe('Child Folder');
    expect(body.data.parent_id).toBe(parentId);
  });

  test('should list folder children', async ({ request }) => {
    const { body: parent } = await createFolder(request, token, 'Parent');
    const parentId = parent.data.id;

    // Create children
    await createFolder(request, token, 'Sub Folder', parentId);
    await createDocument(request, token, 'Sub Doc', 'writer', parentId);

    const response = await request.get(`/api/v1/folders/${parentId}/children`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.success).toBe(true);
    expect(body.data.length).toBe(2);
    // Folders should come first (sorted by is_folder DESC, name ASC)
    expect(body.data[0].is_folder).toBe(true);
    expect(body.data[0].name).toBe('Sub Folder');
  });

  test('should rename a folder', async ({ request }) => {
    const { body: created } = await createFolder(request, token, 'Old Name');
    const folderId = created.data.id;

    const response = await request.put(`/api/v1/folders/${folderId}`, {
      headers: authHeaders(token),
      data: { name: 'New Name' },
    });
    expect(response.status()).toBe(200);

    // Verify rename
    const getResp = await request.get(`/api/v1/folders/${folderId}`, {
      headers: authHeaders(token),
    });
    const body = await getResp.json();
    expect(body.data.name).toBe('New Name');
  });

  test('should delete (trash) a folder', async ({ request }) => {
    const { body: created } = await createFolder(request, token, 'Delete Me');
    const folderId = created.data.id;

    const response = await request.delete(`/api/v1/folders/${folderId}`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);

    // Verify folder is not found (trashed)
    const getResp = await request.get(`/api/v1/folders/${folderId}`, {
      headers: authHeaders(token),
    });
    // Trashed folders should not appear in get (the query filters by is_trashed through the handler logic)
    // The get_folder handler does not filter by is_trashed, so it may still return it
    // but the folder should have is_trashed = true
    const body = await getResp.json();
    if (getResp.status() === 200) {
      expect(body.data.is_trashed).toBe(true);
    } else {
      expect(getResp.status()).toBe(404);
    }
  });

  test('should trash children when trashing parent folder', async ({ request }) => {
    const { body: parent } = await createFolder(request, token, 'Parent To Trash');
    const parentId = parent.data.id;

    await createFolder(request, token, 'Child To Trash', parentId);

    // Delete parent
    const response = await request.delete(`/api/v1/folders/${parentId}`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);

    // Children should be empty (all trashed)
    const childResp = await request.get(`/api/v1/folders/${parentId}/children`, {
      headers: authHeaders(token),
    });
    const childBody = await childResp.json();
    // list_folder_children filters out trashed items
    expect(childBody.data.length).toBe(0);
  });

  test('should reject unauthenticated folder creation', async ({ request }) => {
    const response = await request.post('/api/v1/folders', {
      data: { name: 'Unauth Folder', parent_id: null },
    });
    expect(response.status()).toBe(401);
  });
});

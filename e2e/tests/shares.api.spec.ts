import { test, expect } from '@playwright/test';
import { registerUser, authHeaders, createDocument, uniqueEmail, uniqueUsername } from './helpers';

test.describe('Shares API @api', () => {
  let token: string;
  let userId: string;
  let fileId: string;

  test.beforeEach(async ({ request }) => {
    const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');
    token = user.token;
    userId = user.userId;

    // Create a document to share (each doc creates a file record)
    const { body } = await createDocument(request, token, 'Shareable Doc', 'writer');
    fileId = body.data.file_id;
  });

  test('should create a user-to-user share', async ({ request }) => {
    // Register a second user to share with
    const user2 = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');

    const response = await request.post('/api/v1/shares', {
      headers: authHeaders(token),
      data: {
        file_id: fileId,
        shared_with: user2.userId,
        permission: 'read',
        expires_at: null,
      },
    });
    expect(response.status()).toBe(201);
    const body = await response.json();
    expect(body.success).toBe(true);
    expect(body.data.file_id).toBe(fileId);
    expect(body.data.shared_with).toBe(user2.userId);
    expect(body.data.permission).toBe('read');
    expect(body.data.public_token).toBeNull();
  });

  test('should create a public link share', async ({ request }) => {
    const response = await request.post('/api/v1/shares', {
      headers: authHeaders(token),
      data: {
        file_id: fileId,
        shared_with: null,
        permission: 'read',
        expires_at: null,
      },
    });
    expect(response.status()).toBe(201);
    const body = await response.json();
    expect(body.data.shared_with).toBeNull();
    expect(body.data.public_token).toBeTruthy();
    expect(typeof body.data.public_token).toBe('string');
  });

  test('should create a write permission share', async ({ request }) => {
    const user2 = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');

    const response = await request.post('/api/v1/shares', {
      headers: authHeaders(token),
      data: {
        file_id: fileId,
        shared_with: user2.userId,
        permission: 'write',
        expires_at: null,
      },
    });
    expect(response.status()).toBe(201);
    const body = await response.json();
    expect(body.data.permission).toBe('write');
  });

  test('should get share by id', async ({ request }) => {
    const user2 = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');

    const createResp = await request.post('/api/v1/shares', {
      headers: authHeaders(token),
      data: {
        file_id: fileId,
        shared_with: user2.userId,
        permission: 'read',
        expires_at: null,
      },
    });
    const created = await createResp.json();
    const shareId = created.data.id;

    const response = await request.get(`/api/v1/shares/${shareId}`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.data.id).toBe(shareId);
    expect(body.data.file_id).toBe(fileId);
  });

  test('should list shares for a file', async ({ request }) => {
    const user2 = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');
    const user3 = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');

    // Create two shares
    await request.post('/api/v1/shares', {
      headers: authHeaders(token),
      data: { file_id: fileId, shared_with: user2.userId, permission: 'read', expires_at: null },
    });
    await request.post('/api/v1/shares', {
      headers: authHeaders(token),
      data: { file_id: fileId, shared_with: user3.userId, permission: 'write', expires_at: null },
    });

    const response = await request.get(`/api/v1/shares/file/${fileId}`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.data.length).toBe(2);
  });

  test('should update share permission', async ({ request }) => {
    const user2 = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');

    const createResp = await request.post('/api/v1/shares', {
      headers: authHeaders(token),
      data: { file_id: fileId, shared_with: user2.userId, permission: 'read', expires_at: null },
    });
    const created = await createResp.json();
    const shareId = created.data.id;

    const response = await request.put(`/api/v1/shares/${shareId}`, {
      headers: authHeaders(token),
      data: { permission: 'write' },
    });
    expect(response.status()).toBe(200);

    // Verify the update
    const getResp = await request.get(`/api/v1/shares/${shareId}`, {
      headers: authHeaders(token),
    });
    const body = await getResp.json();
    expect(body.data.permission).toBe('write');
  });

  test('should delete a share', async ({ request }) => {
    const user2 = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');

    const createResp = await request.post('/api/v1/shares', {
      headers: authHeaders(token),
      data: { file_id: fileId, shared_with: user2.userId, permission: 'read', expires_at: null },
    });
    const created = await createResp.json();
    const shareId = created.data.id;

    const response = await request.delete(`/api/v1/shares/${shareId}`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);

    // Verify deleted
    const getResp = await request.get(`/api/v1/shares/${shareId}`, {
      headers: authHeaders(token),
    });
    expect(getResp.status()).toBe(404);
  });

  test('should reject share for nonexistent file', async ({ request }) => {
    const fakeFileId = '00000000-0000-0000-0000-000000000000';
    const response = await request.post('/api/v1/shares', {
      headers: authHeaders(token),
      data: {
        file_id: fakeFileId,
        shared_with: null,
        permission: 'read',
        expires_at: null,
      },
    });
    expect(response.status()).toBe(404);
  });
});

import { test, expect } from '@playwright/test';
import { registerUser, authHeaders, createDocument, uniqueEmail, uniqueUsername } from './helpers';

test.describe('Comments API @api', () => {
  let token: string;
  let userId: string;
  let username: string;
  let docId: string;

  test.beforeEach(async ({ request }) => {
    const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');
    token = user.token;
    userId = user.userId;
    username = user.username;

    const { body } = await createDocument(request, token, 'Commented Doc', 'writer');
    docId = body.data.id;
  });

  test('should create a comment on a document', async ({ request }) => {
    const response = await request.post(`/api/v1/documents/${docId}/comments`, {
      headers: authHeaders(token),
      data: {
        content: 'This is a test comment',
        parent_id: null,
        position: null,
      },
    });
    expect(response.status()).toBe(201);
    const body = await response.json();
    expect(body.success).toBe(true);
    expect(body.data.content).toBe('This is a test comment');
    expect(body.data.document_id).toBe(docId);
    expect(body.data.username).toBe(username);
    expect(body.data.is_resolved).toBe(false);
    expect(body.data.parent_id).toBeNull();
  });

  test('should create a comment with position metadata', async ({ request }) => {
    const position = { start: 10, end: 25, block_id: 'para-1' };
    const response = await request.post(`/api/v1/documents/${docId}/comments`, {
      headers: authHeaders(token),
      data: {
        content: 'Positioned comment',
        parent_id: null,
        position,
      },
    });
    expect(response.status()).toBe(201);
    const body = await response.json();
    expect(body.data.position).toEqual(position);
  });

  test('should list comments on a document', async ({ request }) => {
    // Create multiple comments
    await request.post(`/api/v1/documents/${docId}/comments`, {
      headers: authHeaders(token),
      data: { content: 'Comment 1', parent_id: null, position: null },
    });
    await request.post(`/api/v1/documents/${docId}/comments`, {
      headers: authHeaders(token),
      data: { content: 'Comment 2', parent_id: null, position: null },
    });

    const response = await request.get(`/api/v1/documents/${docId}/comments`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.success).toBe(true);
    expect(body.data.length).toBe(2);
    // Ordered by created_at ASC
    expect(body.data[0].content).toBe('Comment 1');
    expect(body.data[1].content).toBe('Comment 2');
  });

  test('should create a reply to a comment', async ({ request }) => {
    // Create parent comment
    const parentResp = await request.post(`/api/v1/documents/${docId}/comments`, {
      headers: authHeaders(token),
      data: { content: 'Parent comment', parent_id: null, position: null },
    });
    const parentBody = await parentResp.json();
    const parentCommentId = parentBody.data.id;

    // Create reply
    const response = await request.post(`/api/v1/documents/${docId}/comments`, {
      headers: authHeaders(token),
      data: {
        content: 'Reply to parent',
        parent_id: parentCommentId,
        position: null,
      },
    });
    expect(response.status()).toBe(201);
    const body = await response.json();
    expect(body.data.parent_id).toBe(parentCommentId);
    expect(body.data.content).toBe('Reply to parent');
  });

  test('should update a comment', async ({ request }) => {
    const createResp = await request.post(`/api/v1/documents/${docId}/comments`, {
      headers: authHeaders(token),
      data: { content: 'Original text', parent_id: null, position: null },
    });
    const created = await createResp.json();
    const commentId = created.data.id;

    const response = await request.put(`/api/v1/comments/${commentId}`, {
      headers: authHeaders(token),
      data: { content: 'Updated text' },
    });
    expect(response.status()).toBe(200);

    // Verify update via list
    const listResp = await request.get(`/api/v1/documents/${docId}/comments`, {
      headers: authHeaders(token),
    });
    const listBody = await listResp.json();
    const updated = listBody.data.find((c: any) => c.id === commentId);
    expect(updated.content).toBe('Updated text');
  });

  test('should resolve a comment', async ({ request }) => {
    const createResp = await request.post(`/api/v1/documents/${docId}/comments`, {
      headers: authHeaders(token),
      data: { content: 'To resolve', parent_id: null, position: null },
    });
    const created = await createResp.json();
    const commentId = created.data.id;

    const response = await request.post(`/api/v1/comments/${commentId}/resolve`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);

    // Verify resolved
    const listResp = await request.get(`/api/v1/documents/${docId}/comments`, {
      headers: authHeaders(token),
    });
    const listBody = await listResp.json();
    const resolved = listBody.data.find((c: any) => c.id === commentId);
    expect(resolved.is_resolved).toBe(true);
  });

  test('should delete a comment', async ({ request }) => {
    const createResp = await request.post(`/api/v1/documents/${docId}/comments`, {
      headers: authHeaders(token),
      data: { content: 'To delete', parent_id: null, position: null },
    });
    const created = await createResp.json();
    const commentId = created.data.id;

    const response = await request.delete(`/api/v1/comments/${commentId}`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);

    // Verify deleted
    const listResp = await request.get(`/api/v1/documents/${docId}/comments`, {
      headers: authHeaders(token),
    });
    const listBody = await listResp.json();
    const found = listBody.data.find((c: any) => c.id === commentId);
    expect(found).toBeUndefined();
  });

  test('should return 404 when deleting nonexistent comment', async ({ request }) => {
    const fakeId = '00000000-0000-0000-0000-000000000000';
    const response = await request.delete(`/api/v1/comments/${fakeId}`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(404);
  });
});

import { test, expect } from '@playwright/test';
import { registerUser, authHeaders, uniqueEmail, uniqueUsername } from './helpers';

test.describe('Notifications API @api', () => {
  let token: string;

  test.beforeEach(async ({ request }) => {
    const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');
    token = user.token;
  });

  test('should list notifications (empty for new user)', async ({ request }) => {
    const response = await request.get('/api/v1/notifications', {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.success).toBe(true);
    expect(Array.isArray(body.data)).toBe(true);
    expect(body.data.length).toBe(0);
  });

  test('should mark all notifications as read', async ({ request }) => {
    const response = await request.put('/api/v1/notifications/read-all', {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.success).toBe(true);
    expect(body.data.message).toContain('read');
  });

  test('should return 404 when marking nonexistent notification as read', async ({ request }) => {
    const fakeId = '00000000-0000-0000-0000-000000000000';
    const response = await request.put(`/api/v1/notifications/${fakeId}/read`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(404);
  });

  test('should return 404 when deleting nonexistent notification', async ({ request }) => {
    const fakeId = '00000000-0000-0000-0000-000000000000';
    const response = await request.delete(`/api/v1/notifications/${fakeId}`, {
      headers: authHeaders(token),
    });
    expect(response.status()).toBe(404);
  });

  test('should reject unauthenticated notification access', async ({ request }) => {
    const response = await request.get('/api/v1/notifications');
    expect(response.status()).toBe(401);
  });
});

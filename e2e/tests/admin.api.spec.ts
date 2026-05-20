import { test, expect } from '@playwright/test';
import { registerUser, authHeaders, createDocument, uniqueEmail, uniqueUsername } from './helpers';

test.describe('Admin API @api', () => {
  // Note: Admin endpoints require admin role. A regular "editor" user will get 403.
  // These tests verify the access control and, when possible, the response format.

  test('should reject non-admin access to stats', async ({ request }) => {
    const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');

    const response = await request.get('/api/v1/admin/stats', {
      headers: authHeaders(user.token),
    });
    expect(response.status()).toBe(403);
    const body = await response.json();
    expect(body.success).toBe(false);
  });

  test('should reject non-admin access to user list', async ({ request }) => {
    const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');

    const response = await request.get('/api/v1/admin/users', {
      headers: authHeaders(user.token),
    });
    expect(response.status()).toBe(403);
  });

  test('should reject non-admin access to activity log', async ({ request }) => {
    const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');

    const response = await request.get('/api/v1/admin/activity', {
      headers: authHeaders(user.token),
    });
    expect(response.status()).toBe(403);
  });

  test('should reject unauthenticated access to admin stats', async ({ request }) => {
    const response = await request.get('/api/v1/admin/stats');
    expect(response.status()).toBe(401);
  });

  test('should reject unauthenticated access to admin users', async ({ request }) => {
    const response = await request.get('/api/v1/admin/users');
    expect(response.status()).toBe(401);
  });
});

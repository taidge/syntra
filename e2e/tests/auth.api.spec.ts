import { test, expect } from '@playwright/test';
import { registerUser, loginUser, authHeaders, uniqueEmail, uniqueUsername } from './helpers';

test.describe('Authentication API @api', () => {
  test('should register a new user', async ({ request }) => {
    const email = uniqueEmail();
    const username = uniqueUsername();
    const response = await request.post('/api/v1/auth/register', {
      data: { username, email, password: 'Test@12345' },
    });
    expect(response.status()).toBe(201);
    const body = await response.json();
    expect(body.success).toBe(true);
    expect(body.data.token).toBeTruthy();
    expect(body.data.user.email).toBe(email);
    expect(body.data.user.username).toBe(username);
    expect(body.data.user.role).toBe('editor');
  });

  test('should reject duplicate email', async ({ request }) => {
    const email = uniqueEmail();
    await registerUser(request, uniqueUsername(), email, 'Test@12345');

    const response = await request.post('/api/v1/auth/register', {
      data: { username: uniqueUsername(), email, password: 'Test@12345' },
    });
    expect(response.status()).toBe(409);
  });

  test('should reject duplicate username', async ({ request }) => {
    const username = uniqueUsername();
    await registerUser(request, username, uniqueEmail(), 'Test@12345');

    const response = await request.post('/api/v1/auth/register', {
      data: { username, email: uniqueEmail(), password: 'Test@12345' },
    });
    expect(response.status()).toBe(409);
  });

  test('should login with valid credentials', async ({ request }) => {
    const email = uniqueEmail();
    await registerUser(request, uniqueUsername(), email, 'Test@12345');

    const response = await request.post('/api/v1/auth/login', {
      data: { email, password: 'Test@12345' },
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.success).toBe(true);
    expect(body.data.token).toBeTruthy();
  });

  test('should reject invalid password', async ({ request }) => {
    const email = uniqueEmail();
    await registerUser(request, uniqueUsername(), email, 'Test@12345');

    const response = await request.post('/api/v1/auth/login', {
      data: { email, password: 'wrongpassword' },
    });
    expect(response.status()).toBe(401);
  });

  test('should reject nonexistent email on login', async ({ request }) => {
    const response = await request.post('/api/v1/auth/login', {
      data: { email: 'nobody@nowhere.com', password: 'Test@12345' },
    });
    expect(response.status()).toBe(401);
  });

  test('should return user profile with valid token', async ({ request }) => {
    const email = uniqueEmail();
    const username = uniqueUsername();
    const user = await registerUser(request, username, email, 'Test@12345');

    const response = await request.get('/api/v1/auth/me', {
      headers: authHeaders(user.token),
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.data.username).toBe(username);
    expect(body.data.email).toBe(email);
  });

  test('should reject unauthenticated /me request', async ({ request }) => {
    const response = await request.get('/api/v1/auth/me');
    expect(response.status()).toBe(401);
  });

  test('should reject invalid JWT token', async ({ request }) => {
    const response = await request.get('/api/v1/auth/me', {
      headers: authHeaders('invalid.jwt.token'),
    });
    expect(response.status()).toBe(401);
  });

  test('should return storage quota for new user', async ({ request }) => {
    const user = await registerUser(request, uniqueUsername(), uniqueEmail(), 'Test@12345');

    const response = await request.get('/api/v1/auth/me', {
      headers: authHeaders(user.token),
    });
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.data.storage_used).toBe(0);
    expect(body.data.storage_quota).toBeGreaterThan(0);
  });
});

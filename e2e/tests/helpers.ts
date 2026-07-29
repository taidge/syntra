import { APIRequestContext, expect } from '@playwright/test';

const API_BASE = '/api/v1';

export interface AuthUser {
  token: string;
  userId: string;
  username: string;
  email: string;
}

export async function registerUser(
  request: APIRequestContext,
  username: string,
  email: string,
  password: string
): Promise<AuthUser> {
  const response = await request.post(`${API_BASE}/auth/register`, {
    data: { username, email, password },
  });
  expect(response.ok()).toBeTruthy();
  const body = await response.json();
  expect(body.success).toBe(true);
  return {
    token: body.data.token,
    userId: body.data.user.id,
    username: body.data.user.username,
    email: body.data.user.email,
  };
}

export async function loginUser(
  request: APIRequestContext,
  email: string,
  password: string
): Promise<AuthUser> {
  const response = await request.post(`${API_BASE}/auth/login`, {
    data: { email, password },
  });
  expect(response.ok()).toBeTruthy();
  const body = await response.json();
  return {
    token: body.data.token,
    userId: body.data.user.id,
    username: body.data.user.username,
    email: body.data.user.email,
  };
}

export function authHeaders(token: string) {
  return {
    Authorization: `Bearer ${token}`,
  };
}

export async function createDocument(
  request: APIRequestContext,
  token: string,
  name: string,
  docType: 'writer' | 'calc' | 'slides' | 'whiteboard',
  parentId?: string
): Promise<any> {
  const response = await request.post(`${API_BASE}/documents`, {
    headers: authHeaders(token),
    data: { name, document_type: docType, parent_id: parentId || null },
  });
  const body = await response.json();
  return { response, body };
}

export async function createFolder(
  request: APIRequestContext,
  token: string,
  name: string,
  parentId?: string
): Promise<any> {
  const response = await request.post(`${API_BASE}/folders`, {
    headers: authHeaders(token),
    data: { name, parent_id: parentId || null },
  });
  const body = await response.json();
  return { response, body };
}

// Generate unique values per test run
let counter = 0;
export function uniqueEmail(): string {
  counter++;
  return `user_${Date.now()}_${counter}@test.com`;
}

export function uniqueUsername(): string {
  counter++;
  return `user_${Date.now()}_${counter}`;
}

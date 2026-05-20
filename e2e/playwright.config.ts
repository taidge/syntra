import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  fullyParallel: false,  // Sequential for API state
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: 1,
  reporter: process.env.CI ? 'github' : 'html',
  use: {
    baseURL: process.env.API_URL || 'http://localhost:3000',
    extraHTTPHeaders: {
      'Accept': 'application/json',
    },
    trace: 'on-first-retry',
  },
  projects: [
    {
      name: 'api-tests',
      testMatch: /.*\.api\.spec\.ts/,
    },
    {
      name: 'ui-tests',
      testMatch: /.*\.ui\.spec\.ts/,
      use: {
        browserName: 'chromium',
        viewport: { width: 1280, height: 720 },
      },
    },
  ],
  // Optionally start the server before tests
  webServer: process.env.CI ? undefined : {
    command: process.platform === 'win32'
      ? 'cd .. && cargo run -p contrix-server'
      : 'cd .. && cargo run -p contrix-server',
    url: 'http://localhost:3000/api/v1/auth/me',
    reuseExistingServer: !process.env.CI,
    timeout: 180_000,
    ignoreHTTPSErrors: true,
  },
});

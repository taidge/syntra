import { defineConfig } from '@playwright/test';
import { join } from 'node:path';

const artifacts = process.env.CONTRIX_E2E_ARTIFACTS || (process.platform === 'win32'
  ? 'D:/BuildCache/office-glory/e2e'
  : join(process.env.RUNNER_TEMP || '/tmp', 'office-glory/e2e'));

export default defineConfig({
  testDir: './tests',
  outputDir: join(artifacts, 'test-results'),
  fullyParallel: false,  // Sequential for API state
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: 1,
  reporter: process.env.CI ? 'github' : [['html', { outputFolder: join(artifacts, 'report'), open: 'never' }]],
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
        channel: process.env.PLAYWRIGHT_CHANNEL || 'chromium',
        viewport: { width: 1280, height: 720 },
      },
    },
  ],
  // Optionally start the server before tests
  webServer: process.env.CI || process.env.E2E_EXTERNAL_SERVER ? undefined : {
    command: process.platform === 'win32'
      ? 'cd .. && cargo run -p contrix-server'
      : 'cd .. && cargo run -p contrix-server',
    url: 'http://localhost:3000/api/v1/auth/me',
    reuseExistingServer: !process.env.CI,
    timeout: 180_000,
    ignoreHTTPSErrors: true,
  },
});

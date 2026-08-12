# Contrix Office — Product Improvement Roadmap

> Last updated: 2026-03-19
> Status: 165 API endpoints, 40 handler files, 9 migrations, 37+ tables
> Stack: Rust (Salvo + Diesel + Dioxus), SQLite, WebSocket

---

## P0 — Critical (Must fix before any release)

### 1. Frontend API Integration
- All `api_get/post/put/delete` in `frontend/src/state.rs` return stub errors
- Login, register, file upload, folder creation — none call the backend
- Frontend is non-functional without this
- **Action:** Wire gloo-net / web-sys fetch to every API call

### 2. WebSocket Client for Real-time Collaboration
- Backend CRDT/OT + broadcast fully working
- Frontend has zero WebSocket client code
- Multiple users cannot actually co-edit
- **Action:** Implement WS client in Dioxus, render remote cursors, apply incoming ops

### 3. Collaboration Operation Persistence
- Ops only live in memory (tokio broadcast channel)
- Server restart = all concurrent edits lost
- **Action:** Persist ops to `operation_log` table, replay on reconnect via `SyncRequest`

### 4. Password Reset / Forgot Password Flow
- No endpoint, no email, no token
- Users locked out permanently if they forget password
- **Action:** Add `POST /auth/forgot-password`, `POST /auth/reset-password` with token + SMTP delivery

### 5. Enable Rate Limiting by Default
- `rate_limit.enabled = false` in default.toml
- Open to DoS in any deployment
- **Action:** Set `enabled = true`, tune defaults (100 req/min is fine)

### 6. SMTP Actually Used for Email Notifications
- SMTP settings endpoint exists but nothing sends email
- Shares, comments, mentions, invitations — all should trigger email
- **Action:** Add email sending service, hook into share/comment/invitation handlers

---

## P1 — High (Before public beta)

### 7. Office Format Import/Export (DOCX, XLSX, PPTX)
- Export crate has `ExportFormat` enum with these marked "Planned"
- Users cannot share work in universally accepted formats
- **Action:** Implement using `docx-rs`, `rust_xlsxwriter`, or custom serializers

### 8. Undo/Redo
- Not in CRDT operation log, not on frontend
- 4 TODO comments in whiteboard.rs alone
- Essential for any editor
- **Action:** Per-user undo stack in collab engine, `Ctrl+Z` / `Ctrl+Y` keybindings

### 9. CRDT Convergence Hardening
- No tombstone deletion (deleted items can resurrect)
- Overlapping inserts at same position resolved arbitrarily
- Broadcast lag (`RecvError::Lagged`) silently skips messages
- **Action:** Add tombstone markers, tie-break by user_id for determinism, request re-sync on lag

### 11. Role-Based Access Control in Rooms
- Room members have a `role` field but it's not enforced
- Any member can do anything
- **Action:** Check role (admin/editor/viewer) in file/document handlers when file belongs to a room

### 13. Collaborative Cursor Visualization
- Backend sends `CursorUpdate` messages with user color
- Frontend doesn't render remote cursors
- **Action:** Overlay colored cursors/selections in Writer/Calc/Slides editors

### 14. Frontend Logout
- Navbar has `TODO: clear auth state and navigate to login`
- **Action:** Clear JWT from storage, redirect to `/login`

---

## P2 — Medium (Post-beta polish)

### 15. Whiteboard Completeness
- Undo/Redo, Clear All, Export PNG, Export SVG — all TODO
- Pressure sensitivity not wired
- **Action:** Implement all 4 TODOs, use canvas-to-blob for PNG/SVG

### 16. Drag-and-Drop File Upload
- Frontend file_manager.rs has `TODO: trigger file upload`
- No drag-and-drop zone
- **Action:** Add drop zone component, wire to multipart upload API

### 17. Template Picker UI
- Backend supports templates (CRUD + create-from-template)
- Frontend has no template selection dialog
- **Action:** Add template gallery modal in document creation flow

### 18. Admin: Force Logout / Edit User
- Can list users but can't: reset password, change role, force logout, disable account
- **Action:** Add `PUT /admin/users/:id` (role, status), `POST /admin/users/:id/force-logout`

### 19. File Preview / Thumbnails
- `thumbnail_path` column exists on files table
- No thumbnail generation service
- **Action:** Generate thumbnails on upload (images: resize, docs: first-page render)

### 20. Notification Preferences
- All notifications on by default, no way to mute
- **Action:** Add `notification_preferences` table, per-event toggle (share, comment, mention, system)

### 21. Activity Feed on Dashboard
- Dashboard page exists but shows nothing
- **Action:** Show recent activity (your files, shared with you, comments on your docs)

### 22. Trash Auto-Cleanup
- Config has `trash.auto_delete_days = 30` and `auto_delete_enabled = false`
- No background job runs this
- **Action:** Add periodic task (tokio interval) to purge trash older than N days

### 23. Document Conversion Backend
- `conversion_jobs` table exists, status always "pending"
- No actual conversion logic
- **Action:** Use doc-engine + export crate to convert, update job status to "completed"

### 24. Monitoring Metrics Collection
- `/admin/monitoring` endpoint returns placeholder values
- No actual memory/CPU tracking
- **Action:** Read `/proc/self/status` on Linux, use `sysinfo` crate cross-platform

### 25. Keyboard Shortcuts
- No global shortcut system in frontend
- Essential: `Ctrl+S` save, `Ctrl+Z` undo, `Ctrl+B` bold, etc.
- **Action:** Add keyboard event listener in Dioxus with command dispatch

### 26. Autosave
- Writer page has `TODO: parse content, count words/chars, autosave`
- No debounced save on content change
- **Action:** 5-second debounce timer, call `PUT /documents/:id` on change

---

## P3 — Low (Nice to have)

### 27. Offline Mode / Service Worker
- No PWA manifest, no service worker, no local cache
- Single network drop = work lost
- **Action:** Add service worker with IndexedDB queue, sync on reconnect

### 28. SSO / SAML
- Enterprise customers need this
- **Action:** Implement SAML 2.0 SP or integrate with external IdP

### 29. End-to-End Encryption
- No encryption at rest, no E2E for documents
- **Action:** Client-side encryption with key management (complex, consider phased)

### 30. Mobile Responsive UI
- Dioxus renders desktop-only layout
- No responsive breakpoints
- **Action:** Add media queries, collapsible sidebar, touch-friendly toolbar

### 31. Print Support
- No print stylesheet or print dialog
- **Action:** Add `@media print` CSS, "Print" button in toolbar

### 32. Document Sharing via Link with Password
- Public links have no password option
- **Action:** Add `password_hash` to shares table, prompt on access

### 33. Batch Operations
- No multi-select for move, delete, tag, share
- **Action:** Add batch endpoints (`POST /files/batch/delete`, etc.) and multi-select UI

### 34. File Versioning for Non-Document Files
- Only documents have version history
- Uploaded files (PDFs, images) have no versioning
- **Action:** Add `file_versions` table, track on re-upload

### 35. Webhook Retry with Backoff
- Webhook delivery is fire-and-forget
- Failed deliveries not retried
- **Action:** Add retry queue with exponential backoff (1m, 5m, 30m, 2h)

### 36. Import from Google Docs / Office 365
- No import pathway for external documents
- **Action:** Parse DOCX/XLSX on upload, convert to internal format

### 37. Dark Mode
- No theme toggle, no CSS variables for dark palette
- **Action:** Add CSS custom properties, `prefers-color-scheme` media query, toggle in settings

### 38. Localization / i18n
- All strings hardcoded in English
- **Action:** Extract strings to locale files, add language selector

### 39. Plugin / Extension System
- No plugin API, no extension points
- **Action:** Define plugin manifest, lifecycle hooks, sandboxed execution

### 40. AI-Powered Features
- No AI integration (summarize, translate, grammar check)
- **Action:** Add optional LLM endpoint config, grammar/style suggestions

---

## Metrics to Track

| Metric | Current | Target (Beta) | Target (GA) |
|--------|---------|---------------|-------------|
| API Endpoints | 165 | 180+ | 200+ |
| E2E Tests | 53 | 100+ | 200+ |
| Frontend Pages Wired | 0/11 | 11/11 | 11/11 |
| Export Formats | 7 | 10 (+ DOCX/XLSX/PPTX) | 12+ |
| Concurrent Edit Safety | Partial | Full CRDT | Full CRDT + persistence |
| Uptime SLA | N/A | 99.5% | 99.9% |
| Lighthouse Score | N/A | 80+ | 90+ |

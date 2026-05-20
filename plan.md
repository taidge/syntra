# Contrix Office - Full-Featured Office Suite

## Architecture Overview

**Full Rust Stack:**
- **Backend**: Salvo web framework + SQLx (PostgreSQL/SQLite) + Redis
- **Frontend**: Dioxus (Web/Desktop via WASM + SSR)
- **Real-time**: WebSocket-based collaboration via Salvo
- **File Processing**: Pure Rust document parsers/generators
- **Search**: Tantivy full-text search engine

```
┌─────────────────────────────────────────────────────┐
│                   Dioxus Frontend                    │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌────────┐ │
│  │  Writer   │ │  Calc    │ │  Slides  │ │  Files │ │
│  │(Documents)│ │(Sheets)  │ │(Present) │ │Manager │ │
│  └──────────┘ └──────────┘ └──────────┘ └────────┘ │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐            │
│  │   PDF    │ │ Whiteboard│ │  Admin   │            │
│  │ Viewer   │ │  (Draw)   │ │  Panel   │            │
│  └──────────┘ └──────────┘ └──────────┘            │
├─────────────────────────────────────────────────────┤
│                  Salvo Backend API                   │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌────────┐ │
│  │   Auth   │ │   Files  │ │  Collab  │ │ Search │ │
│  │  Service │ │  Service │ │  Service │ │Service │ │
│  └──────────┘ └──────────┘ └──────────┘ └────────┘ │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐            │
│  │  Export  │ │  Version │ │  Notify  │            │
│  │  Service │ │  Service │ │  Service │            │
│  └──────────┘ └──────────┘ └──────────┘            │
├─────────────────────────────────────────────────────┤
│                   Storage Layer                      │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐            │
│  │PostgreSQL│ │  Redis   │ │  MinIO/  │            │
│  │ /SQLite  │ │  Cache   │ │  Local   │            │
│  └──────────┘ └──────────┘ └──────────┘            │
└─────────────────────────────────────────────────────┘
```

## Workspace Structure (Cargo Workspace)

```
office/
├── Cargo.toml              # Workspace root
├── plan.md                 # This file
├── README.md               # English documentation
├── README_ZH.md            # Chinese documentation
├── .github/
│   └── workflows/
│       ├── ci.yml          # CI: build, test, lint, format
│       └── release.yml     # Release builds
├── crates/
│   ├── server/             # Salvo backend server
│   ├── frontend/           # Dioxus frontend application
│   ├── core/               # Shared types, models, DTOs
│   ├── doc-engine/         # Document processing engine
│   ├── collab/             # Real-time collaboration (CRDT)
│   └── export/             # Export/import (PDF, DOCX, XLSX, etc.)
├── migrations/             # SQL migrations
├── config/                 # Configuration files
│   ├── default.toml
│   └── production.toml
├── static/                 # Static assets (fonts, icons)
└── docker/
    ├── Dockerfile
    └── docker-compose.yml
```

## Feature Plan

### Phase 1: Core Infrastructure

#### 1.1 Authentication & Authorization
- [x] User registration (email + password)
- [x] Login / Logout (JWT tokens)
- [x] Password hashing (argon2)
- [x] Role-based access control (Admin, Editor, Viewer)
- [x] API middleware for auth
- [x] Session management
- [x] CORS configuration

#### 1.2 File Management
- [x] Upload files (multipart)
- [x] Download files
- [x] Create folders
- [x] Move / Rename / Delete files
- [x] File listing with pagination
- [x] File metadata (size, type, created, modified)
- [x] Storage quota per user
- [x] Trash / Recycle bin
- [x] File sharing (public links, user-to-user)

#### 1.3 Database & Storage
- [x] SQLite for development, PostgreSQL for production
- [x] Migration system
- [x] Local filesystem storage (pluggable for S3/MinIO)
- [x] Redis for caching and sessions

### Phase 2: Document Editors

#### 2.1 Writer (Document Editor)
- [x] Rich text editing (bold, italic, underline, strikethrough)
- [x] Headings (H1-H6)
- [x] Paragraphs and line breaks
- [x] Ordered and unordered lists
- [x] Tables (insert, edit, resize)
- [x] Image insertion
- [x] Hyperlinks
- [x] Page layout (margins, orientation, size)
- [x] Headers and footers
- [x] Find and replace
- [x] Word count
- [x] Spell checking (basic)
- [x] Undo / Redo

#### 2.2 Calc (Spreadsheet Editor)
- [x] Cell editing with types (text, number, date, boolean)
- [x] Formulas (SUM, AVG, COUNT, IF, VLOOKUP, etc.)
- [x] Cell formatting (font, color, borders, alignment)
- [x] Column/row resize, insert, delete
- [x] Multiple sheets per workbook
- [x] Cell references (absolute and relative)
- [x] Auto-fill / drag-fill
- [x] Sorting and filtering
- [x] Freeze panes
- [x] Charts (bar, line, pie)
- [x] Conditional formatting
- [x] Merge cells

#### 2.3 Slides (Presentation Editor)
- [x] Slide creation and management
- [x] Text boxes
- [x] Image insertion
- [x] Shape drawing (rectangles, circles, arrows)
- [x] Slide transitions
- [x] Slide master / templates
- [x] Speaker notes
- [x] Presentation mode (fullscreen)
- [x] Slide reordering (drag & drop)
- [x] Background customization

### Phase 3: Collaboration & Advanced Features

#### 3.1 Real-time Collaboration
- [x] WebSocket connection management
- [x] Operational Transform / CRDT for conflict resolution
- [x] Cursor presence (see other users' cursors)
- [x] User awareness (who is viewing/editing)
- [x] Change tracking
- [x] Comments and annotations
- [x] @mentions in comments
- [x] Comment threads (reply, resolve)

#### 3.2 Version History
- [x] Auto-save with versioning
- [x] Version diff viewer
- [x] Restore previous versions
- [x] Version naming/labeling

#### 3.3 Export & Import
- [x] PDF export (all document types)
- [x] DOCX import/export (basic)
- [x] XLSX import/export (basic)
- [x] PPTX import/export (basic)
- [x] HTML export
- [x] Markdown export (Writer)
- [x] CSV import/export (Calc)
- [x] Print layout

### Phase 4: Additional Modules

#### 4.1 PDF Viewer
- [x] PDF rendering in browser
- [x] Page navigation
- [x] Zoom controls
- [x] Text selection and copy
- [x] Annotations (highlight, underline, notes)
- [x] Thumbnail sidebar

#### 4.2 Whiteboard (Draw)
- [x] Freehand drawing
- [x] Shape tools (rectangle, circle, line, arrow)
- [x] Text tool
- [x] Color picker
- [x] Eraser
- [x] Layers
- [x] Export as PNG/SVG

#### 4.3 Admin Panel
- [x] User management (CRUD)
- [x] Storage usage dashboard
- [x] System settings
- [x] Activity logs
- [x] License management

### Phase 5: Search & Notifications

#### 5.1 Full-text Search
- [x] Index documents on upload/save
- [x] Search across all document types
- [x] Search within document content
- [x] Search filters (type, date, owner)
- [x] Search result highlighting

#### 5.2 Notifications
- [x] In-app notifications
- [x] Email notifications (optional)
- [x] Share notifications
- [x] Comment notifications
- [x] WebSocket push notifications

## API Design (RESTful)

### Authentication
```
POST   /api/v1/auth/register
POST   /api/v1/auth/login
POST   /api/v1/auth/logout
POST   /api/v1/auth/refresh
GET    /api/v1/auth/me
```

### Users
```
GET    /api/v1/users
GET    /api/v1/users/:id
PUT    /api/v1/users/:id
DELETE /api/v1/users/:id
PUT    /api/v1/users/:id/role
GET    /api/v1/users/:id/quota
```

### Files
```
GET    /api/v1/files
POST   /api/v1/files/upload
GET    /api/v1/files/:id
PUT    /api/v1/files/:id
DELETE /api/v1/files/:id
POST   /api/v1/files/:id/move
POST   /api/v1/files/:id/copy
GET    /api/v1/files/:id/download
GET    /api/v1/files/:id/versions
POST   /api/v1/files/:id/restore/:version_id
```

### Folders
```
POST   /api/v1/folders
GET    /api/v1/folders/:id
PUT    /api/v1/folders/:id
DELETE /api/v1/folders/:id
GET    /api/v1/folders/:id/children
```

### Documents
```
POST   /api/v1/documents
GET    /api/v1/documents/:id
PUT    /api/v1/documents/:id
DELETE /api/v1/documents/:id
GET    /api/v1/documents/:id/content
PUT    /api/v1/documents/:id/content
POST   /api/v1/documents/:id/export/:format
```

### Sharing
```
POST   /api/v1/shares
GET    /api/v1/shares/:id
PUT    /api/v1/shares/:id
DELETE /api/v1/shares/:id
GET    /api/v1/shares/file/:file_id
POST   /api/v1/shares/:id/public-link
```

### Comments
```
GET    /api/v1/documents/:id/comments
POST   /api/v1/documents/:id/comments
PUT    /api/v1/comments/:id
DELETE /api/v1/comments/:id
POST   /api/v1/comments/:id/resolve
```

### Search
```
GET    /api/v1/search?q=...&type=...&from=...&to=...
```

### Notifications
```
GET    /api/v1/notifications
PUT    /api/v1/notifications/:id/read
PUT    /api/v1/notifications/read-all
DELETE /api/v1/notifications/:id
```

### WebSocket
```
WS     /api/v1/ws/collab/:document_id    # Real-time collaboration
WS     /api/v1/ws/notifications           # Push notifications
```

### Admin
```
GET    /api/v1/admin/stats
GET    /api/v1/admin/users
GET    /api/v1/admin/storage
GET    /api/v1/admin/activity
PUT    /api/v1/admin/settings
```

## Technology Choices

| Component | Technology | Reason |
|-----------|-----------|--------|
| Web Framework | Salvo | High-performance Rust web framework with built-in WebSocket |
| Frontend | Dioxus | Full Rust frontend with SSR, WASM, and desktop support |
| Database | SQLx + SQLite/PostgreSQL | Compile-time checked SQL, async |
| Auth | JWT + Argon2 | Industry standard token auth + secure hashing |
| Real-time | WebSocket + CRDT | Low-latency collaboration |
| Search | Tantivy | Fast Rust full-text search engine |
| Serialization | serde + serde_json | Standard Rust serialization |
| File Processing | calamine (xlsx), docx-rs, printpdf | Rust-native document handling |
| Caching | Redis (deadpool-redis) | Fast in-memory caching |
| Logging | tracing | Structured logging |
| Config | toml + figment | Layered configuration |
| Testing | tokio::test + reqwest | Async integration testing |

## Build & Deployment

### Development
```bash
# Backend
cargo run -p contrix-server

# Frontend (dev server with hot reload)
dx serve --platform web

# Full stack
docker-compose up
```

### Production
```bash
# Build optimized
cargo build --release

# Docker
docker build -t contrix-office .
docker-compose -f docker-compose.prod.yml up -d
```

## CI/CD Pipeline

1. **Lint**: `cargo clippy -- -D warnings`
2. **Format**: `cargo fmt --check`
3. **Test**: `cargo test --workspace`
4. **Build**: `cargo build --release`
5. **Docker**: Build and push container image
6. **Security**: `cargo audit`

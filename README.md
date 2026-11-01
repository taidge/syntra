# Contrix Office

[English](README.md) | [中文](README_ZH.md)

A full-featured, open-source office suite built entirely in Rust. Contrix Office provides document editing, spreadsheets, presentations, and whiteboard collaboration — all powered by a high-performance Rust stack.

## Features

### Document Editors

- **Writer** — Rich text document editor (similar to Word/Google Docs)
  - Bold, italic, underline, strikethrough, superscript/subscript
  - Headings (H1–H6), paragraphs, block quotes
  - Ordered and unordered lists with nesting
  - Tables with merge cells, column/row spans
  - Image insertion, hyperlinks
  - Code blocks with syntax highlighting
  - Page layout (margins, orientation, paper size)
  - Word count, find & replace

- **Calc** — Spreadsheet editor (similar to Excel/Google Sheets)
  - Cell editing with types: text, numbers, dates, booleans
  - Formula support (SUM, AVG, COUNT, IF, VLOOKUP, etc.)
  - Cell formatting: fonts, colors, borders, alignment, number formats
  - Multiple sheets per workbook
  - A1 notation cell references
  - Column/row resize, insert, delete
  - Freeze panes, merged cells
  - Conditional formatting
  - Sorting and filtering
  - Charts (bar, line, pie, doughnut, scatter, area)

- **Slides** — Presentation editor (similar to PowerPoint/Google Slides)
  - Slide creation with multiple layouts (title, two-column, blank, etc.)
  - Text boxes, images, shapes (12+ types)
  - Charts and data visualization
  - Slide transitions (fade, slide, zoom, flip, etc.)
  - Speaker notes
  - Presentation mode
  - Slide master / templates
  - Drag-and-drop reordering

- **Whiteboard** — Freeform drawing canvas
  - Freehand pen with pressure sensitivity
  - Shape tools (rectangle, circle, line, arrow)
  - Text tool
  - Eraser
  - Multiple layers with opacity control
  - Export to PNG/SVG

### Collaboration

- Real-time collaborative editing via WebSocket
- CRDT-based conflict resolution (Operational Transform)
- Live cursor presence — see other users editing
- User awareness (who's viewing/editing)
- Comments and annotations with @mentions
- Threaded comment replies, resolve/unresolve

### File Management

- Upload, download, move, rename, delete files
- Folder hierarchy with nested navigation
- Trash / recycle bin with restore
- File sharing (user-to-user and public links)
- Permission levels: Read, Write, Admin
- Storage quotas per user

### Version History

- Auto-save with version snapshots
- Browse and compare versions
- Restore any previous version
- Optional version labeling

### Export & Import

| Format   | Writer | Calc | Slides | Whiteboard |
|----------|--------|------|--------|------------|
| PDF      | Export | Export | Export | Export    |
| HTML     | Export | —    | —      | —          |
| Markdown | Export | —    | —      | —          |
| CSV      | —      | Both | —      | —          |
| JSON     | Both   | Both | Both   | Both       |
| DOCX     | Planned | —  | —      | —          |
| XLSX     | —      | Planned | —   | —          |
| PPTX     | —      | —    | Planned | —         |
| PNG      | —      | —    | —      | Export     |
| SVG      | —      | —    | —      | Export     |

### Search

- Search document names and stored content
- Filter by document type
- Content-aware search result snippets
- Paginated results

### Notifications

- In-app notifications (share, comment, mention, system)
- WebSocket push notifications
- Mark as read / mark all read

### Admin Panel

- User management (create, edit roles, delete)
- System-wide storage usage dashboard
- Activity audit logs
- System settings configuration

## Tech Stack

| Layer | Technology | Purpose |
|-------|-----------|---------|
| Backend | [Salvo](https://salvo.rs) | High-performance Rust web framework |
| Frontend | [Glory](https://glory.rs) + [Sapid](https://github.com/sapid-ui/sapid) | Semantic Rust widgets and WebAssembly browser host |
| Database | [SQLx](https://github.com/launchbadge/sqlx) + SQLite/PostgreSQL | Async database with compile-time safety |
| Auth | JWT + Argon2 | Stateless authentication + secure hashing |
| Real-time | WebSocket + CRDT | Low-latency collaboration |
| Search | SQLite | Transactionally consistent name/content search |
| Config | [Figment](https://github.com/SergioBenitez/Figment) | Layered configuration (TOML + env) |
| Logging | [tracing](https://github.com/tokio-rs/tracing) | Structured, async-aware logging |

## Project Structure

```
office/
├── Cargo.toml                  # Workspace root
├── crates/
│   ├── core/                   # Shared types, models, DTOs, config
│   ├── server/                 # Salvo backend (REST API + WebSocket)
│   ├── frontend/               # Glory + Sapid web frontend
│   ├── doc-engine/             # Document models (Writer, Calc, Slides, Whiteboard)
│   ├── collab/                 # Real-time collaboration engine (CRDT/OT)
│   └── export/                 # Export/import (HTML, Markdown, CSV, etc.)
├── migrations/                 # SQL database migrations
├── config/                     # Configuration files (default.toml, production.toml)
├── static/                     # Static assets
├── docker/                     # Dockerfile + docker-compose.yml
└── .github/workflows/          # CI/CD pipelines
```

## Getting Started

### Prerequisites

- [Rust](https://rustup.rs/) 1.95+ (edition 2024)
- SQLite 3 (bundled with Diesel)
- [Git](https://git-scm.com/) to initialize the pinned [Glory](https://github.com/glory-rs/glory) and [Sapid](https://github.com/sapid-ui/sapid) source submodules
- `wasm32-unknown-unknown` and wasm-bindgen-cli matching the version in `Cargo.lock`

### Quick Start

```powershell
# Clone the repository
git clone --recurse-submodules https://github.com/contrix-org/office.git
cd office

# Build the Glory + Sapid browser app, then start the backend
rustup target add wasm32-unknown-unknown
pwsh ./scripts/dev.ps1
```

Open `http://localhost:3000`. The server serves the API and browser app from the same origin.
Compiler output, temporary files and generated browser assets stay under `D:/BuildCache/office-glory`.
To build separately, run `pwsh ./scripts/build-frontend.ps1`, then
`pwsh ./scripts/dev.ps1 -SkipFrontendBuild` from the repository root.
The scripts set Cargo's registry cache, target directory and compiler temporaries on D:,
then verify the effective target directory with `cargo metadata --no-deps --format-version 1`.
Pass the same `-BuildRoot D:/BuildCache/<task>` to both scripts to use a separate build directory.
Under WSL, the default is `/mnt/d/BuildCache/office-glory`; the scripts verify that `/mnt/d`
is mounted from Windows D: before building. Remote GitHub Actions runners use their runner
temporary directory on Linux/macOS and D: on Windows.

### Source dependencies

Glory and Sapid live in `.dependencies/glory` and `.dependencies/sapid`. Cargo uses these
relative paths, so no separate checkout or manifest editing is needed. The repository records
an exact commit for each submodule; `branch = main` identifies the upstream branch used for
deliberate upgrades. The build and development scripts automatically initialize missing sources.

For an existing clone, or after pulling a commit that updates dependencies, run:

```powershell
git submodule update --init --recursive
```

To deliberately upgrade both dependencies to their latest upstream `main` revisions:

```powershell
git submodule update --init --recursive --remote -- .dependencies/glory .dependencies/sapid
git add .dependencies/glory .dependencies/sapid
# Validate the upgrade, then commit the updated submodule revisions and Cargo.lock if changed.
```

CI initializes the recorded submodule revisions without `--remote`, so a build remains
reproducible as upstream `main` advances. Local edits in other Glory/Sapid checkouts are unaffected.

### Configuration

Configuration is loaded from layered sources:

1. `config/default.toml` — Base defaults
2. `config/production.toml` — Production overrides
3. Environment variables — Highest priority (prefixed with `CONTRIX_`)

Key settings:

```toml
[server]
host = "0.0.0.0"
port = 3000

[database]
url = "sqlite://data/contrix.db?mode=rwc"

[auth]
jwt_secret = "change-me-in-production"
jwt_expiry_hours = 24

[storage]
base_path = "data/files"
max_upload_size_mb = 100
default_quota_mb = 5120        # 5 GB per user
```

### Docker

```bash
# Build and run with Docker Compose
cd docker
docker-compose up -d

# Or build the image directly
docker build -t contrix-office -f docker/Dockerfile .
docker run -p 3000:3000 -v contrix-data:/app/data contrix-office
```

## API Reference

### Authentication

| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/api/v1/auth/register` | Register a new user |
| POST | `/api/v1/auth/login` | Login and receive JWT |
| GET | `/api/v1/auth/me` | Get current user profile |

### Files

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/v1/files` | List files (with pagination) |
| POST | `/api/v1/files/upload` | Upload a file (multipart) |
| GET | `/api/v1/files/:id` | Get file metadata |
| PUT | `/api/v1/files/:id` | Rename a file |
| DELETE | `/api/v1/files/:id` | Move file to trash |
| POST | `/api/v1/files/:id/move` | Move file to another folder |
| GET | `/api/v1/files/:id/download` | Download file content |

### Folders

| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/api/v1/folders` | Create a folder |
| GET | `/api/v1/folders/:id` | Get folder metadata |
| GET | `/api/v1/folders/:id/children` | List folder contents |
| PUT | `/api/v1/folders/:id` | Rename a folder |
| DELETE | `/api/v1/folders/:id` | Delete a folder |

### Documents

| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/api/v1/documents` | Create a document |
| GET | `/api/v1/documents/:id` | Get document with content |
| PUT | `/api/v1/documents/:id` | Update document content |
| DELETE | `/api/v1/documents/:id` | Delete a document |
| GET | `/api/v1/documents/:id/content` | Get document content only |
| POST | `/api/v1/documents/:id/export/:format` | Export document |
| GET | `/api/v1/documents/:id/versions` | List version history |
| POST | `/api/v1/documents/:id/restore/:version_id` | Restore a version |

### Sharing

| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/api/v1/shares` | Create a share |
| GET | `/api/v1/shares/:id` | Get share details |
| PUT | `/api/v1/shares/:id` | Update share permissions |
| DELETE | `/api/v1/shares/:id` | Remove a share |
| GET | `/api/v1/shares/file/:file_id` | List shares for a file |

### Comments

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/v1/documents/:id/comments` | List comments |
| POST | `/api/v1/documents/:id/comments` | Add a comment |
| PUT | `/api/v1/comments/:id` | Edit a comment |
| DELETE | `/api/v1/comments/:id` | Delete a comment |
| POST | `/api/v1/comments/:id/resolve` | Resolve a comment |

### Search

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/v1/search?q=...` | Search documents |

### Notifications

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/v1/notifications` | List notifications |
| PUT | `/api/v1/notifications/:id/read` | Mark as read |
| PUT | `/api/v1/notifications/read-all` | Mark all as read |
| DELETE | `/api/v1/notifications/:id` | Delete notification |

### Admin

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/v1/admin/stats` | System statistics |
| GET | `/api/v1/admin/users` | List all users |
| GET | `/api/v1/admin/activity` | Activity audit log |

### WebSocket

| Endpoint | Description |
|----------|-------------|
| `WS /api/v1/ws/collab/:document_id` | Real-time document collaboration |

## Development

### Build

```bash
# Check all crates compile
cargo check --workspace

# Build all crates
cargo build --workspace

# Build release
cargo build --release -p contrix-server
```

### Test

```bash
# Run all tests
cargo test --workspace

# Run tests for a specific crate
cargo test -p contrix-doc-engine
cargo test -p contrix-collab
cargo test -p contrix-export
```

### Lint & Format

```bash
# Format code
cargo fmt

# Run clippy
cargo clippy --workspace --all-targets -- -D warnings
```

### Database Migrations

Migrations run automatically on server startup. Migration files are in `migrations/`:

- `001_init.sql` — Initial schema (users, files, documents, versions, shares, comments, notifications, activity_logs)

## Architecture

### Backend (Salvo)

The backend follows a layered architecture:

1. **Handlers** — HTTP request handlers (extract params, call services, return responses)
2. **Services** — Business logic (storage, search, etc.)
3. **Middleware** — Cross-cutting concerns (JWT auth, logging, CORS)
4. **State** — Shared application state (database pool, config, collab manager)

### Frontend (Glory + Sapid)

The frontend uses handwritten Glory widgets, Cage/Bond state and Sapid controls. Native links
preserve document URLs and browser history. Each page owns its tasks and listeners.

1. **Pages** — Route-level components (Home, Login, Dashboard, Writer, Calc, Slides, etc.)
2. **Components** — Reusable UI components (Navbar, Sidebar, Toolbar, FileList, Modal)
3. **State** — Application state management and API client

This branch uses the current local semantic APIs. Calc exposes selection modes,
range inputs and Fill actions for gestures the host does not expose. Writer's
rich editor supports bold, italic, code and links; other native formatting is
preserved, with separate controls for tables, lists, images and code blocks.
Complex slide objects can also be adjusted through the presentation JSON panel.

### Collaboration Engine

Real-time collaboration uses:

1. **WebSocket** — Low-latency bidirectional communication
2. **Operational Transform (OT)** — Conflict resolution for concurrent edits
3. **Session Manager** — Manages active editing sessions and participant awareness
4. **Broadcast Channels** — Efficient message fan-out to all participants

### Document Engine

Each document type has a rich structured model:

- **WriterDocument** — Block-based (paragraphs, headings, lists, tables, images, code blocks)
- **Workbook** — Sheet-based with sparse cell storage (HashMap), formulas, formatting
- **Presentation** — Slide-based with elements (text boxes, shapes, images, charts)
- **Whiteboard** — Layer-based with drawing elements (paths, shapes, text, images)

## Contributing

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

Please ensure:
- Code passes `cargo fmt --check` and `cargo clippy`
- New features have tests
- Documentation is updated

## License

This project is dual-licensed under:

- [MIT License](LICENSE-MIT)
- [Apache License 2.0](LICENSE-APACHE)

You may choose either license at your option.

## Acknowledgments

- [Salvo](https://salvo.rs) — Fast and flexible Rust web framework
- [Glory](https://glory.rs) + [Sapid](https://github.com/sapid-ui/sapid) — Cross-platform UI framework for Rust
- [SQLx](https://github.com/launchbadge/sqlx) — Async SQL toolkit for Rust
- Inspired by [Collabora Online](https://www.collaboraoffice.com/) and [ONLYOFFICE](https://www.onlyoffice.com/)

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
- Durable versioned snapshots and operational transformation, with explicit conflict recovery that preserves local edits
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
| DOCX     | Both | — | — | — |
| XLSX     | — | Both | — | — |
| PPTX     | — | — | Element import/export (subset) | — |
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
| Database | Diesel + Horme + SQLite | Short asynchronous query leases and transactions |
| Auth | JWT + Argon2 | Stateless authentication + secure hashing |
| Real-time | WebSocket + durable versions and conditional commits | Online synchronization and explicit conflict recovery |
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

- [Rust](https://rustup.rs/) 1.96+ (edition 2024)
- SQLite 3 (bundled with Diesel)
- [Git](https://git-scm.com/) to initialize the pinned [Glory](https://github.com/glory-rs/glory) and [Sapid](https://github.com/sapid-ui/sapid) source submodules
- `wasm32-unknown-unknown` and wasm-bindgen-cli matching the version in `Cargo.lock`

### Quick Start

```powershell
# Clone the repository
git clone --recurse-submodules https://github.com/arkret-org/office.git
cd office

# Build the Glory + Sapid browser app, then start the backend
rustup target add wasm32-unknown-unknown
pwsh ./scripts/dev.ps1
```

Open `http://localhost:3000`. The server serves the API and browser app from the same origin.
Builds respect the caller's environment and normal Cargo configuration. Scripts do not redirect CARGO_HOME, target, or TEMP/TMP.
They discover artifacts with cargo metadata in this workspace; generated browser assets default to its target_directory/web.
Build separately with pwsh ./scripts/build-frontend.ps1, then pwsh ./scripts/dev.ps1 -SkipFrontendBuild.
-OutputDirectory selects browser asset output without changing compiler caches or temporary directories.
Initialize the pinned submodules and install wasm-bindgen-cli matching Cargo.lock before running the scripts.

Office compatibility tests distinguish native custom-part round trips from independent external files.
External PPTX imports actual slide order and page dimensions, inherited placeholder bounds and master/layout decorations, styled text runs and safe hyperlinks, positioned shapes/groups, embedded images with crop/rotation, supported chart caches, notes, transitions and basic animation timing. Representable content is exported through standard Office parts. Embedded video media and geometry are retained for export; the editing canvas shows a placeholder and does not play the full PowerPoint animation timeline. Complex paragraph layout, custom paths, mirroring, SmartArt/OLE and unsupported visual effects may be approximated or omitted, with specific import warnings and the uploaded original available for download.

External XLSX imports cell styles/theme colors, date formats, row/column dimensions, frozen panes, rectangular named ranges, supported validation/conditional-format rules, cached charts and simple pivots. Chart bindings refresh from same-sheet rectangular ranges; other bindings retain cached series with warnings, and the Calc grid has no chart overlay. Pivots support one row field and one value field with sum/count/average and write standard cache/table parts; complex or filtered pivots retain worksheet values and an import warning. Standard pivot export requires a source header row and a target heading row before their data. Custom validation formulas are retained for Office export but are not enforced by the editor. Unsupported table/filter/visibility/print/rich-text features produce warnings when encountered.

External media is never fetched automatically. Embedded media is limited to 2 MiB per object and 8 MiB per imported presentation before base64 encoding; document JSON is limited to 20 MiB. ZIP/XML expansion, chart-point expansion and numeric geometry also have explicit budgets. The original upload retains content outside this supported subset.
Document saves and version restores require expected_version; conflicts return 409. Chunk uploads require an explicit chunk_index starting at 1.
Collaboration does not provide offline CRDT merging.

Run pwsh ./scripts/package.ps1 to generate a complete ZIP containing the server, configuration samples, migrations and browser assets under metadata's target_directory/dist.
Run pwsh ./scripts/test-package.ps1 -BrowserTests to verify the ZIP from a source-free directory, including login, saving, and recovery after a process restart.
Run pwsh ./scripts/test-e2e.ps1 for the full HTTP and browser suite after installing the e2e dependencies and Playwright Chromium. Each run uses its own database, storage and loopback port beneath the workspace target directory.


### Storage maintenance

`[maintenance]` in `config/default.toml` controls history retention and object reconciliation. Ordinary history converges in batches to the newest 100 versions within 30 days by default; original uploads, current snapshots and bounded active collaboration bases are protected. Physical deletion is asynchronous. When the database's deferred deletion queue reaches 1024 objects, new object writes are rejected until cleanup makes progress; this counts objects, not disk bytes.

New writes register ownership before object I/O. Unknown legacy objects are preserved by default. Enable `reconcile_legacy_dedicated_namespace` only when one metadata database exclusively owns the entire S3 bucket or local storage root. Independent databases must use different physical keys; running divergent copies of the same database against shared original keys is unsupported. Uploads use the configured idle/total body deadlines; chunk sessions reserve quota, permit at most 4096 chunks of up to 8 MiB, and return their limits when created.

### Database snapshots and blob backups

The admin backup endpoints create SQLite database snapshots only. They do not copy local storage or S3 objects, or preserve blobs referenced by older snapshots. Restoring a SQLite snapshot alone cannot guarantee file recovery: garbage collection can already have deleted objects that the live database no longer references.

Coordinate database and blob backups and restore them together, preserving the object keys referenced by the database snapshot. This requires an external consistent backup or snapshot procedure that accounts for concurrent writes and garbage collection. The package restart tests reuse the same live database and storage; they do not validate full disaster recovery from database and blob backups.

### Source dependencies

Glory and Sapid live in `.dependencies/glory` and `.dependencies/sapid`. Cargo uses these
relative paths, so no separate checkout or manifest editing is needed. The repository records
an exact commit for each submodule; `branch = main` identifies the upstream branch used for
deliberate upgrades. Initialize missing sources with git submodule update --init --recursive before building.

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

Database queries use Diesel, Diesel-async's SQLite adapter, and Horme. Request handlers
keep cheap provider handles; each SQL operation leases a connection only while it runs.
Explicit transactions share one lease. SQLite work runs on blocking workers, and cancelled
or timed-out work retains its bounded admission until cleanup finishes.

The `[database]` settings default to `max_connections = 5`, `checkout_timeout_ms = 3000`,
`operation_timeout_ms = 15000`, and `busy_timeout_ms = 3000`. Admission is bounded to twice
the connection count, including waiters and cleanup. SQLite connections use WAL.
The admin monitoring JSON and Prometheus endpoint expose connection, idle, admission,
and cumulative checkout metrics.

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

Before building, verify the Docker builder stores its data on D: or remotely; Cargo artifact placement does not establish Docker cache placement.

```bash
# Build and run with Docker Compose
cd docker
export JWT_SECRET="$(openssl rand -hex 32)"
docker compose up -d

# Or build the image directly from the repository root
cd ..
docker build -t contrix-office -f docker/Dockerfile .
docker run -p 3000:8080 -e JWT_SECRET -v contrix-data:/data contrix-office
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
cargo clippy --locked --workspace --all-targets --no-deps --features contrix-frontend/ssr -- -D warnings
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
- [Diesel](https://diesel.rs/) — Typed SQL for Rust; Horme provides asynchronous query leases
- Inspired by [Collabora Online](https://www.collaboraoffice.com/) and [ONLYOFFICE](https://www.onlyoffice.com/)

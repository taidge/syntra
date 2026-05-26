# Contrix Office — Gap Analysis vs Collabora Online (Round 5)

> Generated: 2026-05-21
> Reference: Collabora Online (`E:\Works\contrix-dev\convert\other\online`)
> Scope: User-visible behaviour. We are NOT trying to port LibreOffice. We are trying to stop the whack-a-mole on basic editor UX.
> Cumulative completed across Rounds 1-4: 56 tasks.

---

## Headline number

Measured by user-visible editor behaviours (not by API endpoint count), Contrix Office is at roughly **12–15% feature parity** with Collabora Online. Backend plumbing is much further along (room CRUD, shares, WOPI handler shells, CRDT engine, importers) — call that **35–45%**. The crippling gap is the frontend: toolbar buttons are visual stubs, Writer/Slides/Whiteboard have no real document model wired to the UI, and Calc is the only editor where typing actually mutates state.

The user's "操作上依然有很多的问题" maps directly to: **the toolbar promises capabilities the editor cannot deliver, the editor pages render but don't actually edit a persistent document model, and there is no save round-trip** outside the Calc page's local HashMap.

---

## Part A — Honest completion table

Severity legend: **Blocker** = basic usability broken. **High** = visible polish absent. **Medium** = advanced power-user. **Low** = nice-to-have.

### Document editing depth — Writer

| Area | Collabora has | Contrix has | Completion % | Severity |
|------|--------------|-------------|--------------|----------|
| Contenteditable surface | Tile-rendered LibreOffice document | Single `<div contenteditable>` with raw HTML | 25% | Blocker |
| Bold/italic/underline toolbar buttons fire commands | Yes, via `.uno:Bold` etc. | Buttons exist, **no onclick** | 0% | Blocker |
| Font family / font size picker applied to selection | Yes | `<select>` exists, **no onchange** wired | 0% | Blocker |
| Paragraph style dropdown (Heading 1/2/3 etc.) | Full style list | `<select>` placeholder, no apply | 0% | High |
| Bulleted / numbered list buttons | `.uno:DefaultBullet` / `.uno:DefaultNumbering` | Button visible, no handler | 0% | Blocker |
| Indent / outdent | Yes | Button visible, no handler | 0% | High |
| Alignment (L/C/R/Justify) | Yes | Button visible, no handler | 0% | Blocker |
| Text color / highlight color | Yes | Color input visible, no handler | 0% | High |
| Find / Replace dialog | `Control.QuickFindPanel.ts`, `find_dialog_spec.js`, `replace_dialog_spec.js` | None | 0% | Blocker |
| Track changes / accept-reject revisions | `track_changes_spec.js`, `redlinetablechanged:` callback | None | 0% | Medium |
| Comments / annotations on text spans | `annotation_spec.js`, `comment:` callback | Comment **API** exists, no UI gesture in Writer | 10% | High |
| Table insert + edit | Yes, full | Toolbar button stub | 0% | High |
| Image insert (upload + paste) | `insertfile`, `paste mimetype=` | Toolbar button stub | 0% | High |
| Word/character count | Yes | **Done** (real-time on input) | 100% | — |
| Page break / multi-page rendering | Real pagination | Single `.page` div, no flow | 10% | Medium |
| Undo / redo | Full UNO undo stack | Not in Writer | 0% | Blocker |
| Spell check / red squiggles | Browser + LibreOffice dictionaries | None | 0% | Medium |

### Document editing depth — Calc

| Area | Collabora has | Contrix has | Completion % | Severity |
|------|--------------|-------------|--------------|----------|
| Cell click + arrow nav + F2 edit | Yes | **Done** | 100% | — |
| Multi-cell rectangular selection | Yes | **Done** (Shift+click, drag) | 100% | — |
| Multi-cell discontiguous (Ctrl+click) | Yes | **Done** | 100% | — |
| Fill handle drag (copy + numeric increment) | Yes | **Done** (Round 4) | 80% | — |
| Formula evaluation (`=A1+B1`, SUM, etc.) | Full LibreOffice calc engine | Stored verbatim, **never evaluated** | 10% | Blocker |
| Cell formatting (bold, color, borders, number format) | Yes | Toolbar buttons exist, no apply | 0% | Blocker |
| Insert / delete row / column | `row_column_operation_spec.js` | None | 0% | High |
| Insert / delete / rename sheet | Add via `+`, no delete / rename / reorder | Partial | 30% | High |
| Sort A-Z | `.uno:SortAscending` | Toolbar stub | 0% | High |
| AutoFilter | `autofilter_spec.js` | None | 0% | Medium |
| Chart insert / wizard | `chart_dialog_spec.js`, `chart_wizard_data_series_spec.js` | None | 0% | Medium |
| Number formatting (currency, %, decimals) | Yes | Toolbar stub | 0% | High |
| Frozen panes / split | Yes | None | 0% | Medium |
| Cell address input (jump to A1) | `Control.AddressInputField.ts` | Read-only display only | 25% | High |
| Status bar Sum/Average/Count | Yes | **Done** | 100% | — |
| Paste from external (Excel, web table) | Full clipboard pipeline | No paste handler | 0% | High |
| Save round-trip (cells persist after reload) | Yes | **Cells live only in memory** | 0% | Blocker |

### Document editing depth — Slides

| Area | Collabora has | Contrix has | Completion % | Severity |
|------|--------------|-------------|--------------|----------|
| Slide list panel | Yes | **Done** | 100% | — |
| Add / delete / reorder slides | Yes | Add only | 25% | High |
| Edit title / body text on slide | Tile + jsdialog | Single contenteditable, no real model | 25% | Blocker |
| Insert text box, shape, image | `slide_operations_spec.js` | Toolbar stubs | 0% | High |
| Transitions (Fade/Slide/etc.) applied | `transition_spec.js` | `<select>` stores value, never used | 10% | Medium |
| Animations on shapes | `Notebookbar.ImpressAnimationTab.ts` | None | 0% | Low |
| Speaker notes saved | Yes | Textarea exists, never read back | 10% | High |
| Slideshow / present mode | `slideshow_spec.js`, `presenter_console_spec.js`, fullscreen | None | 0% | Blocker |
| Slide layout templates | Multiple layouts | `<select>` stub, never applied | 0% | High |
| Master slide editing | Yes | None | 0% | Low |
| Save round-trip (slides persist) | Yes | **Memory-only** | 0% | Blocker |

### Document editing depth — Whiteboard

| Area | Collabora has (Draw module) | Contrix has | Completion % | Severity |
|------|----------------------------|-------------|--------------|----------|
| Pen / shape tools | Yes | Tool buttons toggle, **no draw handler on SVG** | 10% | Blocker |
| Pointer events draw strokes | Yes | None | 0% | Blocker |
| Undo / redo stack | Yes | Local stack exists, no draw to undo | 50% | High |
| Clear canvas | Yes | **Done** (clears empty array) | 100% | — |
| Export PNG / SVG | Yes | Button shows "coming soon" | 0% | High |
| Save round-trip | Yes | None | 0% | Blocker |

### Real-time collaboration (visible behaviours)

| Area | Collabora has | Contrix has | Completion % | Severity |
|------|--------------|-------------|--------------|----------|
| Multi-user WebSocket transport | Yes | **Done** (Salvo WS) | 100% | — |
| Presence list (avatars / names) | `Control.UserList.ts`, `viewinfo:` | Writer renders pills from `UserJoined` | 50% | — |
| Remote cursor visualisation | `invalidateviewcursor:`, coloured cursors | Backend sends `CursorUpdate`, **frontend ignores it** | 10% | Blocker |
| Remote text selection highlight | `textviewselection:` | None | 0% | High |
| Live op application across clients | Yes | Backend broadcasts, **Calc/Slides/Whiteboard don't subscribe** | 25% | Blocker |
| Per-user editing colours assigned consistently | Yes | Backend assigns colour, used only in pills | 50% | High |
| "User X is editing this cell" lock indicator | `viewlock:` | None | 0% | Medium |
| Comment threads (add/reply/resolve) | `annotation_spec.js`, comment callback | Backend exists, no UI in editors | 10% | High |
| @mention in comments | `Control.Mention.ts` | None | 0% | Medium |
| Document repair / version conflict dialog | `Control.DocumentRepair.js` | None | 0% | Medium |
| Idle / inactive throttling | `useractive`/`userinactive` | None | 0% | Low |

### Toolbar / menu / chrome

| Area | Collabora has | Contrix has | Completion % | Severity |
|------|--------------|-------------|--------------|----------|
| Persistent application menubar (File / Edit / View / Insert / Format / Tools) | `Control.Menubar.ts` | None | 0% | High |
| Notebookbar / ribbon | `Notebookbar*` files | None | 0% | Low |
| Context menu on right-click | `Control.ContextMenu.js` + server `contextmenu:` payload | None | 0% | High |
| Sidebar (properties / styles) | `Control.Sidebar.ts` | Empty `<Sidebar />` shell in Writer | 10% | High |
| Status bar (page, zoom, language, save state) | `Control.StatusBar.js` | Partial (counts only) | 50% | Medium |
| Document title rename indicator (saved/saving/error) | `Control.SaveState.ts` | "Unsaved changes" / "All changes saved" text only | 40% | Medium |
| Global keyboard shortcuts (Ctrl+B/I/U, Ctrl+Z/Y, Ctrl+F) | Yes | Only Ctrl+S (no-op) | 5% | Blocker |

### File operations / persistence

| Area | Collabora has | Contrix has | Completion % | Severity |
|------|--------------|-------------|--------------|----------|
| Open document → load content from server | WOPI GetFile | **Editors never call `GET /documents/:id`** | 0% | Blocker |
| Autosave (debounced) | Yes | None | 0% | Blocker |
| Manual save (Ctrl+S → POST) | Yes | Ctrl+S handled, **no HTTP call** | 10% | Blocker |
| Save indicator (saving / saved / failed) | Yes | Local string only | 30% | High |
| Version history list | Yes via WOPI | Backend `file_versions` table, no UI | 25% | High |
| Restore old version | `versionrestore` | None | 0% | Medium |
| Export to PDF / DOCX / XLSX / PPTX | `downloadas` | Backend export crate exists, **no UI button calls it** | 30% | High |
| Download original | Yes | Backend endpoint exists, not surfaced | 25% | High |

### Integration surface

| Area | Collabora has | Contrix has | Completion % | Severity |
|------|--------------|-------------|--------------|----------|
| WOPI host integration | Full `wsd/Storage.cpp` | `handlers/wopi.rs` exists (skim only) | 30% | Medium |
| postMessage API for embedders | Yes (`PostMessageOrigin`) | None | 0% | Medium |
| OAuth / social login | Out of scope for Collabora | `oauth.rs` handler exists, default.toml disabled | 25% | Medium |
| Embeddable iframe with auth token | Yes | None | 0% | Low |

### Settings / preferences UI

| Area | Collabora has | Contrix has | Completion % | Severity |
|------|--------------|-------------|--------------|----------|
| User profile / password change | n/a (host's job) | Backend endpoints exist; no UI | 25% | High |
| Language / locale picker | `Control.LanguageDialog.js` | None | 0% | Medium |
| Theme / dark mode toggle | Yes | CSS exists (Round 3), **no toggle UI** | 50% | High |
| Keyboard shortcut customisation | Limited | None | 0% | Low |
| Notification preferences | Yes (host's job) | Backend endpoint exists; no UI | 25% | Medium |

### Mobile / touch / responsive

| Area | Collabora has | Contrix has | Completion % | Severity |
|------|--------------|-------------|--------------|----------|
| Mobile-specific top bar | `Control.MobileTopBar.ts` | None | 0% | Medium |
| Bottom bar (mobile) | `Control.MobileBottomBar.js` | None | 0% | Medium |
| Touch-drag selection on Calc | Yes | Mouse-only handlers | 0% | High |
| Pinch zoom | Yes | None | 0% | Medium |
| Responsive layout breakpoint | Yes via cypress mobile suite | CSS in Round 3, untested | 40% | Medium |

### Accessibility

| Area | Collabora has | Contrix has | Completion % | Severity |
|------|--------------|-------------|--------------|----------|
| ARIA labels on toolbar buttons | `a11y_*_spec.js` suites | `title=` only, no `aria-label` | 20% | High |
| Keyboard-only navigation through toolbars | Yes | Only Tab works | 25% | High |
| Screen reader announcements on cell change | `a11ycaretposition` | None | 0% | Medium |
| Focus ring visibility | Yes | Default browser only | 30% | Medium |
| High-contrast mode CSS | Yes | None | 0% | Low |

### Admin / dashboards

| Area | Collabora has | Contrix has | Completion % | Severity |
|------|--------------|-------------|--------------|----------|
| Admin console (active docs, memory, users) | Full | Page exists with tabs (Round M) | 60% | — |
| Force kick session | `kill <pid>` | None | 0% | Medium |
| Live memory/CPU graphs | Yes | Static numbers only | 30% | Low |
| Audit log of admin actions | Some | Backend table only | 20% | Medium |

### Error handling / empty / loading states

| Area | Collabora has | Contrix has | Completion % | Severity |
|------|--------------|-------------|--------------|----------|
| Loading spinner while document loads | Yes | None (editors render blank) | 0% | Blocker |
| "Document not found" error page | Yes | None | 0% | High |
| Network reconnect banner on WS disconnect | Yes | Silently fails | 0% | Blocker |
| Conflict resolution dialog | Yes | None | 0% | Medium |
| Toast notifications for save errors | Yes | None | 0% | High |

---

## Part B — Prioritized task list

### Phase α — Blockers (must land before claiming basic usability)

Each task is sized to one agent. Files are absolute paths. Approach lines are intentionally terse.

#### Batch T — Persistence round-trip (T1-T4 mostly sequential, T2/T3/T4 can parallelise after T1)

- **T1. Document load on editor mount** — Why: editors today render empty even when the document exists. Touch: `crates/frontend/src/pages/writer.rs`, `pages/calc.rs`, `pages/slides.rs`, `pages/whiteboard.rs`, `state.rs`. Approach: in `use_effect`, call `api_get(format!("/api/v1/documents/{id}"))`, deserialize the typed content (WriterDocument / Workbook / Presentation / WhiteboardScene from `doc-engine`), hydrate signals. Parallelisable: N (touches `state.rs`).
- **T2. Debounced autosave in all four editors** — Why: zero work is persisted. Touch: same four pages. Approach: spawn a `gloo_timers::callback::Timeout` on every state change; 1.5s debounce; `api_put("/api/v1/documents/{id}", json_body)`; flip `saved_status` to "Saving…" → "All changes saved" / "Save failed". Parallelisable: Y (one agent per editor after T1).
- **T3. Ctrl+S actually saves** — Why: users hit Ctrl+S and nothing reaches the server. Touch: same four pages. Approach: in existing `onkeydown` Ctrl+S branch, call the same autosave fn from T2 (force-flush). Parallelisable: Y (folds into T2).
- **T4. Calc cells round-trip through `Workbook` model** — Why: Calc's HashMap is throwaway; can't reload the same data. Touch: `crates/frontend/src/pages/calc.rs`. Approach: replace the `HashMap<(usize,u32,u32), String>` with a real `Workbook` from `doc-engine`; on every commit, mutate the Workbook; on autosave (T2), serialise the Workbook to JSON. Parallelisable: N (depends on T1, T2).

#### Batch U — Toolbar wiring (each button = one task, all parallel)

- **U1. Writer Bold/Italic/Underline/Strikethrough buttons** — Why: buttons exist but onclick is missing — exact source of "操作上的问题". Touch: `crates/frontend/src/components/toolbar.rs`, `pages/writer.rs`. Approach: lift a `format_command: Signal<Option<String>>` into Writer state; toolbar buttons set it; Writer effect runs `document.execCommand("bold")` via `web_sys`. Add Ctrl+B / Ctrl+I / Ctrl+U keybindings. Parallelisable: Y.
- **U2. Writer Font family + Font size selects** — Touch: same files. Approach: `onchange` handler on `<select>` runs `execCommand("fontName", false, value)` and `"fontSize"`. Parallelisable: Y.
- **U3. Writer alignment buttons (L/C/R/Justify) + Lists + Indent/Outdent** — Touch: same. Approach: each maps to `execCommand("justifyLeft" / "insertUnorderedList" / "indent" / "outdent")`. Parallelisable: Y.
- **U4. Writer Text Color + Highlight Color inputs** — Touch: same. Approach: `oninput` → `execCommand("foreColor" / "hiliteColor", false, value)`. Parallelisable: Y.
- **U5. Calc cell formatting buttons (Bold / Italic / Underline)** — Touch: `pages/calc.rs`. Approach: add `cell_styles: HashMap<(usize,u32,u32), CellStyle>` signal; clicking toolbar mutates style for current selection; CSS classes render bold/italic. Parallelisable: Y.
- **U6. Calc number format buttons ($ / % / .0 / .00)** — Touch: same. Approach: store `format: NumberFormat` per cell in style map; display layer formats f64 according to it (`{:.2}`, currency prefix, `* 100` + `%`). Parallelisable: Y.

#### Batch V — Editor core gestures

- **V1. Find dialog in Writer + Calc** — Why: no way to find text. Touch: new `components/find_dialog.rs`; mount via Ctrl+F. Approach: simple modal with input + Next/Prev; in Writer use `window.find()`; in Calc scan the cell map. Parallelisable: Y.
- **V2. Undo / Redo for Writer + Calc** — Why: every editor needs it. Touch: `pages/writer.rs`, `pages/calc.rs`. Approach: in Writer rely on `execCommand("undo"/"redo")` + Ctrl+Z/Y; in Calc maintain `Vec<Snapshot>` of HashMap before each mutation, cap at 100. Parallelisable: Y.
- **V3. Calc paste from external clipboard** — Why: users cannot paste rows from Excel/web. Touch: `pages/calc.rs`. Approach: `onpaste` event on the grid; read `text/plain`, split on `\n` then `\t`, fill from active cell. Parallelisable: Y.
- **V4. Calc formula evaluation (basic)** — Why: `=A1+B1` is stored verbatim. Touch: new `crates/doc-engine/src/calc/eval.rs` if not present; otherwise extend it; wire into `pages/calc.rs` display. Approach: implement A1 reference resolution + arithmetic + SUM/AVG/MIN/MAX/COUNT/IF. Cycle-detect with a visit set. Parallelisable: Y once eval module exists.
- **V5. Loading spinner + error states in editors** — Why: blank canvas during load looks broken. Touch: all four editor pages. Approach: `load_state: Signal<LoadState>` with Loading / Loaded / Error variants; render skeleton or message. Parallelisable: Y.

#### Batch W — Collaboration polish

- **W1. Render remote cursors in Writer** — Why: backend sends `CursorUpdate`, frontend currently ignores it (Writer collab is presence-pills only). Touch: `pages/writer.rs`, `collab_client.rs`. Approach: handle `CollabMessage::CursorUpdate`; track positions in a `HashMap<user_id, (line, col)>`; overlay coloured `<div>` at computed pixel position; show name flag on hover. Parallelisable: N.
- **W2. WS reconnect with backoff + visible banner** — Why: silent failure on flaky network. Touch: `collab_client.rs`, all four editor pages. Approach: in `onclose`, schedule reconnect with 1s/2s/5s/15s backoff; expose `connection_state: ConnectionState`; banner reads "Reconnecting… (3)". Parallelisable: N (touches shared file).
- **W3. Apply remote ops in Calc** — Why: Writer subscribes but Calc doesn't, so Calc is single-user despite the backend supporting it. Touch: `pages/calc.rs`. Approach: copy the `use_effect(connect_collab)` pattern from Writer; on `CollabMessage::Operation`, parse cell ops and apply to the HashMap. Parallelisable: Y (after T4).

---

### Phase β — High-value polish (1–2 weeks to reach a defensible beta)

- **X1. Application menubar component** — File / Edit / View / Insert / Format / Tools / Help dropdowns wired to existing toolbar actions. New `components/menubar.rs`. Parallel: Y.
- **X2. Context menu on right-click in Writer / Calc / Slides** — `oncontextmenu` opens a `Menu` component with Cut / Copy / Paste / Delete / Insert. Parallel: Y.
- **X3. Comments UI inside editors** — Comment API exists; wire a sidebar pane and "Add comment" gesture (select text → button). Pages writer.rs, calc.rs. Parallel: Y.
- **X4. Export menu** — Dropdown in toolbar / File menu calling `POST /api/v1/documents/:id/export?format=pdf|docx|xlsx|pptx`, then download blob. Touch every editor page + `state.rs`. Parallel: Y.
- **X5. Version history panel** — UI for existing `/api/v1/file-versions/:file_id` endpoint; show timeline + Restore button. New `components/version_history.rs`. Parallel: Y.
- **X6. Calc insert/delete row + column** — Right-click row header → context menu → mutate Workbook. Touch `pages/calc.rs`. Parallel: Y (after T4 + X2).
- **X7. Calc sheet rename + delete + reorder** — Double-click tab to rename; right-click for Delete; drag to reorder. Touch `pages/calc.rs`. Parallel: Y.
- **X8. Address input box (jump to A1)** — Make `cell_ref` span an editable input that on Enter parses "B17" and jumps. Touch `pages/calc.rs`. Parallel: Y.
- **X9. Slides reorder / delete + speaker notes persistence** — Drag thumbnails to reorder; delete button on hover; textarea writes to a per-slide notes field on the Presentation model. Touch `pages/slides.rs` + `doc-engine`. Parallel: Y.
- **X10. Slides "Present" button enters fullscreen view** — Plain `requestFullscreen` on a div that renders one slide at a time with Esc / arrow nav. Touch `pages/slides.rs`. Parallel: Y.
- **X11. Whiteboard pointer drawing** — `onpointerdown/move/up` on SVG; emit `<path>` strokes; persist into `WhiteboardScene` from doc-engine. Touch `pages/whiteboard.rs`. Parallel: N (depends on T1).
- **X12. Theme toggle in Navbar** — Persist `localStorage["theme"]`; toggle `data-theme="dark"` on `<html>`. Touch `components/navbar.rs`. Parallel: Y.
- **X13. Toast notification system** — Reusable `Signal<Vec<Toast>>` in `state.rs` + component. Wire save-failed, share-success, etc. Parallel: Y.
- **X14. ARIA labels + focus-visible CSS** — Add `aria-label` to every toolbar button; visible focus ring CSS; tab order audit. Touch `components/toolbar.rs`, `static/assets/*.css`. Parallel: Y.
- **X15. Profile / password change UI** — Page using existing backend endpoints. New `pages/profile.rs`. Parallel: Y.

---

### Phase γ — Long tail (months of work, listed briefly, not scheduled)

- Real find-and-replace dialog (with regex, case, whole-word options) for Writer + Calc.
- Track changes UI in Writer (accept/reject/sidebar).
- Comment @mention autocomplete.
- Calc charts (insert chart of selection — needs SVG renderer, not LibreOffice).
- Calc autofilter + sort-by-column UI.
- Calc cell border editor (per-side border with style/width/color).
- Slides animations on shapes.
- Slides master slide / layout editor.
- Slides presenter console (next-slide preview, notes, timer).
- Whiteboard shape selection + resize + delete.
- Whiteboard PNG / SVG export (canvas-to-blob).
- Spell-check integration (browser's `spellcheck="true"` is a start; server-side dictionary harder).
- Mobile bottom bar + touch selection in Calc.
- Pinch-zoom on canvases.
- Real document repair / conflict-resolution dialog.
- postMessage API for embedders (a la WOPI host integration).
- WOPI Discovery XML completeness.
- SSO / SAML.
- Plugin / extension API.
- Localisation / i18n string extraction.
- AI-assist features (summarise / translate / grammar).
- High-contrast accessibility theme.
- Service-worker offline edit queue with reconciliation.

---

## What's actually shipped (the honest "you've done a lot" section)

The last four rounds genuinely moved the needle. Anyone reading this should not interpret the low parity % as "Contrix is broken" — most of the missing surface is *frontend wiring*, not missing backend capability.

- **Backend coverage is substantial**: 165 API endpoints across 47 handler files. Auth, rooms, shares, comments, files, folders, file-versions, trash, recent, favorites, tags, templates, presence, search (Tantivy), notifications, webhooks, WebSocket collab, WOPI handler shell, OAuth handler shell, SMTP shell, monitoring, admin user management, branding, watermark, signatures, API keys, IP restrictions, sessions, TOTP, password policy, quota, batch ops, capabilities, client logs.
- **CRDT collaboration engine** in `crates/collab/` is real, not a stub, with broadcast channels and per-document state.
- **Office format I/O** (Round 4): DOCX / XLSX / PPTX import and export round-trip through the internal model, 40 unit tests pass.
- **Calc UX work** (recent round): double-click edit, fill handle with numeric increment, Ctrl/Shift multi-select, Sum/Average/Count in status bar — all now work and are covered by 11 Playwright UI tests.
- **Writer presence pills + WebSocket subscription** (Round N).
- **PWA manifest + service worker + dark-mode + mobile + print CSS** (Rounds I, J, O).
- **Admin dashboard tabs wired to real APIs** (Round M).
- **Notification publishers** for shares/comments (Round L).
- **Webhook fire-event on relevant handlers** (Round H).
- **E2E coverage**: 74 API tests + 11 Calc UI tests, all passing.
- **9 database migrations, 37+ tables, 9 e2e test files**.

The work that exists is solid. The work that's missing is concentrated in one place: **the frontend pages do not yet treat their editor surface as a real document model with save/load round-trips**. That's where the user's frustration is coming from, and that's exactly what Phase α attacks.

---

## Cumulative task budget after this round

- Round 5 Phase α (Batch T+U+V+W): **15 tasks**
- Round 5 Phase β (Batch X): **15 tasks**
- Round 5 Phase γ: ~25 line items, multi-month
- Combined α+β: **30 tasks**, each scoped for a single agent dispatch.

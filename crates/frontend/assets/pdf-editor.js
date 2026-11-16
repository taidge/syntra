// PDF.js reads the original pages. pdf-lib changes only requested objects/pages.
// Dependencies and fonts are packaged locally; opening a PDF makes no CDN call.
const VENDOR = '/assets/pdf-vendor/';
const MAX_BYTES = 32 * 1024 * 1024;
const HISTORY_BYTES = 64 * 1024 * 1024;
let libraries;
let cjkBytes;

function loadLibraries() {
  return libraries ||= Promise.all([
    import(`${VENDOR}pdf.mjs`), import(`${VENDOR}pdf-lib.esm.min.js`), import(`${VENDOR}fontkit.es.min.js`),
  ]).then(([reader, writer, fontkit]) => {
    reader.GlobalWorkerOptions.workerSrc = `${VENDOR}pdf.worker.mjs`;
    return { reader, writer, fontkit: fontkit.default };
  });
}
function stylesheet(href) {
  if (document.querySelector(`link[href="${href}"]`)) return;
  const link = document.createElement('link');
  link.rel = 'stylesheet'; link.href = href; document.head.append(link);
}
function equalBytes(left, right) {
  return left?.length === right?.length && left.every((value, index) => value === right[index]);
}
async function matchesFontStream(stream, expected, writer) {
  const encoded = stream.getContents?.();
  if (!encoded) return false;
  const filter = stream.dict.get(writer.PDFName.of('Filter'))?.toString();
  if (!filter) return equalBytes(encoded, expected);
  if (filter !== '/FlateDecode') return false;
  // Fonts come from an untrusted PDF. Compare incrementally and stop at the
  // known font length, rather than inflating an arbitrary stream without bounds.
  const reader = new Blob([encoded]).stream().pipeThrough(new DecompressionStream('deflate')).getReader();
  let offset = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) return offset === expected.length;
      if (offset + value.length > expected.length) return false;
      for (let index = 0; index < value.length; index++) if (value[index] !== expected[offset + index]) return false;
      offset += value.length;
    }
  } catch { return false; }
  finally { await reader.cancel().catch(() => {}); }
}
async function annotationFont(editor, document) {
  cjkBytes ||= fetch(`${VENDOR}office-cjk.ttf`).then(response => {
    if (!response.ok) throw new Error('The annotation font is unavailable. Rebuild the browser PDF assets.');
    return response.arrayBuffer();
  }).catch(error => { cjkBytes = null; throw error; });
  const bytes = new Uint8Array(await cjkBytes), writer = editor.libs.writer;
  const embedder = await writer.CustomFontEmbedder.for(editor.libs.fontkit, bytes, 'ContrixAnnotationCJK');
  const name = writer.PDFName.of('BaseFont');
  for (const [ref, object] of document.context.enumerateIndirectObjects()) {
    if (!(object instanceof writer.PDFDict) || object.get(name)?.toString() !== '/ContrixAnnotationCJK') continue;
    try {
      const descendant = object.lookup(writer.PDFName.of('DescendantFonts'), writer.PDFArray).lookup(0, writer.PDFDict);
      const descriptor = descendant.lookup(writer.PDFName.of('FontDescriptor'), writer.PDFDict);
      const stream = descriptor.lookup(writer.PDFName.of('FontFile2'));
      if (!await matchesFontStream(stream, bytes, writer)) continue;
      // The complete font and ToUnicode map already exist. A public PDFFont
      // wrapper reuses that reference without adding another 6 MB font stream.
      return { font: writer.PDFFont.of(ref, document, embedder), embedder, fresh: false };
    } catch { /* An unrelated font with a similar name is never trusted. */ }
  }
  return { font: writer.PDFFont.of(document.context.nextRef(), document, embedder), embedder, fresh: true };
}
async function readCmap(stream, writer) {
  const encoded = stream.getContents?.();
  if (!encoded || encoded.length > 2 * 1024 * 1024) return '';
  const filter = stream.dict.get(writer.PDFName.of('Filter'))?.toString();
  if (!filter) return new TextDecoder().decode(encoded);
  if (filter !== '/FlateDecode') return '';
  const reader = new Blob([encoded]).stream().pipeThrough(new DecompressionStream('deflate')).getReader();
  const chunks = []; let total = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      total += value.length;
      if (total > 2 * 1024 * 1024) return '';
      chunks.push(value);
    }
    const bytes = new Uint8Array(total); let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
    return new TextDecoder().decode(bytes);
  } catch { return ''; }
  finally { await reader.cancel().catch(() => {}); }
}
async function finishAnnotationFont(editor, document, annotation, text) {
  const { font, embedder, fresh } = annotation, writer = editor.libs.writer;
  // A full CJK font includes GSUB alternate glyphs which fontkit's default
  // ToUnicode map omits. Capture the Unicode sequences actually laid out before
  // creating its broad font cache, then retain them across later annotations.
  const actual = text.split(/\r\n|\r|\n/).flatMap(line => embedder.font.layout(line, embedder.fontFeatures).glyphs.map(glyph => [glyph.id, [...glyph.codePoints], glyph.advanceWidth * embedder.scale]));
  if (fresh) await font.embed();
  const dictionary = document.context.lookup(font.ref, writer.PDFDict);
  const descendant = dictionary.lookup(writer.PDFName.of('DescendantFonts'), writer.PDFArray).lookup(0, writer.PDFDict);
  const widths = descendant.lookup(writer.PDFName.of('W'), writer.PDFArray);
  for (const [id, , width] of actual) { widths.push(document.context.obj(id)); widths.push(document.context.obj([width])); }
  const reference = dictionary.get(writer.PDFName.of('ToUnicode'));
  const previous = await readCmap(document.context.lookup(reference), writer);
  const mapping = new Map();
  for (const match of previous.matchAll(/^<([0-9A-Fa-f]{4})>\s+<([0-9A-Fa-f]{1,64})>$/gm)) mapping.set(Number.parseInt(match[1], 16), match[2]);
  for (const [id, points] of actual) {
    const value = String.fromCodePoint(...points);
    mapping.set(id, Array.from({ length: value.length }, (_, index) => value.charCodeAt(index).toString(16).padStart(4, '0')).join(''));
  }
  const pairs = [...mapping].sort((left, right) => left[0] - right[0]).map(([id, unicode]) => `<${id.toString(16).padStart(4, '0')}> <${unicode}>`);
  const chunks = [];
  for (let index = 0; index < pairs.length; index += 100) {
    const group = pairs.slice(index, index + 100); chunks.push(`${group.length} beginbfchar\n${group.join('\n')}\nendbfchar`);
  }
  const cmap = `/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Contrix-Annotation-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000><ffff>\nendcodespacerange\n${chunks.join('\n')}\nendcmap\nCMapName currentdict /CMap defineresource pop\nend\nend`;
  document.context.assign(reference, document.context.flateStream(cmap));
}
function download(bytes, name) {
  const url = URL.createObjectURL(new Blob([bytes], { type: 'application/pdf' }));
  const anchor = document.createElement('a');
  anchor.href = url; anchor.download = name; anchor.hidden = true;
  document.body.append(anchor); anchor.click(); anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
async function errorMessage(response) {
  try { return (await response.json()).error?.message || `Request failed (${response.status}).`; }
  catch { return `Request failed (${response.status}).`; }
}

export function mountPdfEditor(selector, fileId, token) {
  const editor = {
    selector, fileId, token, root: null, destroyed: false, abort: new AbortController(),
    pdf: null, loading: null, renderTask: null, textLayer: null, renderSerial: 0,
    bytes: null, savedBytes: null, etag: null, canEdit: false, protected: false,
    current: 0, zoom: 1, fit: true, busy: false, dirty: false, tool: 'select',
    undo: [], redo: [], matches: [], matchIndex: -1, query: '', searchSerial: 0,
  };
  const attach = () => {
    if (editor.destroyed) return;
    editor.root = document.querySelector(selector);
    if (!editor.root) { editor.frame = requestAnimationFrame(attach); return; }
    initialize(editor).catch(error => {
      if (!editor.destroyed) showMessage(editor, error.message || 'Unable to open this PDF.', true);
    });
  };
  editor.frame = requestAnimationFrame(attach);
  return editor;
}

async function api(editor, path, options = {}) {
  const response = await fetch(`/api/v1${path}`, {
    ...options, signal: editor.abort.signal,
    headers: { Authorization: `Bearer ${editor.token}`, ...options.headers },
  });
  if (!response.ok) {
    const error = new Error(await errorMessage(response)); error.status = response.status; throw error;
  }
  return response;
}
function showMessage(editor, text, error = false) {
  if (!editor.root) return;
  const target = editor.root.querySelector('.pdf-message');
  target.textContent = text; target.classList.toggle('error', error); target.hidden = !text;
}
function setBusy(editor, busy) { editor.busy = busy; updateControls(editor); }
function updateControls(editor) {
  if (!editor.root) return;
  const loaded = !!editor.pdf;
  const editing = loaded && editor.canEdit && !editor.protected;
  for (const control of editor.root.querySelectorAll('[data-edit]')) control.disabled = editor.busy || !editing;
  for (const control of editor.root.querySelectorAll('[data-loaded]')) control.disabled = editor.busy || !loaded;
  for (const control of editor.root.querySelectorAll('[data-action="previous"]')) control.disabled = editor.busy || !loaded || editor.current === 0;
  for (const control of editor.root.querySelectorAll('[data-action="next"]')) control.disabled = editor.busy || !loaded || editor.current + 1 >= editor.pdf.numPages;
  editor.root.querySelector('[data-action="undo"]').disabled = editor.busy || !editing || !editor.undo.length;
  editor.root.querySelector('[data-action="redo"]').disabled = editor.busy || !editing || !editor.redo.length;
  editor.root.querySelector('[data-action="save"]').disabled = editor.busy || !editing || !editor.dirty;
  editor.root.querySelector('[data-action="delete"]').disabled = editor.busy || !editing || editor.pdf.numPages <= 1;
  editor.root.querySelector('[data-action="earlier"]').disabled = editor.busy || !editing || editor.current === 0;
  editor.root.querySelector('[data-action="later"]').disabled = editor.busy || !editing || editor.current + 1 >= editor.pdf.numPages;
  editor.root.querySelector('[data-action="convert"]').disabled = editor.busy || !loaded || editor.protected;
  editor.root.querySelector('.pdf-page-number').value = loaded ? editor.current + 1 : 1;
  editor.root.querySelector('.pdf-page-number').max = loaded ? editor.pdf.numPages : 1;
  editor.root.querySelector('.pdf-page-count').textContent = loaded ? `/ ${editor.pdf.numPages}` : '/ —';
  editor.root.querySelector('.pdf-zoom').value = String(Math.round(editor.zoom * 100));
  editor.root.querySelector('.pdf-save-status').textContent = editor.busy ? 'Working…' : editor.dirty ? 'Unsaved changes' : loaded ? 'All changes saved' : 'Loading PDF…';
  editor.root.querySelector('.pdf-readonly').hidden = !loaded || editing;
  editor.root.querySelector('.pdf-readonly').textContent = editor.protected
    ? 'Encrypted PDF: reading is available; editing and conversion are unavailable.'
    : 'Read-only access. You can read and download this PDF.';
  editor.root.querySelector('.pdf-tool-hint').textContent = editor.tool === 'text'
    ? 'Enter text, then click its position on the page.'
    : editor.tool === 'highlight' ? 'Drag across the area to highlight.' : 'Select and copy text from the page.';
  for (const button of editor.root.querySelectorAll('[data-tool]')) button.setAttribute('aria-pressed', String(button.dataset.tool === editor.tool));
  editor.root.querySelector('.pdf-page-wrap')?.setAttribute('data-tool', editor.tool);
  for (const button of editor.root.querySelectorAll('.pdf-page-option')) {
    const active = Number(button.dataset.page) === editor.current;
    button.classList.toggle('active', active); button.setAttribute('aria-current', active ? 'page' : 'false');
    button.disabled = editor.busy;
  }
}

async function initialize(editor) {
  stylesheet(`${VENDOR}pdf_viewer.css`); stylesheet('/assets/pdf-editor.css');
  editor.root.innerHTML = `
    <header class="pdf-header"><a href="/files" class="pdf-back">Files</a><h1 class="pdf-title">PDF</h1><span class="pdf-save-status" role="status">Loading PDF…</span>
      <button type="button" data-action="original">Download original</button><button type="button" data-action="download" data-loaded>Download PDF</button>
      <button type="button" data-action="convert" data-loaded>Convert to Writer</button><button type="button" data-action="cancel-ocr" hidden>Cancel OCR</button><button type="button" class="pdf-primary" data-action="save" data-edit>Save PDF</button></header>
    <div class="pdf-reader-toolbar" role="toolbar" aria-label="PDF reader tools">
      <button type="button" data-action="previous" data-loaded aria-label="Previous page">Previous</button>
      <label>Page <input type="number" class="pdf-page-number" aria-label="PDF page number" min="1" value="1" data-loaded></label><span class="pdf-page-count">/ —</span>
      <button type="button" data-action="next" data-loaded aria-label="Next page">Next</button>
      <label>Zoom <input type="number" class="pdf-zoom" aria-label="PDF zoom percent" min="25" max="300" step="25" value="100" data-loaded>%</label>
      <button type="button" data-action="fit" data-loaded>Fit width</button>
      <form class="pdf-find-form"><input type="search" class="pdf-find" aria-label="Find in PDF" placeholder="Find in PDF" maxlength="200"><button type="submit" data-loaded>Find</button></form>
      <button type="button" data-action="previous-match" data-loaded aria-label="Previous search result">↑</button><button type="button" data-action="next-match" data-loaded aria-label="Next search result">↓</button><span class="pdf-find-status" role="status"></span>
    </div>
    <p class="pdf-message" role="status" aria-live="polite" hidden></p><p class="pdf-readonly" hidden></p>
    <form class="pdf-password-form" hidden><label>PDF password <input class="pdf-password" type="password" autocomplete="off" aria-label="PDF password"></label><button type="submit">Unlock PDF</button><span class="pdf-password-status" role="alert"></span></form>
    <div class="pdf-workspace">
      <nav class="pdf-page-list" aria-label="PDF pages"></nav>
      <div class="pdf-stage" aria-label="PDF reading area"><div class="pdf-page-wrap" data-tool="select"></div></div>
      <aside class="pdf-edit-panel" aria-label="PDF editing tools">
        <h2>Edit PDF</h2><p>Add text and highlights, or arrange pages. To rewrite the body, convert to Writer.</p>
        <div class="pdf-tool-buttons"><button type="button" data-tool="select" data-loaded aria-pressed="true">Select text</button><button type="button" data-tool="text" data-edit aria-pressed="false">Add text</button><button type="button" data-tool="highlight" data-edit aria-pressed="false">Highlight</button></div>
        <label>Text to add<textarea class="pdf-new-text" aria-label="Text to add" rows="3" maxlength="2000" data-edit></textarea></label>
        <div class="pdf-text-style"><label>Font size<input class="pdf-font-size" aria-label="Annotation font size" type="number" min="6" max="96" value="14" data-edit></label><label>Color<input class="pdf-font-color" aria-label="Annotation text color" type="color" value="#111827" data-edit></label></div>
        <p class="pdf-tool-hint">Select and copy text from the page.</p>
        <h2>Pages</h2><div class="pdf-page-tools"><button type="button" data-action="rotate-left" data-edit>Rotate left</button><button type="button" data-action="rotate-right" data-edit>Rotate right</button><button type="button" data-action="earlier" data-edit>Move page earlier</button><button type="button" data-action="later" data-edit>Move page later</button><button type="button" class="pdf-danger" data-action="delete" data-edit>Delete page</button></div>
        <div class="pdf-history"><button type="button" data-action="undo" data-edit>Undo</button><button type="button" data-action="redo" data-edit>Redo</button></div>
        <p class="pdf-edit-note">Saving creates a new version. The original PDF remains available. Writer conversion extracts editable text; check OCR results, formulas and table layouts.</p>
      </aside>
    </div>`;
  const signal = editor.abort.signal;
  editor.root.addEventListener('click', event => onClick(editor, event).catch(error => showMessage(editor, error.message, true)), { signal });
  editor.root.querySelector('.pdf-find-form').addEventListener('submit', event => { event.preventDefault(); find(editor).catch(error => showMessage(editor, error.message, true)); }, { signal });
  editor.root.querySelector('.pdf-password-form').addEventListener('submit', event => {
    event.preventDefault();
    const password = editor.root.querySelector('.pdf-password').value;
    editor.root.querySelector('.pdf-password').value = '';
    if (editor.unlock) { editor.root.querySelector('.pdf-password-form').hidden = true; editor.unlock(password); }
  }, { signal });
  editor.root.querySelector('.pdf-page-number').addEventListener('change', event => navigate(editor, Number(event.target.value) - 1), { signal });
  editor.root.querySelector('.pdf-zoom').addEventListener('change', event => {
    editor.fit = false; editor.zoom = Math.min(3, Math.max(.25, Number(event.target.value) / 100 || 1)); render(editor).catch(error => showMessage(editor, error.message, true));
  }, { signal });
  window.addEventListener('beforeunload', event => { if (editor.dirty) { event.preventDefault(); event.returnValue = ''; } }, { signal });
  updateControls(editor);
  editor.libs = await loadLibraries();
  if (editor.destroyed) return;
  const metadata = await (await api(editor, `/files/${editor.fileId}`)).json();
  editor.name = metadata.data.name;
  editor.root.querySelector('.pdf-title').textContent = editor.name;
  await loadCurrent(editor);
  editor.resize = new ResizeObserver(() => {
    if (editor.fit && editor.pdf && !editor.busy) {
      clearTimeout(editor.resizeTimer);
      editor.resizeTimer = setTimeout(() => render(editor).catch(error => showMessage(editor, error.message, true)), 100);
    }
  });
  editor.resize.observe(editor.root.querySelector('.pdf-stage'));
}

async function pdfResponseBytes(response) {
  if (Number(response.headers.get('Content-Length')) > MAX_BYTES) throw new Error('PDF exceeds the 32 MB reader limit. Download the original instead.');
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (bytes.length > MAX_BYTES) throw new Error('PDF exceeds the 32 MB reader limit. Download the original instead.');
  return bytes;
}
async function loadCurrent(editor) {
  setBusy(editor, true);
  try {
    const response = await api(editor, `/files/${editor.fileId}/pdf`);
    const bytes = await pdfResponseBytes(response);
    editor.etag = response.headers.get('ETag');
    editor.canEdit = response.headers.get('X-Pdf-Can-Edit') === 'true';
    editor.encrypted = response.headers.get('X-Pdf-Encrypted') === 'true';
    editor.savedBytes = bytes; editor.bytes = bytes; editor.dirty = false;
    await loadDocument(editor, bytes, 0);
    showMessage(editor, '');
  } finally { setBusy(editor, false); }
}
async function loadDocument(editor, bytes, page) {
  editor.searchSerial++; editor.matches = []; editor.matchIndex = -1;
  editor.renderSerial++; editor.renderTask?.cancel(); editor.textLayer?.cancel();
  const previous = editor.loading;
  editor.protected = !!editor.encrypted;
  const task = editor.libs.reader.getDocument({
    data: bytes.slice(), isEvalSupported: false, enableScripting: false, enableXfa: false,
    cMapUrl: `${VENDOR}cmaps/`, cMapPacked: true,
    standardFontDataUrl: `${VENDOR}standard_fonts/`, wasmUrl: `${VENDOR}wasm/`,
    disableAutoFetch: true, canvasMaxAreaInBytes: 64 * 1024 * 1024,
  });
  editor.pendingLoading = task;
  task.onPassword = (updatePassword, reason) => {
    if (editor.destroyed) return;
    editor.protected = true; editor.unlock = updatePassword;
    editor.root.querySelector('.pdf-password-form').hidden = false;
    editor.root.querySelector('.pdf-password-status').textContent = reason === 2 ? 'Incorrect password. Try again.' : '';
    editor.root.querySelector('.pdf-password').focus();
  };
  const next = await task.promise;
  if (editor.destroyed) { await task.destroy(); return; }
  if (next.numPages > 500) { await task.destroy(); throw new Error('This PDF exceeds the 500-page reader limit. Download the original instead.'); }
  editor.pdf = next;
  editor.loading = task; editor.pendingLoading = null;
  await previous?.destroy();
  editor.current = Math.max(0, Math.min(page, editor.pdf.numPages - 1));
  editor.root.querySelector('.pdf-password-form').hidden = true; editor.unlock = null;
  buildPages(editor);
  await render(editor);
  const metadata = await editor.pdf.getMetadata().catch(() => null);
  if (metadata?.info?.IsSignaturesPresent) showMessage(editor, 'This PDF contains digital signatures. Editing creates a new version and invalidates those signatures.');
  updateControls(editor);
}

function buildPages(editor) {
  editor.thumbnails?.disconnect();
  const rail = editor.root.querySelector('.pdf-page-list'); rail.replaceChildren();
  const observer = new IntersectionObserver(entries => {
    for (const entry of entries) if (entry.isIntersecting) {
      observer.unobserve(entry.target);
      thumbnail(editor, entry.target).catch(() => {});
    }
  }, { root: rail, rootMargin: '160px' });
  editor.thumbnails = observer;
  for (let index = 0; index < editor.pdf.numPages; index++) {
    const button = document.createElement('button'); button.type = 'button';
    button.className = 'pdf-page-option'; button.dataset.page = index;
    button.setAttribute('aria-label', `PDF page ${index + 1}`);
    const canvas = document.createElement('canvas'); canvas.width = 100; canvas.height = 140; canvas.setAttribute('aria-hidden', 'true');
    const number = document.createElement('span'); number.textContent = index + 1;
    button.append(canvas, number); rail.append(button); observer.observe(button);
  }
}
async function thumbnail(editor, button) {
  const pdf = editor.pdf;
  const page = await pdf.getPage(Number(button.dataset.page) + 1);
  if (editor.destroyed || pdf !== editor.pdf || !button.isConnected) return;
  const original = page.getViewport({ scale: 1 });
  const viewport = page.getViewport({ scale: Math.min(100 / original.width, 140 / original.height) });
  const canvas = button.querySelector('canvas');
  canvas.width = Math.ceil(viewport.width); canvas.height = Math.ceil(viewport.height);
  await page.render({ canvas, canvasContext: canvas.getContext('2d'), viewport }).promise;
}
async function navigate(editor, page) {
  if (!editor.pdf || editor.busy) return;
  editor.current = Math.max(0, Math.min(Math.round(page), editor.pdf.numPages - 1));
  await render(editor).catch(error => showMessage(editor, error.message, true));
  editor.root.querySelector('.pdf-page-option.active')?.scrollIntoView({ block: 'nearest' });
}
async function render(editor) {
  if (!editor.pdf || editor.destroyed) return;
  const serial = ++editor.renderSerial;
  editor.renderTask?.cancel(); editor.textLayer?.cancel();
  const page = await editor.pdf.getPage(editor.current + 1);
  if (serial !== editor.renderSerial || editor.destroyed) return;
  const base = page.getViewport({ scale: 4 / 3 });
  const stage = editor.root.querySelector('.pdf-stage');
  if (editor.fit) editor.zoom = Math.max(.1, Math.min(3, (stage.clientWidth - 48) / base.width));
  const viewport = page.getViewport({ scale: editor.zoom * 4 / 3 });
  editor.viewport = viewport;
  const wrap = editor.root.querySelector('.pdf-page-wrap');
  wrap.style.width = `${viewport.width}px`; wrap.style.height = `${viewport.height}px`;
  wrap.style.setProperty('--scale-factor', String(viewport.scale));
  wrap.style.setProperty('--total-scale-factor', String(viewport.scale * (viewport.userUnit || 1)));
  const canvas = document.createElement('canvas'); canvas.setAttribute('aria-label', `PDF page ${editor.current + 1} preview`);
  const density = Math.min(window.devicePixelRatio || 1, 2, Math.sqrt(16000000 / (viewport.width * viewport.height)));
  canvas.width = Math.max(1, Math.ceil(viewport.width * density)); canvas.height = Math.max(1, Math.ceil(viewport.height * density));
  canvas.style.width = `${viewport.width}px`; canvas.style.height = `${viewport.height}px`;
  const textLayer = document.createElement('div'); textLayer.className = 'textLayer';
  const inputLayer = document.createElement('div'); inputLayer.className = 'pdf-input-layer'; inputLayer.setAttribute('aria-label', 'Place PDF annotation');
  wrap.replaceChildren(canvas, textLayer, inputLayer);
  editor.renderTask = page.render({ canvas, canvasContext: canvas.getContext('2d'), viewport, transform: [density, 0, 0, density, 0, 0] });
  try { await editor.renderTask.promise; } catch (error) { if (error.name !== 'RenderingCancelledException') throw error; }
  if (serial !== editor.renderSerial || editor.destroyed) return;
  const content = await page.getTextContent();
  editor.textLayer = new editor.libs.reader.TextLayer({ textContentSource: content, container: textLayer, viewport });
  try { await editor.textLayer.render(); }
  catch (error) { if (serial !== editor.renderSerial || editor.destroyed) return; throw error; }
  if (serial !== editor.renderSerial || editor.destroyed) return;
  if (editor.query) highlightSearch(editor);
  inputLayer.addEventListener('pointerdown', event => beginAnnotation(editor, event));
  updateControls(editor);
}

function trimHistory(editor) {
  while (editor.undo.length > 12 || editor.undo.reduce((size, item) => size + item.bytes.length, 0) > HISTORY_BYTES) editor.undo.shift();
  while (editor.redo.length > 12 || editor.redo.reduce((size, item) => size + item.bytes.length, 0) > HISTORY_BYTES) editor.redo.shift();
}
async function mutate(editor, change, nextPage = editor.current) {
  if (editor.busy || !editor.canEdit || editor.protected) return;
  setBusy(editor, true);
  try {
    const before = { bytes: editor.bytes, page: editor.current };
    const document = await editor.libs.writer.PDFDocument.load(editor.bytes, { updateMetadata: false });
    await change(document);
    const bytes = await document.save();
    if (bytes.length > MAX_BYTES) throw new Error('The edited PDF exceeds 32 MB. Undo or reduce the added content.');
    await loadDocument(editor, bytes, nextPage);
    editor.undo.push(before); editor.redo = []; trimHistory(editor);
    editor.bytes = bytes; editor.dirty = !equalBytes(bytes, editor.savedBytes); editor.tool = 'select';
    showMessage(editor, '');
  } finally { setBusy(editor, false); }
}
async function history(editor, undo) {
  const source = undo ? editor.undo : editor.redo, target = undo ? editor.redo : editor.undo;
  if (editor.busy || !source.length) return;
  setBusy(editor, true);
  try {
    const previous = source[source.length - 1];
    const current = { bytes: editor.bytes, page: editor.current };
    await loadDocument(editor, previous.bytes, previous.page);
    source.pop(); target.push(current); trimHistory(editor);
    editor.bytes = previous.bytes; editor.dirty = !equalBytes(editor.bytes, editor.savedBytes);
  } finally { setBusy(editor, false); }
}
function point(editor, event) {
  const bounds = editor.root.querySelector('.pdf-page-wrap').getBoundingClientRect();
  return [Math.max(0, Math.min(bounds.width, event.clientX - bounds.left)), Math.max(0, Math.min(bounds.height, event.clientY - bounds.top))];
}
function beginAnnotation(editor, event) {
  if (editor.busy || !editor.canEdit || editor.protected || editor.tool === 'select' || event.button !== 0) return;
  event.preventDefault();
  const start = point(editor, event);
  if (editor.tool === 'text') {
    const text = editor.root.querySelector('.pdf-new-text').value.replace(/\t/g, '    ');
    if (!text.trim()) { showMessage(editor, 'Enter the text to add, then click the page.'); return; }
    const size = Math.max(6, Math.min(96, Number(editor.root.querySelector('.pdf-font-size').value) || 14));
    const color = editor.root.querySelector('.pdf-font-color').value;
    const location = editor.viewport.convertToPdfPoint(...start);
    mutate(editor, async document => {
      document.registerFontkit(editor.libs.fontkit);
      const annotation = await annotationFont(editor, document);
      if (annotation.embedder.font.layout(text.replace(/[\r\n]/g, ' ')).glyphs.some(glyph => glyph.id === 0)) throw new Error('The annotation font does not contain some of these characters. Use supported text or convert to Writer.');
      const page = document.getPage(editor.current);
      const rgb = /^#[0-9a-f]{6}$/i.test(color) ? color.match(/[0-9a-f]{2}/gi).map(value => parseInt(value, 16) / 255) : [0, 0, 0];
      page.drawText(text, { x: location[0], y: location[1], size, font: annotation.font, color: editor.libs.writer.rgb(...rgb), rotate: editor.libs.writer.degrees(page.getRotation().angle % 360), lineHeight: size * 1.25 });
      await finishAnnotationFont(editor, document, annotation, text);
    }).catch(error => showMessage(editor, error.message, true));
    return;
  }
  const layer = event.currentTarget;
  const rectangle = document.createElement('div'); rectangle.className = 'pdf-drag-rectangle'; layer.append(rectangle);
  layer.setPointerCapture(event.pointerId);
  const moving = next => {
    const end = point(editor, next);
    Object.assign(rectangle.style, { left: `${Math.min(start[0], end[0])}px`, top: `${Math.min(start[1], end[1])}px`, width: `${Math.abs(end[0] - start[0])}px`, height: `${Math.abs(end[1] - start[1])}px` });
  };
  const finished = next => {
    layer.removeEventListener('pointermove', moving); layer.removeEventListener('pointerup', finished); layer.removeEventListener('pointercancel', cancelled); rectangle.remove();
    const end = point(editor, next);
    if (Math.abs(end[0] - start[0]) < 3 || Math.abs(end[1] - start[1]) < 3) return;
    const from = editor.viewport.convertToPdfPoint(...start), to = editor.viewport.convertToPdfPoint(...end);
    mutate(editor, async document => document.getPage(editor.current).drawRectangle({
      x: Math.min(from[0], to[0]), y: Math.min(from[1], to[1]), width: Math.abs(to[0] - from[0]), height: Math.abs(to[1] - from[1]),
      color: editor.libs.writer.rgb(1, .85, 0), opacity: .35, blendMode: editor.libs.writer.BlendMode.Multiply,
    })).catch(error => showMessage(editor, error.message, true));
  };
  const cancelled = () => { layer.removeEventListener('pointermove', moving); layer.removeEventListener('pointerup', finished); rectangle.remove(); };
  layer.addEventListener('pointermove', moving); layer.addEventListener('pointerup', finished); layer.addEventListener('pointercancel', cancelled, { once: true });
}

async function save(editor) {
  if (!editor.dirty) return true;
  if (!editor.canEdit || editor.protected || editor.busy) return false;
  setBusy(editor, true);
  let succeeded = false;
  try {
    const response = await api(editor, `/files/${editor.fileId}/pdf`, {
      method: 'PUT', headers: { 'Content-Type': 'application/pdf', 'If-Match': editor.etag }, body: editor.bytes,
    });
    const result = await response.json(); editor.etag = result.data.etag;
    succeeded = true;
  } catch (error) {
    // A lost response can follow a committed PUT. Confirm the actual bytes before
    // retrying with an old ETag or claiming that local changes were saved.
    if (!editor.destroyed && error.status !== 401 && error.status !== 403) {
      try {
        const current = await api(editor, `/files/${editor.fileId}/pdf`);
        if (equalBytes(await pdfResponseBytes(current), editor.bytes)) { editor.etag = current.headers.get('ETag'); succeeded = true; }
      } catch {}
    }
    if (!succeeded) showMessage(editor, error.status === 409
      ? 'This PDF changed on the server. Your edits are kept here. Download your copy before reloading the latest version.'
      : `Save could not be confirmed. Your edits are kept here. ${error.message}`, true);
  } finally {
    if (succeeded) { editor.savedBytes = editor.bytes; editor.dirty = false; showMessage(editor, 'PDF saved.'); }
    setBusy(editor, false);
  }
  return succeeded;
}
async function convert(editor) {
  if (editor.busy || editor.protected || !editor.pdf) return;
  if (editor.dirty && !await save(editor)) return;
  setBusy(editor, true);
  try {
    showMessage(editor, 'Converting text to an editable Writer document…');
    let ocr_pages;
    for (let index = 1; index <= editor.pdf.numPages; index++) {
      const page = await editor.pdf.getPage(index);
      const text = await page.getTextContent();
      if (!text.items.some(item => item.str?.trim())) {
        editor.ocrAbort = new AbortController();
        editor.root.querySelector('[data-action="cancel-ocr"]').hidden = false;
        const cancelled = () => editor.ocrAbort?.abort();
        editor.abort.signal.addEventListener('abort', cancelled, { once: true });
        try {
          showMessage(editor, 'Recognizing scanned pages locally…');
          const { recognizePdfPages } = await import('/assets/pdf-ocr.js');
          ocr_pages = await recognizePdfPages(editor.pdf, {
            signal: editor.ocrAbort.signal,
            onProgress: progress => showMessage(editor, typeof progress === 'string' ? progress : progress.message || `${progress.status || 'Recognizing text'} ${Math.round((progress.progress || 0) * 100)}%`),
          });
        } finally { editor.abort.signal.removeEventListener('abort', cancelled); }
        break;
      }
    }
    const response = await api(editor, `/files/${editor.fileId}/pdf/convert`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ expected_etag: editor.etag, name: editor.name.replace(/\.pdf$/i, ''), ...(ocr_pages ? { ocr_pages } : {}) }),
    });
    const result = await response.json();
    window.location.assign(`/writer/${result.data.document_id}`);
  } catch (error) {
    if (error.name === 'AbortError') showMessage(editor, 'Text recognition cancelled. Your PDF is unchanged.');
    else throw error;
  } finally {
    editor.ocrAbort = null;
    if (editor.root) editor.root.querySelector('[data-action="cancel-ocr"]').hidden = true;
    setBusy(editor, false);
  }
}
async function onClick(editor, event) {
  const pageButton = event.target.closest('.pdf-page-option');
  if (pageButton) { await navigate(editor, Number(pageButton.dataset.page)); return; }
  const button = event.target.closest('button');
  if (!button || button.disabled) return;
  if (button.dataset.tool) { editor.tool = button.dataset.tool; updateControls(editor); return; }
  switch (button.dataset.action) {
    case 'previous': await navigate(editor, editor.current - 1); break;
    case 'next': await navigate(editor, editor.current + 1); break;
    case 'fit': editor.fit = true; await render(editor); break;
    case 'save': await save(editor); break;
    case 'download': download(editor.bytes, editor.name); break;
    case 'original': {
      const response = await api(editor, `/files/${editor.fileId}/pdf?version=0`);
      download(await pdfResponseBytes(response), editor.name.replace(/\.pdf$/i, '') + '-original.pdf'); break;
    }
    case 'convert': await convert(editor); break;
    case 'cancel-ocr': editor.ocrAbort?.abort(); break;
    case 'undo': await history(editor, true); break;
    case 'redo': await history(editor, false); break;
    case 'rotate-left': case 'rotate-right': await mutate(editor, async document => {
      const page = document.getPage(editor.current);
      page.setRotation(editor.libs.writer.degrees((page.getRotation().angle + (button.dataset.action === 'rotate-right' ? 90 : 270)) % 360));
    }); break;
    case 'delete': await mutate(editor, async document => document.removePage(editor.current), Math.min(editor.current, editor.pdf.numPages - 2)); break;
    case 'earlier': case 'later': {
      const target = editor.current + (button.dataset.action === 'earlier' ? -1 : 1);
      await mutate(editor, async document => { const page = document.getPage(editor.current); document.removePage(editor.current); document.insertPage(target, page); }, target); break;
    }
    case 'previous-match': await nextMatch(editor, -1); break;
    case 'next-match': await nextMatch(editor, 1); break;
  }
}

async function find(editor) {
  if (!editor.pdf) return;
  const query = editor.root.querySelector('.pdf-find').value.trim().toLocaleLowerCase();
  const serial = ++editor.searchSerial, pdf = editor.pdf;
  editor.query = query; editor.matches = []; editor.matchIndex = -1;
  const status = editor.root.querySelector('.pdf-find-status');
  if (!query) { status.textContent = ''; highlightSearch(editor); return; }
  status.textContent = 'Searching…';
  let characters = 0;
  for (let index = 0; index < pdf.numPages; index++) {
    if (editor.destroyed || serial !== editor.searchSerial || editor.pdf !== pdf) return;
    const page = await pdf.getPage(index + 1);
    const content = await page.getTextContent();
    const text = content.items.map(item => item.str || '').join(' ');
    characters += text.length;
    if (characters > 16000000) throw new Error('PDF text exceeds the search limit. Search a smaller document.');
    if (text.toLocaleLowerCase().includes(query)) editor.matches.push(index);
  }
  if (serial !== editor.searchSerial) return;
  status.textContent = editor.matches.length ? `Found on ${editor.matches.length} page${editor.matches.length === 1 ? '' : 's'}` : 'No matches';
  if (editor.matches.length) { editor.matchIndex = 0; await navigate(editor, editor.matches[0]); }
  else highlightSearch(editor);
}
function highlightSearch(editor) {
  for (const span of editor.root.querySelectorAll('.textLayer span')) {
    span.classList.toggle('pdf-search-match', !!editor.query && span.textContent.toLocaleLowerCase().includes(editor.query));
  }
}
async function nextMatch(editor, direction) {
  if (!editor.matches.length) return;
  editor.matchIndex = (editor.matchIndex + direction + editor.matches.length) % editor.matches.length;
  await navigate(editor, editor.matches[editor.matchIndex]);
}

export function destroyPdfEditor(editor) {
  if (!editor || editor.destroyed) return;
  editor.destroyed = true; cancelAnimationFrame(editor.frame); clearTimeout(editor.resizeTimer);
  editor.abort.abort(); editor.renderTask?.cancel(); editor.textLayer?.cancel();
  editor.ocrAbort?.abort();
  editor.resize?.disconnect(); editor.thumbnails?.disconnect();
  editor.loading?.destroy().catch(() => {}); editor.pendingLoading?.destroy().catch(() => {}); editor.bytes = null; editor.savedBytes = null;
  editor.undo = []; editor.redo = []; editor.root = null;
}

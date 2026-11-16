// OCR remains in the browser. Documents and page images never leave the app
// for an external recognition service, and all worker/language assets are local.
const ROOT = '/assets/pdf-vendor/tesseract';
const MAX_TEXT_BYTES = 20 * 1024 * 1024;
const MAX_PAGE_TEXT_BYTES = 1024 * 1024;
const MAX_SCANNED_PAGES = 50;
const encoder = new TextEncoder();

function aborted() { return new DOMException('PDF conversion cancelled', 'AbortError'); }

function withAbort(promise, signal) {
  if (!signal) return promise;
  return new Promise((resolve, reject) => {
    const cancel = () => reject(aborted());
    signal.addEventListener('abort', cancel, { once: true });
    if (signal.aborted) cancel();
    Promise.resolve(promise).then(resolve, reject).finally(() => signal.removeEventListener('abort', cancel));
  });
}

async function pageText(page) {
  const content = await page.getTextContent();
  let text = '';
  let previousY;
  for (const item of content.items) {
    if (typeof item.str !== 'string') continue;
    const y = item.transform?.[5];
    if (text && previousY !== undefined && y !== undefined && Math.abs(y - previousY) > Math.max(2, (item.height || 10) / 2) && !text.endsWith('\n')) text += '\n';
    text += item.str;
    text += item.hasEOL ? '\n' : ' ';
    previousY = y;
  }
  return text.replace(/\0/g, '').trim();
}

/** Return one text string per PDF page, using OCR only for pages without text. */
export async function recognizePdfPages(pdf, { onProgress = () => {}, signal } = {}) {
  if (!Number.isInteger(pdf.numPages) || pdf.numPages < 1 || pdf.numPages > 500) throw new Error('PDF conversion supports at most 500 pages');
  let worker;
  let renderTask;
  let canvas;
  let currentPage = 1;
  let scanned = 0;
  let totalBytes = 0;
  const pages = [];
  const report = value => {
    if (!signal?.aborted) onProgress({ ...value, status: value.phase, message: `Page ${value.page} of ${value.total}: ${value.phase}` });
  };
  const cancel = () => { renderTask?.cancel(); if (worker) void worker.terminate(); };
  signal?.addEventListener('abort', cancel, { once: true });
  try {
    for (let index = 1; index <= pdf.numPages; index++) {
      if (signal?.aborted) throw aborted();
      currentPage = index;
      report({ page: index, total: pdf.numPages, progress: 0, phase: 'reading' });
      const page = await withAbort(pdf.getPage(index), signal);
      let text = await withAbort(pageText(page), signal);
      if (!text.trim()) {
        scanned++;
        if (scanned > MAX_SCANNED_PAGES) throw new Error('OCR supports at most 50 scanned pages per conversion; split this PDF into smaller files');
        if (!worker) {
          const module = await import(`${ROOT}/tesseract.esm.min.js`);
          const createWorker = module.createWorker || module.default?.createWorker;
          if (typeof createWorker !== 'function') throw new Error('The local OCR engine could not be initialized');
          const creating = createWorker(['eng', 'chi_sim'], 1, {
            workerPath: `${location.origin}${ROOT}/worker.min.js`,
            corePath: `${location.origin}${ROOT}/core`,
            langPath: `${location.origin}${ROOT}/langs`,
            workerBlobURL: false,
            cacheMethod: 'none',
            gzip: true,
            logger: message => report({ page: currentPage, total: pdf.numPages, progress: message.progress || 0, phase: message.status || 'recognizing' }),
          }).then(async created => {
            if (signal?.aborted) { await created.terminate(); throw aborted(); }
            return created;
          });
          worker = await withAbort(creating, signal);
          if (signal?.aborted) throw aborted();
          await worker.setParameters({ preserve_interword_spaces: '1' });
        }
        const base = page.getViewport({ scale: 1 });
        if (!Number.isFinite(base.width) || !Number.isFinite(base.height) || base.width <= 0 || base.height <= 0) throw new Error('PDF page dimensions cannot be recognized safely');
        const scale = Math.min(3, 3200 / Math.max(base.width, base.height), Math.sqrt(12_000_000 / (base.width * base.height)));
        const viewport = page.getViewport({ scale });
        canvas = document.createElement('canvas');
        canvas.width = Math.ceil(viewport.width);
        canvas.height = Math.ceil(viewport.height);
        renderTask = page.render({ canvasContext: canvas.getContext('2d', { willReadFrequently: true }), viewport, background: 'rgb(255,255,255)' });
        await renderTask.promise;
        renderTask = undefined;
        if (signal?.aborted) throw aborted();
        const result = await withAbort(worker.recognize(canvas, {}, { text: true }), signal);
        text = (result.data.text || '').replace(/\0/g, '').trim();
        canvas.width = canvas.height = 0;
        canvas = undefined;
      }
      if (signal?.aborted) throw aborted();
      const bytes = encoder.encode(text).length;
      totalBytes += bytes;
      if (bytes > MAX_PAGE_TEXT_BYTES || totalBytes > MAX_TEXT_BYTES) throw new Error('Recognized text exceeds the document size limit');
      pages.push(text);
      report({ page: index, total: pdf.numPages, progress: 1, phase: 'page complete' });
    }
    if (!pages.some(text => text.trim())) throw new Error('No readable text was found. The original PDF remains unchanged.');
    return pages;
  } catch (error) {
    if (signal?.aborted) throw aborted();
    throw error;
  } finally {
    signal?.removeEventListener('abort', cancel);
    renderTask?.cancel();
    if (canvas) canvas.width = canvas.height = 0;
    if (worker) await worker.terminate().catch(() => {});
  }
}

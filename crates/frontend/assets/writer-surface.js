// Writer owns this contenteditable subtree. Imported text is always assigned
// through textContent; neither imported HTML nor document URLs become markup.
const clone = value => value === undefined ? undefined : JSON.parse(JSON.stringify(value));
const finite = (value, fallback = 0, min = -10000, max = 10000) =>
  value != null && Number.isFinite(Number(value)) ? Math.min(max, Math.max(min, Number(value))) : fallback;
const key = value => JSON.stringify(value, (_name, item) => item && typeof item === 'object' && !Array.isArray(item)
  ? Object.keys(item).sort().reduce((out, name) => { out[name] = item[name]; return out; }, {}) : item);
const freshId = () => crypto.randomUUID();
const isElement = node => node?.nodeType === Node.ELEMENT_NODE;
const safeLink = value => typeof value === 'string' && /^(https?:|mailto:)/i.test(value) && !/[\u0000-\u0020]/.test(value);
const plainRun = text => ({text, bold:false, italic:false, underline:false, strikethrough:false,
  font_family:null, font_size:null, color:null, background_color:null, link:null, superscript:false, subscript:false});
const paragraph = () => ({type:'Paragraph', id:freshId(), runs:[], alignment:'Left', indent_level:0,
  spacing:{before:0, after:8, line_height:1.15}});
const renderedBlocks = new WeakMap();
const generatedBlocks = new WeakMap();

function imageSource(value) {
  if (typeof value !== 'string') return null;
  if (/^data:image\/(png|jpeg|gif|webp);base64,[a-z0-9+/=\s]+$/i.test(value)) return value;
  try {
    const url = new URL(value, location.href);
    if (url.origin === location.origin && /^\/(api\/v1\/files\/|assets\/|icons\/)/.test(url.pathname)) return url.href;
  } catch (_) { /* show the supplied alternative text */ }
  return null;
}
function applyRunStyle(element, run) {
  element.style.fontWeight = run.bold ? '700' : '400';
  element.style.fontStyle = run.italic ? 'italic' : 'normal';
  element.style.textDecoration = [run.underline && 'underline', run.strikethrough && 'line-through'].filter(Boolean).join(' ') || 'none';
  if (typeof run.font_family === 'string' && run.font_family.trim()) element.style.fontFamily = run.font_family;
  if (run.font_size != null) element.style.fontSize = `${finite(run.font_size, 12, 1, 400)}pt`;
  element.style.color = typeof run.color === 'string' && CSS.supports('color', run.color) ? run.color : '#000000';
  if (typeof run.background_color === 'string' && CSS.supports('color', run.background_color)) element.style.backgroundColor = run.background_color;
  if (run.superscript) element.style.verticalAlign = 'super';
  else if (run.subscript) element.style.verticalAlign = 'sub';
}
function appendRuns(parent, runs) {
  for (const [index, run] of (Array.isArray(runs) ? runs : []).entries()) {
    const element = document.createElement(safeLink(run.link) ? 'a' : 'span');
    element.className = 'writer-surface-run';
    element.dataset.writerRun = String(index);
    if (safeLink(run.link)) element.href = run.link;
    applyRunStyle(element, run);
    element.textContent = String(run.text ?? '');
    parent.append(element);
  }
  if (!parent.childNodes.length) parent.append(document.createElement('br'));
}
function formatMap(model) {
  const formats = model?.professional?.paragraph_formats;
  const result = new Map();
  if (Array.isArray(formats)) {
    for (const item of formats) if (item?.block_id) result.set(String(item.block_id), item);
  } else if (formats && typeof formats === 'object') {
    for (const [id, item] of Object.entries(formats)) result.set(id, item);
  }
  return result;
}
function blockStyle(element, block, formats) {
  const format = formats.get(String(block.id)) || {};
  const spacing = block.spacing || {};
  element.style.marginTop = `${finite(spacing.before, 0, 0, 1000)}pt`;
  element.style.marginBottom = `${finite(spacing.after, 8, 0, 1000)}pt`;
  element.style.lineHeight = format.line_height_pt != null
    ? `${finite(format.line_height_pt, 14, 1, 1000)}pt` : String(finite(spacing.line_height, 1.15, 0.5, 10));
  const alignment = String(format.alignment || block.alignment || 'Left').toLowerCase();
  element.style.textAlign = ['left','center','right','justify'].includes(alignment) ? alignment : 'left';
  element.style.marginLeft = `${finite(format.left_indent_pt, finite(block.indent_level) * 18)}pt`;
  element.style.marginRight = `${finite(format.right_indent_pt)}pt`;
  if (format.first_line_indent_chars != null) element.style.textIndent = `${finite(format.first_line_indent_chars)}em`;
  else element.style.textIndent = `${finite(format.first_line_indent_pt)}pt`;
  if (format.numbering_label) {
    const label = document.createElement('span');
    label.dataset.writerNumbering = 'true';
    label.contentEditable = 'false';
    label.className = 'writer-numbering-label';
    label.textContent = `${format.numbering_label} `;
    element.append(label);
  }
}
function renderListItems(parent, items) {
  for (const item of items || []) {
    const li = document.createElement('li');
    li.className = 'writer-surface-list-item';
    const text = document.createElement('div');
    text.dataset.writerListText = 'true';
    appendRuns(text, item.runs);
    li.append(text);
    if (item.children?.length) {
      const nested = document.createElement(parent.tagName.toLowerCase());
      renderListItems(nested, item.children);
      li.append(nested);
    }
    parent.append(li);
  }
}
function renderBlock(block, formats, depth = 0, index = null) {
  if (depth > 32) return document.createTextNode('Document nesting limit reached');
  let element;
  switch (block.type) {
    case 'Paragraph': case 'Heading': {
      element = document.createElement(block.type === 'Heading' ? `h${finite(block.level, 1, 1, 6)}` : 'p');
      blockStyle(element, block, formats);
      appendRuns(element, block.runs);
      break;
    }
    case 'List':
      element = document.createElement(block.ordered ? 'ol' : 'ul');
      renderListItems(element, block.items);
      break;
    case 'Table': {
      element = document.createElement('table');
      element.className = 'writer-surface-table';
      const tbody = document.createElement('tbody');
      for (const row of block.rows || []) {
        const tr = document.createElement('tr');
        for (const cell of row.cells || []) {
          const td = document.createElement('td');
          td.colSpan = finite(cell.col_span, 1, 1, 100);
          td.rowSpan = finite(cell.row_span, 1, 1, 100);
          if (cell.background_color && CSS.supports('color', cell.background_color)) td.style.backgroundColor = cell.background_color;
          for (const [index, nested] of (cell.blocks || []).entries()) td.append(renderBlock(nested, formats, depth + 1, index));
          if (!td.childNodes.length) td.append(document.createElement('p'));
          tr.append(td);
        }
        tbody.append(tr);
      }
      element.append(tbody);
      break;
    }
    case 'Image': {
      element = document.createElement('figure');
      element.contentEditable = 'false';
      element.style.textAlign = String(block.alignment || 'Left').toLowerCase();
      const source = imageSource(block.url);
      if (source) {
        const image = document.createElement('img');
        image.src = source;
        image.alt = String(block.alt_text || 'Document image');
        if (block.width != null) image.style.width = `${finite(block.width, 100, 1, 10000)}px`;
        if (block.height != null) image.style.height = `${finite(block.height, 100, 1, 10000)}px`;
        image.style.maxWidth = '100%';
        element.append(image);
      } else {
        element.className = 'writer-image-placeholder';
        element.textContent = String(block.alt_text || 'Image not loaded');
      }
      break;
    }
    case 'CodeBlock':
      element = document.createElement('pre');
      element.textContent = String(block.code || '');
      break;
    case 'Blockquote':
      element = document.createElement('blockquote');
      for (const [index, nested] of (block.blocks || []).entries()) element.append(renderBlock(nested, formats, depth + 1, index));
      break;
    case 'HorizontalRule': element = document.createElement('hr'); element.contentEditable = 'false'; break;
    case 'PageBreak':
      element = document.createElement('div');
      element.className = 'writer-page-break';
      element.contentEditable = 'false';
      element.setAttribute('aria-label', 'Page break');
      break;
    default:
      element = document.createElement('div');
      element.contentEditable = 'false';
      element.textContent = `Unsupported block: ${String(block.type || '')}`;
  }
  element.classList.add('writer-surface-block');
  element.dataset.writerBlockId = String(block.id || '');
  if (index != null) element.dataset.writerBlockIndex = String(index);
  element.dataset.writerType = block.type || 'Paragraph';
  renderedBlocks.set(element, {model:clone(block), html:element.outerHTML});
  return element;
}
function applyInlineStyle(run, element, original = null) {
  const tag = element.tagName;
  if (tag === 'B' || tag === 'STRONG') run.bold = true;
  if (tag === 'I' || tag === 'EM') run.italic = true;
  if (tag === 'U') run.underline = true;
  if (tag === 'S' || tag === 'STRIKE' || tag === 'DEL') run.strikethrough = true;
  if (tag === 'SUP') { run.superscript = true; run.subscript = false; }
  if (tag === 'SUB') { run.subscript = true; run.superscript = false; }
  if (tag === 'A') run.link = safeLink(element.getAttribute('href')) ? element.getAttribute('href') : null;
  if (element.dataset.writerUnlink) run.link = null;
  if (tag === 'FONT' && element.getAttribute('face')) run.font_family = element.getAttribute('face');
  const style = element.style;
  let expected = null;
  if (original) {
    const normalized = document.createElement('span');
    applyRunStyle(normalized, original);
    expected = normalized.style;
  }
  const changed = name => !expected || style[name] !== expected[name];
  if (style.fontWeight) run.bold = style.fontWeight === 'bold' || Number(style.fontWeight) >= 600;
  if (style.fontStyle) run.italic = style.fontStyle === 'italic';
  if (style.fontFamily && changed('fontFamily')) run.font_family = style.fontFamily;
  if (style.fontSize && changed('fontSize')) {
    const size = parseFloat(style.fontSize);
    if (Number.isFinite(size)) {
      const points = style.fontSize.endsWith('pt') ? size : size * 0.75;
      // Chrome can rewrite 22pt to 29.3333px during editing. This is a CSSOM
      // representation change, not an intentional document size edit.
      if (run.font_size == null || Math.abs(points - Number(run.font_size)) >= 0.01) run.font_size = points;
    }
  }
  if (style.color && changed('color')) run.color = style.color;
  if (style.backgroundColor && changed('backgroundColor')) run.background_color = style.backgroundColor;
  if (style.textDecorationLine || style.textDecoration) {
    const decoration = style.textDecorationLine || style.textDecoration;
    run.underline = decoration.includes('underline');
    run.strikethrough = decoration.includes('line-through');
  }
  if (style.verticalAlign) { run.superscript = style.verticalAlign === 'super'; run.subscript = style.verticalAlign === 'sub'; }
  return run;
}
function readRuns(element, originals) {
  const result = [];
  const origins = [];
  const walk = (node, inherited, depth = 0, origin = null) => {
    if (depth > 64) return;
    if (node.nodeType === Node.TEXT_NODE) {
      if (node.nodeValue) {
        const run = clone(inherited);
        run.text = node.nodeValue;
        const last = result[result.length - 1];
        if (last && origins[origins.length - 1] === origin && key({...last, text:''}) === key({...run, text:''})) last.text += run.text;
        else { result.push(run); origins.push(origin); }
      }
      return;
    }
    if (!isElement(node) || node.dataset.writerNumbering || node.contentEditable === 'false') return;
    let run = clone(inherited);
    const index = node.dataset.writerRun;
    const original = index !== undefined ? originals?.[Number(index)] : null;
    if (original) run = clone(original);
    if (original) origin = Number(index);
    run.text = '';
    applyInlineStyle(run, node, original);
    if (node.tagName === 'BR') { result.push({...run, text:'\n'}); origins.push(origin); return; }
    for (const child of node.childNodes) walk(child, run, depth + 1, origin);
  };
  const base = clone(originals?.[0] || plainRun(''));
  for (const child of element.childNodes) walk(child, base);
  // An empty paragraph's placeholder BR is not document text.
  if (result.length === 1 && result[0].text === '\n' && element.textContent === '') result[0].text = '';
  return result.length ? result : [{...base, text:''}];
}
function readListItems(element, originals) {
  return Array.from(element.children).filter(child => child.tagName === 'LI').map((li, index) => {
    const item = clone(originals?.[index] || {runs:[], children:[]});
    const text = Array.from(li.children).find(child => child.dataset.writerListText);
    item.runs = readRuns(text || li, item.runs);
    const nested = Array.from(li.children).find(child => child.tagName === 'UL' || child.tagName === 'OL');
    item.children = nested ? readListItems(nested, item.children) : [];
    return item;
  });
}
function readBlocks(container, originals, seen = new Set(), depth = 0, formatCopies = []) {
  if (depth > 32) return clone(originals || []);
  const byId = new Map((originals || []).filter(item => item.id).map(item => [String(item.id), item]));
  const result = [];
  for (const node of container.childNodes) {
    if (node.nodeType === Node.TEXT_NODE) {
      if (node.nodeValue) result.push({...paragraph(), runs:[plainRun(node.nodeValue)]});
      continue;
    }
    if (!isElement(node) || node.dataset.writerNumbering) continue;
    const id = node.dataset.writerBlockId;
    const original = generatedBlocks.get(node) || byId.get(id) || originals?.[Number(node.dataset.writerBlockIndex)];
    const rendered = renderedBlocks.get(node);
    if (rendered && node.outerHTML === rendered.html && (!rendered.model.id || !seen.has(rendered.model.id))) {
      if (rendered.model.id) seen.add(rendered.model.id);
      result.push(clone(rendered.model));
      continue;
    }
    const block = clone(original || paragraph());
    const duplicatesId = block.id && seen.has(block.id);
    if (duplicatesId || !original) {
      const sourceId = duplicatesId ? block.id : node.previousElementSibling?.dataset.writerBlockId;
      if (duplicatesId) block.id = freshId();
      // Enter may clone both the original identity and its noneditable number
      // label. A new paragraph gets its own stable identity and layout record,
      // while the original paragraph keeps its original numbering label.
      node.dataset.writerBlockId = block.id;
      for (const label of node.querySelectorAll('[data-writer-numbering]')) label.remove();
      generatedBlocks.set(node, clone(block));
      if (sourceId) formatCopies.push({sourceId:String(sourceId), targetId:String(block.id)});
    }
    if (block.id) seen.add(block.id);
    if (original && node.contentEditable === 'false') {
      // Untouched images, page breaks and unknown block types are opaque.
    } else if (node.tagName === 'TABLE' && block.type === 'Table') {
      const rows = Array.from(node.rows);
      block.rows = rows.map((row, r) => {
        const source = clone(block.rows?.[r] || {cells:[]});
        source.cells = Array.from(row.cells).map((cell, c) => {
          const value = clone(source.cells?.[c] || {blocks:[], col_span:1, row_span:1, background_color:null});
          value.blocks = readBlocks(cell, value.blocks, seen, depth + 1, formatCopies);
          return value;
        });
        return source;
      });
    } else if ((node.tagName === 'OL' || node.tagName === 'UL') && block.type === 'List') {
      block.ordered = node.tagName === 'OL';
      block.items = readListItems(node, block.items);
    } else if (block.type === 'Image' || block.type === 'PageBreak' || block.type === 'HorizontalRule') {
      // Non-editable structures retain their canonical data and unknown fields.
    } else if (block.type === 'CodeBlock') block.code = node.textContent || '';
    else if (block.type === 'Blockquote' && node.dataset.writerType === 'Blockquote') block.blocks = readBlocks(node, block.blocks, seen, depth + 1, formatCopies);
    else {
      block.type = /^H[1-6]$/.test(node.tagName) ? 'Heading' : 'Paragraph';
      if (block.type === 'Heading') block.level = Number(node.tagName.slice(1));
      else {
        block.alignment ||= 'Left';
        block.indent_level ||= 0;
        block.spacing ||= {before:0, after:8, line_height:1.15};
      }
      block.runs = readRuns(node, original?.runs);
    }
    result.push(block);
  }
  return result;
}
function selectionOffsets(root) {
  const selection = document.getSelection();
  if (!selection?.rangeCount || !root.contains(selection.anchorNode) || !root.contains(selection.focusNode)) return null;
  const offset = (node, position) => {
    const range = document.createRange(); range.selectNodeContents(root); range.setEnd(node, position);
    return range.toString().length;
  };
  return [offset(selection.anchorNode, selection.anchorOffset), offset(selection.focusNode, selection.focusOffset)];
}
function selectionCoversText(root) {
  const offsets = selectionOffsets(root);
  const selection = document.getSelection();
  return !!offsets && !selection?.isCollapsed && Math.min(...offsets) === 0 &&
    Math.max(...offsets) === root.textContent.length;
}
function restoreOffsets(root, offsets) {
  if (!offsets) return;
  const locate = position => {
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    let remaining = position; let last = root;
    while (walker.nextNode()) {
      last = walker.currentNode;
      if (remaining <= last.length) return [last, remaining];
      remaining -= last.length;
    }
    return [last, last === root ? 0 : last.length];
  };
  const [anchor, a] = locate(offsets[0]); const [focus, f] = locate(offsets[1]);
  try { document.getSelection()?.setBaseAndExtent(anchor, a, focus, f); } catch (_) { /* changed structure */ }
}

export function createWriterSurface(selector, changed) {
  const surface = {selector, changed, root:null, model:null, signature:'', enabled:false, destroyed:false,
    frame:0, selection:null, composing:false, wholeReplacement:false, pendingPlainDOM:null};
  surface.remember = () => {
    const selection = document.getSelection();
    if (surface.root && selection?.rangeCount && surface.root.contains(selection.anchorNode) && surface.root.contains(selection.focusNode)) {
      surface.selection = selection.getRangeAt(0).cloneRange();
    }
  };
  surface.emit = () => {
    if (!surface.root || !surface.model || !surface.enabled || surface.composing) return;
    const dom = surface.root.outerHTML;
    // execCommand can synchronously dispatch input before the paste handler's
    // fallback emit; compositionend can likewise precede a final input. Until
    // the controlled source acknowledges a whole-text replacement, identical
    // DOM must not be reinterpreted as deletion of opaque native blocks.
    if (surface.pendingPlainDOM === dom) return;
    if (surface.pendingPlainDOM != null) surface.wholeReplacement = true;
    if (surface.wholeReplacement || !surface.root.querySelector('[data-writer-block-id]')) {
      surface.wholeReplacement = false;
      surface.pendingPlainDOM = dom;
      // Force the next controlled update to repair any opaque blocks removed
      // by a whole-text replacement, even when the replacement text is equal.
      surface.signature = '';
      surface.changed(JSON.stringify({__writer_plain_text:surface.root.innerText || ''}));
      return;
    }
    const next = clone(surface.model);
    const formatCopies = [];
    // data-writer-run indices refer to the last DOM render, not a newly split
    // canonical run list emitted by an earlier local keystroke or format.
    next.body = readBlocks(surface.root, surface.renderModel?.body || surface.model.body, new Set(), 0, formatCopies);
    const formats = next.professional?.paragraph_formats;
    if (Array.isArray(formats)) {
      for (const copy of formatCopies) {
        const source = formats.find(format => String(format.block_id) === copy.sourceId);
        if (source && !formats.some(format => String(format.block_id) === copy.targetId)) {
          formats.push({...clone(source), block_id:copy.targetId, numbering_label:null});
        }
      }
    } else if (formats && typeof formats === 'object') {
      for (const copy of formatCopies) if (formats[copy.sourceId] && !formats[copy.targetId]) {
        formats[copy.targetId] = {...clone(formats[copy.sourceId]), block_id:copy.targetId, numbering_label:null};
      }
    }
    const signature = key(next);
    if (signature !== surface.signature) {
      surface.model = next; surface.signature = signature;
      surface.changed(JSON.stringify(next));
    }
    surface.remember();
  };
  surface.beforeInput = event => {
    if (!surface.enabled || !surface.root) return;
    const replacesText = /^(insert|delete)/.test(event.inputType || '');
    const entireText = selectionCoversText(surface.root);
    // A local caret edit keeps the structured parser. Full selection + text
    // insertion/deletion uses the native model's lossless plain-text updater.
    // Keep that intent throughout IME composition until its final input emits.
    if (!surface.composing || !surface.wholeReplacement) surface.wholeReplacement = replacesText && entireText;
  };
  surface.paste = event => {
    if (!surface.enabled) return;
    event.preventDefault();
    const text = event.clipboardData?.getData('text/plain');
    if (text != null) {
      // execCommand may dispatch input without beforeinput. Capture the full
      // selection while it still exists, before the command replaces its DOM.
      if (surface.root && selectionCoversText(surface.root)) surface.wholeReplacement = true;
      document.execCommand('insertText', false, text);
      surface.emit();
    }
  };
  surface.startComposition = () => { surface.composing = true; };
  surface.endComposition = () => { surface.composing = false; surface.emit(); };
  surface.drop = event => { event.preventDefault(); };
  surface.mount = () => {
    if (surface.destroyed) return;
    surface.root = document.querySelector(selector);
    if (!surface.root) { surface.frame = requestAnimationFrame(surface.mount); return; }
    surface.root.setAttribute('role', 'textbox');
    surface.root.setAttribute('aria-label', 'Document body');
    surface.root.setAttribute('aria-multiline', 'true');
    surface.root.spellcheck = false;
    surface.root.addEventListener('beforeinput', surface.beforeInput);
    surface.root.addEventListener('input', surface.emit);
    surface.root.addEventListener('paste', surface.paste);
    surface.root.addEventListener('drop', surface.drop);
    surface.root.addEventListener('compositionstart', surface.startComposition);
    surface.root.addEventListener('compositionend', surface.endComposition);
    document.addEventListener('selectionchange', surface.remember);
    surface.render();
  };
  surface.render = () => {
    if (!surface.root || !surface.model || surface.destroyed) return;
    const offsets = selectionOffsets(surface.root);
    const formats = formatMap(surface.model);
    surface.renderModel = clone(surface.model);
    const fragment = document.createDocumentFragment();
    for (const [index, block] of (surface.model.body || []).entries()) fragment.append(renderBlock(block, formats, 0, index));
    if (!fragment.childNodes.length) fragment.append(renderBlock(paragraph(), formats));
    surface.root.replaceChildren(fragment);
    surface.root.contentEditable = String(surface.enabled);
    surface.root.setAttribute('aria-disabled', String(!surface.enabled));
    const page = surface.root.closest('.writer-page');
    const settings = surface.model.page_settings || {};
    if (page) for (const [name, value] of Object.entries({
      'page-width':finite(settings.width_mm, 210, 30, 2000), 'page-height':finite(settings.height_mm, 297, 30, 2000),
      'margin-top':finite(settings.margin_top, 25.4, 0, 500), 'margin-right':finite(settings.margin_right, 25.4, 0, 500),
      'margin-bottom':finite(settings.margin_bottom, 25.4, 0, 500), 'margin-left':finite(settings.margin_left, 25.4, 0, 500),
    })) page.style.setProperty(`--writer-${name}`, `${value}mm`);
    restoreOffsets(surface.root, offsets);
    surface.remember();
  };
  surface.frame = requestAnimationFrame(surface.mount);
  return surface;
}
export function updateWriterSurface(surface, serialized, enabled) {
  if (surface.destroyed) return;
  surface.pendingPlainDOM = null;
  const model = JSON.parse(serialized);
  const signature = key(model);
  const changed = signature !== surface.signature;
  surface.enabled = !!enabled;
  surface.model = model;
  surface.signature = signature;
  if (changed && !surface.composing) surface.render();
  if (surface.root) {
    surface.root.contentEditable = String(surface.enabled);
    surface.root.setAttribute('aria-disabled', String(!surface.enabled));
  }
}
export function formatWriterSurface(surface, serialized) {
  if (!surface.root || !surface.enabled) throw new Error('Document is not ready for editing');
  const format = JSON.parse(serialized);
  const previous = surface.selection?.cloneRange();
  surface.root.focus();
  if (previous && surface.root.contains(previous.commonAncestorContainer)) {
    const selection = document.getSelection(); selection.removeAllRanges(); selection.addRange(previous);
  }
  const selection = document.getSelection();
  const selected = selection?.rangeCount ? selection.getRangeAt(0).cloneRange() : null;
  // Insert an innermost style span so an existing data-run's explicit normal
  // weight cannot swallow a toolbar strong/em wrapper outside that run.
  if (selected && !selected.collapsed && ['bold','italic','code','link'].includes(format.kind)) {
    if (format.kind === 'link' && format.value != null && !safeLink(format.value)) throw new Error('Unsupported link');
    const offsets = selectionOffsets(surface.root);
    const walker = document.createTreeWalker(surface.root, NodeFilter.SHOW_TEXT);
    const pieces = [];
    while (walker.nextNode()) {
      const text = walker.currentNode;
      if (!selected.intersectsNode(text) || text.parentElement?.closest('[contenteditable="false"]')) continue;
      const from = text === selected.startContainer ? selected.startOffset : 0;
      const to = text === selected.endContainer ? selected.endOffset : text.length;
      if (from < to) pieces.push([text, from, to]);
    }
    for (const [text, from, to] of pieces) {
      text.splitText(to);
      const middle = text.splitText(from);
      const mark = document.createElement(format.kind === 'link' && format.value != null ? 'a' : 'span');
      if (format.kind === 'bold') mark.style.fontWeight = format.value ? '700' : '400';
      else if (format.kind === 'italic') mark.style.fontStyle = format.value ? 'italic' : 'normal';
      else if (format.kind === 'code') mark.style.fontFamily = format.value ? 'monospace' : 'serif';
      else if (format.value != null) mark.href = format.value;
      else mark.dataset.writerUnlink = 'true';
      middle.replaceWith(mark); mark.append(middle);
    }
    restoreOffsets(surface.root, offsets);
    surface.emit();
    return;
  }
  if (format.kind === 'bold' || format.kind === 'italic') {
    if (document.queryCommandState(format.kind) !== !!format.value) document.execCommand(format.kind, false);
  } else if (format.kind === 'code') document.execCommand('fontName', false, format.value ? 'monospace' : 'serif');
  else if (format.kind === 'block' && /^(p|h[1-6]|blockquote)$/.test(format.value)) document.execCommand('formatBlock', false, format.value);
  else if (format.kind === 'link') {
    if (format.value == null) document.execCommand('unlink', false);
    else if (safeLink(format.value)) document.execCommand('createLink', false, format.value);
    else throw new Error('Unsupported link');
  }
  surface.emit();
}
export function destroyWriterSurface(surface) {
  surface.destroyed = true;
  cancelAnimationFrame(surface.frame);
  document.removeEventListener('selectionchange', surface.remember);
  if (surface.root) {
    surface.root.removeEventListener('beforeinput', surface.beforeInput);
    surface.root.removeEventListener('input', surface.emit);
    surface.root.removeEventListener('paste', surface.paste);
    surface.root.removeEventListener('drop', surface.drop);
    surface.root.removeEventListener('compositionstart', surface.startComposition);
    surface.root.removeEventListener('compositionend', surface.endComposition);
  }
  surface.changed = () => {};
  surface.selection = null;
  surface.root = null;
}

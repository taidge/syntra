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
const lineBoxes = new WeakMap();

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
  if (typeof run.font_family === 'string' && run.font_family.trim()) element.style.fontFamily = `${run.font_family}, "Microsoft YaHei", "Noto Sans CJK SC", sans-serif`;
  if (run.font_size != null) element.style.fontSize = `${finite(run.font_size, 12, 1, 400)}pt`;
  element.style.color = typeof run.color === 'string' && CSS.supports('color', run.color) ? run.color : '#000000';
  if (typeof run.background_color === 'string' && CSS.supports('color', run.background_color)) element.style.backgroundColor = run.background_color;
  if (run.superscript) element.style.verticalAlign = 'super';
  else if (run.subscript) element.style.verticalAlign = 'sub';
}
function styleLineRun(element, run, line, metrics) {
  if (!line || run.superscript || run.subscript) return;
  const family = String(run.font_family || line.family), key = family.trim().replace(/^['"]|['"]$/g,'').toLowerCase();
  const metric = metrics?.[key] || metrics?.[family];
  if (!metric) return;
  const size = finite(run.font_size,line.size,1,1000);
  const browserBaseline = metric.ascent*size+(line.line-(metric.ascent+metric.descent)*size)/2;
  element.style.verticalAlign = 'top';
  element.style.position = 'relative';
  element.style.top = `${finite(line.baseline-browserBaseline,0,-8000,8000)}pt`;
  element.style.lineHeight = 'inherit';
}
function appendRuns(parent, runs, metrics = {}) {
  for (const [index, run] of (Array.isArray(runs) ? runs : []).entries()) {
    const element = document.createElement(safeLink(run.link) ? 'a' : 'span');
    element.className = 'writer-surface-run';
    element.dataset.writerRun = String(index);
    if (safeLink(run.link)) element.href = run.link;
    applyRunStyle(element, run);
    styleLineRun(element,run,lineBoxes.get(parent),metrics);
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
function officeLineBox(runs, spacing, format, metrics) {
  if (format.line_height_at_least || !metrics || !Object.keys(metrics).length) return null;
  let selected = null, descent = 0;
  for (const run of runs || []) {
    if (!run.text) continue;
    if (!run.font_family) return null;
    const family = String(run.font_family), key = family.trim().replace(/^['"]|['"]$/g,'').toLowerCase();
    const metric = metrics[key] || metrics[family];
    if (!metric) return null;
    if (![metric.ascent,metric.descent,metric.line_gap].every(value => Number.isFinite(value) && value >= 0 && value <= 4)) return null;
    if (run.font_size != null && !Number.isFinite(Number(run.font_size))) return null;
    const size = finite(run.font_size,12,1,1000);
    const rawAscent = Math.round(metric.ascent*size*20), rawDescent = Math.round(metric.descent*size*20), gap = Math.round(metric.line_gap*size*20);
    const height = (metric.cjk ? Math.floor((rawAscent+rawDescent)*127/100) : rawAscent+rawDescent)+gap;
    const ascent = (metric.cjk ? Math.floor(rawAscent*127/100) : rawAscent)+gap;
    descent = Math.max(descent,height-ascent);
    if (!selected || ascent > selected.ascent) selected = {ascent,family,size};
  }
  if (!selected) return null;
  spacing = format.spacing || spacing || {};
  const exact = Number(format.line_height_pt);
  const line = format.line_height_pt != null && Number.isFinite(exact) && exact > 0 ? finite(exact,14,1,1000)
    : Math.min(1000, Math.max(20,Math.floor((selected.ascent+descent)*Math.round(finite(spacing.line_height,1.15,.5,10)*240)/240))/20);
  const baseline = Math.min(line,Math.max(0,format.line_height_pt != null && Number.isFinite(exact) && exact > 0 ? selected.ascent/20 : (selected.ascent+line*20-selected.ascent-descent)/20));
  return {...selected,line,baseline};
}
function blockStyle(element, block, formats, metrics = {}) {
  const format = formats.get(String(block.id)) || {};
  const spacing = format.spacing || block.spacing || {};
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
  const lineBox = officeLineBox(block.runs,spacing,format,metrics);
  lineBoxes.set(element,lineBox);
  if (lineBox) {
    element.style.fontFamily = lineBox.family;
    element.style.fontSize = `${lineBox.size}pt`;
    element.style.lineHeight = `${lineBox.line}pt`;
  }
  if (format.numbering_label) {
    const label = document.createElement('span');
    label.dataset.writerNumbering = 'true';
    label.contentEditable = 'false';
    label.className = 'writer-numbering-label';
    label.textContent = `${format.numbering_label} `;
    if (lineBox) styleLineRun(label,{font_family:lineBox.family,font_size:lineBox.size},lineBox,metrics);
    element.append(label);
  }
}
function renderListItems(parent, items, metrics) {
  const imported = (items || []).some(item => item.layout);
  if (imported) { parent.style.paddingLeft = '0'; parent.style.margin = '0'; }
  for (const [index, item] of (items || []).entries()) {
    const li = document.createElement('li');
    li.className = 'writer-surface-list-item';
    const text = document.createElement('div');
    text.dataset.writerListText = 'true';
    if (item.layout) {
      li.style.listStyle = 'none';
      const format = item.layout.format || {};
      const runs = item.layout.label_font_family ? [...(item.runs || []),{text:format.numbering_label,font_family:item.layout.label_font_family,font_size:item.layout.label_font_size}] : item.runs;
      blockStyle(text, {id:format.block_id, spacing:item.layout.spacing,runs}, new Map([[String(format.block_id),format]]),metrics);
      if (!text.style.fontSize && item.runs?.[0]?.font_size != null) text.style.fontSize = `${finite(item.runs[0].font_size,12,1,400)}pt`;
      if (format.contextual_spacing && items[index + 1]?.layout?.format?.contextual_spacing) text.style.marginBottom = '0pt';
      const marker = text.querySelector('[data-writer-numbering]');
      if (marker && item.layout.label_font_family) marker.style.fontFamily = item.layout.label_font_family;
      if (marker && item.layout.label_font_family) {
        if (item.layout.label_font_size != null) marker.style.fontSize = `${finite(item.layout.label_font_size,12,1,1000)}pt`;
        styleLineRun(marker,{font_family:item.layout.label_font_family,font_size:item.layout.label_font_size},lineBoxes.get(text),metrics);
      }
      if (marker && format.first_line_indent_pt < 0) {
        marker.style.display = 'inline-block';
        marker.style.width = `${-format.first_line_indent_pt}pt`;
        marker.style.textIndent = '0';
      }
    }
    appendRuns(text, item.runs,metrics);
    li.append(text);
    if (item.children?.length) {
      const nested = document.createElement(parent.tagName.toLowerCase());
      renderListItems(nested, item.children,metrics);
      li.append(nested);
    }
    parent.append(li);
  }
}
function renderBlock(block, formats, depth = 0, index = null, metrics = {}) {
  if (depth > 32) return document.createTextNode('Document nesting limit reached');
  let element;
  switch (block.type) {
    case 'Paragraph': case 'Heading': {
      element = document.createElement(block.type === 'Heading' ? `h${finite(block.level, 1, 1, 6)}` : 'p');
      blockStyle(element, block, formats, metrics);
      appendRuns(element, block.runs, metrics);
      if (formats.get(String(block.id))?.line_height_at_least) {
        // A minimum is the paragraph strut; each run also contributes its natural line box.
        for (const run of element.children) run.style.lineHeight = 'normal';
        element.style.lineHeight = `max(1em, ${element.style.lineHeight})`;
      }
      break;
    }
    case 'List':
      element = document.createElement(block.ordered ? 'ol' : 'ul');
      renderListItems(element, block.items,metrics);
      break;
    case 'Table': {
      element = document.createElement('table');
      element.className = 'writer-surface-table';
      const widths = (block.column_widths || []).map(width => finite(width, 0, 0, 10000));
      const total = widths.reduce((sum, width) => sum + width, 0);
      if (total > 0) {
        const columns = document.createElement('colgroup');
        for (const width of widths) {
          const column = document.createElement('col');
          column.style.width = `${width / total * 100}%`;
          columns.append(column);
        }
        element.style.width = `${total}px`;
        element.style.tableLayout = 'fixed';
        element.append(columns);
      }
      const tbody = document.createElement('tbody');
      for (const row of block.rows || []) {
        const tr = document.createElement('tr');
        for (const cell of row.cells || []) {
          const td = document.createElement('td');
          td.colSpan = finite(cell.col_span, 1, 1, 100);
          td.rowSpan = finite(cell.row_span, 1, 1, 100);
          if (cell.layout) {
            td.style.verticalAlign = ({Top:'top',Center:'middle',Bottom:'bottom'})[cell.layout.vertical_alignment] || 'top';
            td.style.padding = (cell.layout.padding || [0,5.4,0,5.4]).map(v=>`${finite(v,0,0,1000)}pt`).join(' ');
          }
          if (cell.background_color && CSS.supports('color', cell.background_color)) td.style.backgroundColor = cell.background_color;
          for (const [index, nested] of (cell.blocks || []).entries()) td.append(renderBlock(nested, formats, depth + 1, index,metrics));
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
        image.style.display = 'inline-block';
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
      for (const [index, nested] of (block.blocks || []).entries()) element.append(renderBlock(nested, formats, depth + 1, index,metrics));
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
    if (!isElement(node) || node.dataset.writerNumbering || node.dataset.writerDecoration) continue;
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

// Page furniture stays outside contenteditable and out of the save model.
// Hard breaks occupy the remainder of the physical page instead of looking
// like an ordinary empty paragraph. Re-measure after fonts and images load.
function layoutPages(surface) {
  if (!surface.root || !surface.model || surface.destroyed) return;
  const paper = surface.root.closest('.writer-page');
  if (!paper) return;
  const settings = surface.model.page_settings || {};
  const px = 96 / 25.4;
  const height = finite(settings.height_mm, 297, 30, 2000) * px;
  const top = finite(settings.margin_top, 25.4, 0, 500) * px;
  const bottom = finite(settings.margin_bottom, 25.4, 0, 500) * px;
  for (const gap of surface.root.querySelectorAll('[data-writer-decoration="gap"]')) gap.remove();
  const origin = paper.getBoundingClientRect().top;
  for (const block of Array.from(surface.root.children)) {
    if (block.classList.contains('writer-page-break')) {
      block.style.height = '0px';
      const y = block.getBoundingClientRect().top - origin;
      const nextPage = Math.floor(Math.max(0, y - top) / height) + 1;
      block.style.height = `${Math.max(0, nextPage * height + top - y)}px`;
    } else {
      const rect = block.getBoundingClientRect();
      const y = rect.top - origin;
      const page = Math.floor(Math.max(0, y - top) / height);
      const end = (page + 1) * height - bottom;
      // Keep a normal block together when it fits one page. Very long
      // paragraphs/tables remain editable and can naturally span pages.
      if (y + rect.height > end + 1 && rect.height <= height - top - bottom && y > page * height + top + 1) {
        const gap = document.createElement('div');
        gap.dataset.writerDecoration = 'gap';
        gap.contentEditable = 'false';
        gap.setAttribute('aria-hidden', 'true');
        gap.style.height = `${(page + 1) * height + top - y}px`;
        block.before(gap);
      }
    }
  }
  const last = surface.root.lastElementChild;
  const contentEnd = last ? last.getBoundingClientRect().bottom - origin : top;
  const count = Math.min(500, Math.max(1, Math.ceil((contentEnd + bottom - .5) / height)));
  paper.style.minHeight = `${height * count}px`;
  let furniture = paper.querySelector(':scope > .writer-page-furniture');
  if (!furniture) {
    furniture = document.createElement('div');
    furniture.className = 'writer-page-furniture';
    furniture.contentEditable = 'false';
    furniture.setAttribute('aria-hidden', 'true');
    paper.append(furniture);
  }
  furniture.replaceChildren();
  const layout = surface.model.professional || {};
  for (let index = 0; index < count; index++) {
    const page = document.createElement('div');
    page.className = 'writer-physical-page';
    page.style.top = `${index * height}px`;
    page.style.height = `${height}px`;
    if (!(index === 0 && layout.different_first_page)) {
      for (const [kind, value] of [['header', layout.header], ['footer', `${layout.footer || ''}${layout.page_numbers ? `  ${index + 1}` : ''}`]]) {
        if (!value) continue;
        const item = document.createElement('div');
        item.className = `writer-page-${kind}`;
        item.textContent = value;
        const alignment = String(layout[`${kind}_alignment`] || 'Center').toLowerCase();
        item.style.textAlign = ['left', 'center', 'right', 'justify'].includes(alignment) ? alignment : 'center';
        item.style.left = `${finite(settings.margin_left, 25.4, 0, 500)}mm`;
        item.style.right = `${finite(settings.margin_right, 25.4, 0, 500)}mm`;
        item.style[kind === 'header' ? 'top' : 'bottom'] = `${Math.max(2, (kind === 'header' ? top : bottom) / px * .4)}mm`;
        page.append(item);
      }
    }
    furniture.append(page);
  }
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
  surface.layout = () => {
    cancelAnimationFrame(surface.layoutFrame);
    surface.layoutFrame = requestAnimationFrame(() => layoutPages(surface));
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
    surface.layout();
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
    window.addEventListener('resize', surface.layout);
    surface.root.addEventListener('load', surface.layout, true);
    surface.render();
  };
  surface.render = () => {
    if (!surface.root || !surface.model || surface.destroyed) return;
    const offsets = selectionOffsets(surface.root);
    const formats = formatMap(surface.model);
    surface.renderModel = clone(surface.model);
    const fragment = document.createDocumentFragment();
    for (const [index, block] of (surface.model.body || []).entries()) fragment.append(renderBlock(block, formats, 0, index,surface.model.professional?.font_metrics));
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
    surface.layout();
    document.fonts.ready.then(surface.layout);
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
  // Browser formatBlock can merge adjacent Office paragraphs. Retag only the
  // selected model blocks, keeping their identities, run spans and table cells.
  if (format.kind === 'block' && /^(p|h[1-6])$/.test(format.value)) {
    if (!selected) return;
    const offsets = selectionOffsets(surface.root);
    const targets = Array.from(surface.root.querySelectorAll('p,h1,h2,h3,h4,h5,h6')).filter(node => {
      if (!selected.intersectsNode(node) || node.closest('[contenteditable="false"]')) return false;
      if (selected.collapsed) return node.contains(selected.startContainer);
      const overlap = document.createRange();
      overlap.selectNodeContents(node);
      if (node.contains(selected.startContainer)) overlap.setStart(selected.startContainer, selected.startOffset);
      if (node.contains(selected.endContainer)) overlap.setEnd(selected.endContainer, selected.endOffset);
      return overlap.toString().length > 0;
    });
    for (const node of targets) {
      if (node.localName === format.value) continue;
      const replacement = document.createElement(format.value);
      for (const attr of node.attributes) replacement.setAttribute(attr.name, attr.value);
      if (generatedBlocks.has(node)) generatedBlocks.set(replacement, generatedBlocks.get(node));
      while (node.firstChild) replacement.append(node.firstChild);
      node.replaceWith(replacement);
    }
    restoreOffsets(surface.root, offsets);
    surface.emit();
    return;
  }
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
  cancelAnimationFrame(surface.layoutFrame);
  window.removeEventListener('resize', surface.layout);
  document.removeEventListener('selectionchange', surface.remember);
  if (surface.root) {
    surface.root.removeEventListener('load', surface.layout, true);
    surface.root.closest('.writer-page')?.querySelector(':scope > .writer-page-furniture')?.remove();
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

import { createWriterSurface, updateWriterSurface, formatWriterSurface } from './writer-surface.js';

// The host owns persistence and acknowledges whole-document plain-text replacements
// with the canonical model, retaining opaque blocks throughout undo and redo.
export function mountWriter({ root, selector, status, options, onChange, onSave, onRead }) {
  const { zh, menus, remote, limit } = options;
  let ready = false,
    tooLarge = false,
    dirty = options.dirty,
    pending = false,
    unrecorded = false,
    revision = 0;
  let history = [],
    cursor = -1,
    units = 0;
  const say = (en, cn) => {
    status.textContent = zh ? cn : en;
  };
  const remember = (raw) => {
    if (raw === history[cursor]) return;
    for (const stale of history.splice(cursor + 1)) units -= stale.length;
    history.push(raw);
    units += raw.length;
    while (history.length > 1 && (history.length > 48 || units > 8 * 1024 * 1024))
      units -= history.shift().length;
    cursor = history.length - 1;
  };
  function notify(raw, record) {
    dirty = true;
    revision++;
    tooLarge = new Blob([raw]).size > limit;
    if (record) remember(raw);
    if (tooLarge) say('Document exceeds the editing limit.', '文档超过编辑大小限制。');
    else {
      say('Unsaved changes', '有未保存的修改');
      onChange(raw, revision);
    }
    bar.refresh();
    bar.refreshFind();
  }
  const surface = createWriterSurface(selector, (raw) => {
    const plain = raw.startsWith('{"__writer_plain_text":');
    pending = plain && new Blob([raw]).size <= limit;
    unrecorded = plain && !pending;
    notify(raw, !plain);
  });
  const enabled = () => ready && !surface.composing && !pending;
  function step(n) {
    if (!unrecorded) cursor += n;
    unrecorded = false;
    updateWriterSurface(surface, history[cursor], true);
    notify(history[cursor], false);
    surface.root.focus();
  }
  const actions = {
    undo: {
      label: ['Undo', '撤销'],
      shortcut: 'Ctrl+Z',
      enabled: () => enabled() && (unrecorded || cursor > 0),
      run: () => step(-1)
    },
    redo: {
      label: ['Redo', '重做'],
      shortcut: 'Ctrl+Y',
      enabled: () => enabled() && !unrecorded && cursor + 1 < history.length,
      run: () => step(1)
    },
    reading: {
      label: ['Return to reading', '返回阅读'],
      run: () => {
        if (!dirty) onRead();
        else {
          confirm.hidden = false;
          confirm.querySelector('button').focus();
        }
      }
    }
  };
  const save = remote ? 'save' : 'save_as';
  actions[save] = {
    label: remote ? ['Save to server', '保存到服务器'] : ['Save as DOCX…', '另存 DOCX…'],
    caption: true,
    shortcut: 'Ctrl+S',
    enabled: () => enabled() && !tooLarge && (!remote || dirty),
    run: () => {
      surface.emit();
      if (enabled() && !tooLarge) onSave();
    }
  };
  for (const [id, cn] of [
    ['bold', '粗体'],
    ['italic', '斜体']
  ]) {
    const active = () => document.queryCommandState(id);
    actions[id] = {
      label: [id[0].toUpperCase() + id.slice(1), cn],
      shortcut: id === 'bold' ? 'Ctrl+B' : 'Ctrl+I',
      enabled,
      pressed: active,
      run: () => formatWriterSurface(surface, JSON.stringify({ kind: id, value: !active() }))
    };
  }
  const styles = document.createElement('select');
  styles.setAttribute('aria-label', zh ? '段落样式' : 'Paragraph style');
  for (const [id, tag, en, cn] of [
    ['paragraph', 'p', 'Normal text', '正文'],
    ['heading1', 'h1', 'Heading 1', '标题 1'],
    ['heading2', 'h2', 'Heading 2', '标题 2']
  ]) {
    actions[id] = {
      label: [en, cn],
      enabled,
      run: () => {
        formatWriterSurface(surface, JSON.stringify({ kind: 'block', value: tag }));
        styles.value = tag;
      }
    };
    styles.add(new Option(zh ? cn : en, tag));
    styles.lastChild.dataset.action = id;
  }
  styles.onchange = () => {
    if (enabled()) actions[styles.selectedOptions[0].dataset.action].run();
  };
  const bar = window.SyntraOffice.mount({
    root,
    menus,
    actions,
    zh,
    content: () => surface.root,
    extra: { file: ['reading'], format: ['heading1', 'heading2'] },
    groups: [
      [save, 'print', 'reading'],
      ['undo', 'redo', 'find'],
      [styles, 'bold', 'italic']
    ]
  });
  styles.disabled = true;
  const confirm = document.createElement('div');
  confirm.hidden = true;
  confirm.className = 'syntra-discard';
  confirm.setAttribute('role', 'group');
  confirm.append(zh ? '放弃未保存的修改？' : 'Discard unsaved changes?');
  for (const [name, run] of [
    [
      zh ? '继续编辑' : 'Keep editing',
      () => {
        confirm.hidden = true;
        surface.root.focus();
      }
    ],
    [zh ? '放弃修改' : 'Discard edits', onRead]
  ]) {
    const btn = document.createElement('button');
    btn.textContent = name;
    btn.onclick = run;
    confirm.append(btn);
  }
  root.append(confirm);
  document.addEventListener('selectionchange', () => {
    const node = document.getSelection()?.anchorNode;
    if (surface.root && surface.root.contains(node)) {
      const block = (node.nodeType === 1 ? node : node.parentElement)?.closest('h1,h2,p');
      styles.value = block?.localName || 'p';
      bar.refresh();
    }
  });
  if (dirty) say('Unsaved changes', '有未保存的修改');
  return {
    load(model) {
      updateWriterSurface(surface, model, true);
      remember(model);
      ready = true;
      styles.disabled = false;
      bar.refresh();
    },
    restore(version, model) {
      if (version === revision) {
        pending = false;
        updateWriterSurface(surface, model, true);
        remember(model);
        bar.refresh();
      }
    }
  };
}

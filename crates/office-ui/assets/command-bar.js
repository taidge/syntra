// Embedded Office hosts supply only executable actions. Menus and toolbar buttons
// share one dispatcher, availability state, shortcuts and accessible labels.
window.SyntraOffice = (() => {
  const paths = {
    find: 'M16 16l5 5 M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0',
    print: 'M7 8V3h10v5 M6 17H3V9h18v8h-3 M7 14h10v7H7z M17 11h1',
    open: 'M3 7V5h6l2 2h10v3M3 9h19l-3 10H4z',
    save: 'M4 3h13l3 3v15H4z M8 3v6h8V3 M8 21v-8h8v8',
    save_as: 'M4 3h13l3 3v4 M4 3v18h6 M8 3v6h8V3 M13 18l6-6 3 3-6 6-4 1z',
    edit: 'M4 17l12-12 4 4L8 21H4z M14 7l4 4',
    reading: 'M2 5q5-2 10 1 5-3 10-1v14q-5-2-10 1-5-3-10-1z M12 6v14',
    undo: 'M9 5l-6 6 6 5 M4 11h10a6 6 0 0 1 6 6',
    redo: 'M15 5l6 6-6 5 M20 11H10a6 6 0 0 0-6 6',
    bold: 'M7 4h6a4 4 0 0 1 0 8H7z M7 12h7a4 4 0 0 1 0 8H7z',
    italic: 'M12 4h7 M5 20h7 M15.5 4l-7 16',
    zoom_in: 'M16 16l5 5 M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0 M6 10h8 M10 6v8',
    zoom_out: 'M16 16l5 5 M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0 M6 10h8',
    fit: 'M3 5v14 M21 5v14 M5 12h14 M9 8l-4 4 4 4 M15 8l4 4-4 4',
    previous: 'M14 6l-6 6 6 6',
    next: 'M10 6l6 6-6 6'
  };
  const titles = {
    file: ['File', '文件'],
    edit: ['Edit', '编辑'],
    view: ['View', '视图'],
    format: ['Format', '格式'],
    insert: ['Insert', '插入'],
    tools: ['Tools', '工具'],
    help: ['Help', '帮助']
  };
  function glyph(id) {
    const ns = 'http://www.w3.org/2000/svg';
    const svg = document.createElementNS(ns, 'svg');
    svg.setAttribute('viewBox', '0 0 24 24');
    svg.setAttribute('aria-hidden', 'true');
    const path = document.createElementNS(ns, 'path');
    path.setAttribute('d', paths[id]);
    svg.append(path);
    return svg;
  }
  function mount({ root, menus, actions, groups, extra = {}, zh = false, kind = 'writer', content }) {
    root.className = 'syntra-command-bar';
    root.dataset.kind = kind;
    const element = (tag, cls, parent) => {
      const node = document.createElement(tag);
      node.className = cls;
      parent.append(node);
      return node;
    };
    const button = (cls, parent, name) => {
      const node = element('button', cls, parent);
      node.type = 'button';
      node.setAttribute('aria-label', name);
      return node;
    };
    const lang = (a) => a[zh ? 1 : 0];
    const bindings = [];
    const findBar = element('div', 'syntra-find', root);
    findBar.hidden = true;
    const input = element('input', '', findBar);
    input.type = 'search';
    input.setAttribute('aria-label', zh ? '查找内容' : 'Find text');
    input.placeholder = zh ? '在文档中查找' : 'Find in document';
    const count = element('output', '', findBar);
    let matches = [],
      index = 0,
      capped = false;
    const highlightKey = 'syntra-search';
    function paint() {
      globalThis.CSS?.highlights?.delete(highlightKey);
      count.value = !input.value
        ? ''
        : matches.length
          ? index + 1 + ' / ' + matches.length + (capped ? '+' : '')
          : zh
            ? '无结果'
            : 'No matches';
      if (!matches.length) return;
      const range = matches[index];
      if (globalThis.CSS?.highlights && globalThis.Highlight)
        CSS.highlights.set(highlightKey, new Highlight(range));
      else {
        const sel = getSelection();
        sel.removeAllRanges();
        sel.addRange(range);
      }
      range.startContainer.parentElement?.scrollIntoView({ block: 'center', inline: 'nearest' });
    }
    function search() {
      matches = [];
      index = 0;
      capped = false;
      const body = content?.();
      if (input.value && body) {
        const walker = document.createTreeWalker(body, NodeFilter.SHOW_TEXT);
        const term = input.value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
        const pattern = new RegExp(term, 'igu');
        let block,
          nodes = [],
          length = 0;
        function collect() {
          if (!nodes.length) return;
          const joined = nodes.map((part) => part.node.data).join('');
          let first = 0,
            last = 0;
          for (const hit of joined.matchAll(pattern)) {
            if (matches.length === 1000) {
              capped = true;
              break;
            }
            const end = hit.index + hit[0].length;
            while (nodes[first].end <= hit.index) first++;
            last = Math.max(last, first);
            while (nodes[last].end < end) last++;
            const range = new Range();
            range.setStart(nodes[first].node, hit.index - nodes[first].start);
            range.setEnd(nodes[last].node, end - nodes[last].start);
            matches.push(range);
          }
          nodes = [];
          length = 0;
        }
        while (!capped && walker.nextNode()) {
          const node = walker.currentNode,
            parent = node.parentElement;
          if (!node.length || !parent || parent.closest('[hidden],script,style')) continue;
          const nextBlock = parent.closest('p,h1,h2,h3,h4,h5,h6,td,th,li,div,section') || body;
          if (nextBlock !== block) {
            collect();
            block = nextBlock;
          }
          nodes.push({ node, start: length, end: length + node.length });
          length += node.length;
        }
        if (!capped) collect();
      }
      paint();
    }
    function advance(step) {
      if (matches.length) {
        index = (index + step + matches.length) % matches.length;
        paint();
      }
    }
    let beforeFind;
    function hideFind() {
      findBar.hidden = true;
      globalThis.CSS?.highlights?.delete(highlightKey);
      beforeFind?.focus();
    }
    for (const [name, icon, run] of [
      [zh ? '上一个匹配' : 'Previous match', 'previous', () => advance(-1)],
      [zh ? '下一个匹配' : 'Next match', 'next', () => advance(1)],
      [zh ? '关闭查找' : 'Close find', null, hideFind]
    ]) {
      const btn = button('syntra-tool', findBar, name);
      btn.title = name;
      if (icon) btn.append(glyph(icon));
      else btn.textContent = '×';
      btn.onclick = run;
    }
    input.oninput = search;
    findBar.onkeydown = (e) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        hideFind();
      }
      if (e.key === 'Enter') {
        e.preventDefault();
        advance(e.shiftKey ? -1 : 1);
      }
    };
    if (content) {
      actions.find = {
        label: ['Find', '查找'],
        shortcut: 'Ctrl+F',
        run: () => {
          if (findBar.hidden) beforeFind = document.activeElement;
          findBar.hidden = false;
          input.focus();
          input.select();
          search();
        }
      };
      actions.print = { label: ['Print…', '打印…'], shortcut: 'Ctrl+P', run: () => window.print() };
    }

    const menusRow = element('div', 'syntra-menubar', root);
    menusRow.setAttribute('role', 'menubar');
    const toolbar = element('div', 'syntra-toolbar', root);
    toolbar.setAttribute('role', 'toolbar');
    toolbar.setAttribute('aria-label', zh ? '常用操作' : 'Document tools');
    let current = null;
    const triggers = [];
    const enabled = (a) => !a.enabled || a.enabled();
    function close(restore = false) {
      if (!current) return;
      const { trigger, popup } = current;
      current = null;
      popup.hidden = true;
      trigger.setAttribute('aria-expanded', 'false');
      if (restore) trigger.focus();
    }
    function refresh() {
      for (const { node, a, tool } of bindings) {
        node.disabled = !enabled(a);
        if (tool && a.pressed) node.setAttribute('aria-pressed', String(a.pressed()));
      }
    }
    function invoke(a) {
      if (!enabled(a)) return;
      close();
      a.run();
      refresh();
    }
    function actionButton(id, parent, tool) {
      const a = actions[id],
        label = lang(a.label);
      const node = button(tool ? 'syntra-tool' : 'syntra-item', parent, label);
      if (!tool) node.setAttribute('role', 'menuitem');
      if (tool && paths[id]) node.append(glyph(id));
      if (!tool || a.caption || !paths[id]) element('span', 'syntra-caption', node).textContent = label;
      if (!tool && a.shortcut) element('kbd', '', node).textContent = a.shortcut;
      node.title = label + (a.shortcut ? ' (' + a.shortcut + ')' : '');
      // Keep the document selection for pointer-initiated formatting.
      node.onmousedown = (e) => e.preventDefault();
      node.onclick = () => invoke(a);
      bindings.push({ node, a, tool });
      return node;
    }
    for (const [id, entries] of menus) {
      const ids = [...new Set([...entries, ...(extra[id] || [])])].filter((key) =>
        Object.hasOwn(actions, key)
      );
      if (!ids.length) continue;
      const wrap = element('div', 'syntra-menu', menusRow);
      const trigger = button('syntra-menu-trigger', wrap, lang(titles[id]));
      trigger.textContent = lang(titles[id]);
      trigger.setAttribute('role', 'menuitem');
      trigger.setAttribute('aria-haspopup', 'menu');
      trigger.setAttribute('aria-expanded', 'false');
      const popup = element('div', 'syntra-menu-popup', wrap);
      popup.setAttribute('role', 'menu');
      popup.setAttribute('aria-label', lang(titles[id]));
      popup.hidden = true;
      for (const key of ids) actionButton(key, popup, false);
      const show = (focus = false) => {
        close();
        refresh();
        popup.hidden = false;
        trigger.setAttribute('aria-expanded', 'true');
        current = { trigger, popup };
        popup.style.left =
          Math.min(
            0,
            innerWidth - popup.getBoundingClientRect().width - wrap.getBoundingClientRect().left - 4
          ) + 'px';
        if (focus) popup.querySelector('button:enabled')?.focus();
      };
      triggers.push({ trigger, show });
      trigger.onclick = (e) => (current?.trigger === trigger ? close() : show(e.detail === 0));
      trigger.onpointerenter = () => {
        if (current && current.trigger !== trigger) show();
      };
    }
    for (const group of groups) {
      const items = group.filter((item) => typeof item !== 'string' || Object.hasOwn(actions, item));
      if (!items.length) continue;
      const box = element('div', 'syntra-tool-group', toolbar);
      for (const item of items) typeof item === 'string' ? actionButton(item, box, true) : box.append(item);
    }
    menusRow.onkeydown = (e) => {
      const which = triggers.findIndex((m) => m.trigger === (current?.trigger || e.target));
      if (which < 0) return;
      if (e.key === 'Escape') close(true);
      else if (['ArrowRight', 'ArrowLeft'].includes(e.key)) {
        const next =
          triggers[(which + (e.key === 'ArrowRight' ? 1 : -1) + triggers.length) % triggers.length];
        if (current) next.show(true);
        else next.trigger.focus();
      } else if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) {
        if (!current) triggers[which].show();
        const items = [...current.popup.querySelectorAll('button:enabled')];
        const i = items.indexOf(document.activeElement);
        const pos =
          e.key === 'Home'
            ? 0
            : e.key === 'End'
              ? items.length - 1
              : i < 0
                ? e.key === 'ArrowUp'
                  ? items.length - 1
                  : 0
                : (i + (e.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length;
        items[pos]?.focus();
      } else return;
      e.preventDefault();
      e.stopPropagation();
    };
    document.addEventListener(
      'pointerdown',
      (e) => {
        if (!menusRow.contains(e.target)) close();
      },
      true
    );
    document.addEventListener('focusin', (e) => {
      if (!menusRow.contains(e.target)) close();
    });
    document.addEventListener('keydown', (e) => {
      if (e.isComposing || e.altKey || !(e.ctrlKey || e.metaKey)) return;
      let key = e.key.toLowerCase();
      if (key === '=') key = '+';
      const chord = 'ctrl+' + (e.shiftKey && key !== '+' ? 'shift+' : '') + key;
      const a = Object.values(actions).find((a) => a.shortcut?.toLowerCase() === chord);
      if (a) {
        e.preventDefault();
        invoke(a);
      }
    });
    root.append(findBar);
    refresh();
    return {
      refresh,
      toolbar,
      refreshFind: () => {
        if (!findBar.hidden) search();
      }
    };
  }
  return { mount };
})();

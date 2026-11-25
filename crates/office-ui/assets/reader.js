// Render Syntra's exported HTML as a paged or tabbed offline reader.
window.SyntraOffice.mountReader = ({ kind, width, zh, locale, menus, open, edit }) => {
  const label = (en, cn) => (zh ? cn : en);
  const make = (tag, className) => {
    const node = document.createElement(tag);
    if (className) node.className = className;
    return node;
  };
  const style = make('style');
  style.textContent = `
        html,body{height:100%;overflow:hidden}
        body{display:flex;flex-direction:column;background:#e8eaee;margin:0}
        .reader-stage{flex:1;min-height:0;overflow:auto;padding:12px;overscroll-behavior:contain}
        .reader-paper{background:white;margin-inline:auto;box-shadow:0 1px 5px #0003}
        .reader-paper[data-kind="writer"]{min-height:85vh;padding:clamp(20px,5vw,72px)}
        .reader-paper[data-kind="calc"]{padding:0}
        .reader-paper[data-kind="calc"] .sheet > h1{display:none}
        .reader-paper .writer-page-break{margin:18px 0;border-top:1px dashed #bcc3cd}
        .reader-paper [hidden]{display:none!important}
    `;
  document.head.append(style);
  const paper = make('article', 'reader-paper');
  paper.dataset.kind = kind;
  while (document.body.firstChild) paper.append(document.body.firstChild);
  // Reading a hyperlink never replaces the current document.
  for (const a of paper.querySelectorAll('a[href]')) a.removeAttribute('href');
  const stage = make('div', 'reader-stage');
  stage.append(paper);
  const root = make('header');
  document.body.append(root, stage);
  const actions = {
    open: { label: ['Open Office document', '打开 Office 文档'], shortcut: 'Ctrl+O', run: open },
    zoom_in: {
      label: ['Zoom in', '放大'],
      shortcut: 'Ctrl++',
      run: () => adjust(1.25),
      enabled: () => scale < 4
    },
    zoom_out: {
      label: ['Zoom out', '缩小'],
      shortcut: 'Ctrl+-',
      run: () => adjust(1 / 1.25),
      enabled: () => scale > 0.05
    },
    zoom_reset: {
      label: ['Actual size', '实际大小'],
      shortcut: 'Ctrl+0',
      run: () => {
        fit = false;
        scale = 1;
        resize();
      }
    },
    fit: {
      label: ['Fit width', '适应宽度'],
      run: () => {
        fit = true;
        resize();
      },
      pressed: () => fit
    }
  };
  if (edit) actions.edit = { label: ['Edit Word document', '编辑 Word 文档'], caption: true, run: edit };
  const navigation = [];
  let bar,
    fit = true,
    scale = 1;
  // A sheet may have several print segments followed by separate chart sections.
  const groups = [];
  if (kind === 'slides') {
    for (const node of paper.querySelectorAll(':scope > .canvas-page')) {
      groups.push({ name: label('Slide ', '幻灯片 ') + (groups.length + 1), nodes: [node] });
    }
  } else if (kind === 'calc') {
    const shortDate = new Intl.DateTimeFormat(locale || undefined, {
      year: 'numeric', month: 'numeric', day: 'numeric', timeZone: 'UTC'
    });
    for (const cell of paper.querySelectorAll('td[data-short-date]')) {
      const iso = cell.textContent.split('T')[0];
      const date = new Date(iso + 'T00:00:00Z');
      if (Number.isFinite(date.valueOf())) cell.textContent = shortDate.format(date);
    }
    for (const node of paper.children) {
      const name = node.getAttribute('aria-label') ?? node.querySelector('h1')?.textContent;
      if (node.matches('.sheet') && (!groups.length || groups.at(-1).name !== name)) {
        groups.push({ name: name || label('Worksheet', '工作表'), nodes: [] });
      }
      groups.at(-1)?.nodes.push(node);
    }
  }
  if (groups.length) {
    let index = 0;
    actions.previous = {
      label: ['Previous', '上一项'],
      run: () => select(index - 1),
      enabled: () => index > 0
    };
    actions.next = {
      label: ['Next', '下一项'],
      run: () => select(index + 1),
      enabled: () => index + 1 < groups.length
    };
    const picker = make('select');
    picker.setAttribute(
      'aria-label',
      kind === 'calc' ? label('Worksheet', '工作表') : label('Slide', '幻灯片')
    );
    groups.forEach((group, i) => picker.add(new Option(group.name, String(i))));
    picker.onchange = () => select(picker.selectedIndex);
    navigation.push('previous', picker, 'next');
    function select(i) {
      if (i < 0 || i >= groups.length) return;
      for (const el of groups[index].nodes) el.hidden = true;
      index = i;
      for (const el of groups[index].nodes) el.hidden = false;
      picker.selectedIndex = index;
      if (bar) resize();
      bar?.refreshFind();
      stage.scrollTo(0, 0);
    }
    for (const group of groups)
      for (const el of group.nodes) {
        el.dataset.syntraGroup = '';
        el.hidden = true;
      }
    select(0);
    addEventListener('keydown', (event) => {
      if (event.target.closest('select,button') || kind !== 'slides') return;
      if (['PageUp', 'ArrowLeft', 'PageDown', 'ArrowRight'].includes(event.key)) {
        event.preventDefault();
        select(index + (['PageUp', 'ArrowLeft'].includes(event.key) ? -1 : 1));
      }
    });
  }

  const status = make('output');
  bar = window.SyntraOffice.mount({
    root,
    kind,
    zh,
    menus,
    actions,
    content: () => paper,
    extra: { edit: ['edit'], view: ['fit', 'previous', 'next'] },
    groups: [['open', 'print', 'find', 'edit'], navigation, ['zoom_out', status, 'zoom_in', 'fit']]
  });
  function adjust(factor) {
    fit = false;
    scale = Math.min(4, Math.max(0.05, scale * factor));
    resize();
  }
  function resize() {
    const grid = kind === 'calc' && paper.querySelector('.sheet:not([hidden]) > table');
    const natural = grid?.offsetWidth || width;
    if (fit) scale = Math.min(1, Math.max(0.01, (stage.clientWidth - 24) / natural));
    paper.style.width = natural + 'px';
    paper.style.zoom = String(scale);
    status.value = Math.round(scale * 100) + '%';
    bar.refresh();
  }
  new ResizeObserver(resize).observe(stage);
  resize();
};

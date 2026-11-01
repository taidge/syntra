//! Application-owned workbook state and semantic Glory/Sapid presentation.
use super::*;

#[derive(Clone)]
struct Book {
    snapshot: CalcSnapshot,
    content: serde_json::Value,
    undo: Vec<(CalcSnapshot, serde_json::Value)>,
    redo: Vec<(CalcSnapshot, serde_json::Value)>,
    revision: u64,
    dirty: bool,
}
impl Default for Book {
    fn default() -> Self {
        Self {
            snapshot: CalcSnapshot {
                cells: HashMap::new(),
                styles: HashMap::new(),
                merged_cells: HashMap::new(),
                sheets: vec!["Sheet1".into()],
            },
            content: serde_json::json!({"sheets": []}),
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            dirty: false,
        }
    }
}
impl Book {
    fn hydrate(&mut self, content: serde_json::Value) {
        let (mut sheets, cells, styles, merged_cells) = hydrate_workbook(&content);
        if sheets.is_empty() {
            sheets.push("Sheet1".into());
        }
        self.snapshot = CalcSnapshot {
            sheets,
            cells,
            styles,
            merged_cells,
        };
        self.content = content;
        self.undo.clear();
        self.redo.clear();
        self.dirty = false;
        self.revision += 1;
    }
    fn checkpoint(&mut self) {
        self.undo
            .push((self.snapshot.clone(), self.content.clone()));
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.changed();
    }
    fn changed(&mut self) {
        self.revision += 1;
        self.dirty = true;
    }
    fn serialize(&self) -> serde_json::Value {
        let snap = &self.snapshot;
        let mut root = self.content.as_object().cloned().unwrap_or_default();
        let sheets = snap
            .sheets
            .iter()
            .enumerate()
            .map(|(si, name)| {
                let mut sheet = self
                    .content
                    .get("sheets")
                    .and_then(|s| s.get(si))
                    .and_then(|s| s.as_object())
                    .cloned()
                    .unwrap_or_else(|| {
                        serde_json::json!({"id":uuid::Uuid::new_v4(), "column_widths":{},
                        "row_heights":{}, "frozen_rows":0, "frozen_cols":0,
                        "conditional_formats":[], "data_validations":[], "charts":[]})
                        .as_object()
                        .unwrap()
                        .clone()
                    });
                let keys: std::collections::BTreeSet<_> = snap
                    .cells
                    .keys()
                    .chain(snap.styles.keys())
                    .filter(|key| key.0 == si)
                    .map(|key| (key.1, key.2))
                    .collect();
                let cells = keys.into_iter().map(|(r,c)| {
                let value = snap.cells.get(&(si,r,c)).cloned().unwrap_or_default();
                let mut cell = if value.starts_with('=') {
                    serde_json::json!({"formula":value,"value":{"type":"Empty"}})
                } else if value.is_empty() {
                    serde_json::json!({"formula":null,"value":{"type":"Empty"}})
                } else if let Some(n) = value.parse::<f64>().ok().filter(|n| n.is_finite()) {
                    serde_json::json!({"formula":null,"value":{"type":"Number","value":n}})
                } else {
                    serde_json::json!({"formula":null,"value":{"type":"Text","value":value}})
                };
                if let Some(style) = snap.styles.get(&(si,r,c)).and_then(style_to_json) {
                    cell["format"] = style;
                }
                serde_json::json!([{"row":r,"col":c},cell])
            }).collect();
                let ranges: Vec<_> = snap
                    .merged_cells
                    .get(&si)
                    .into_iter()
                    .flatten()
                    .map(|range| {
                        let range = range.normalized();
                        serde_json::json!({"start":{"row":range.start.0,"col":range.start.1},
                    "end":{"row":range.end.0,"col":range.end.1}})
                    })
                    .collect();
                sheet.insert("name".into(), name.clone().into());
                sheet.insert("cells".into(), serde_json::Value::Array(cells));
                sheet.insert("merged_cells".into(), ranges.into());
                serde_json::Value::Object(sheet)
            })
            .collect();
        root.insert("sheets".into(), serde_json::Value::Array(sheets));
        serde_json::Value::Object(root)
    }
}

#[derive(Clone, Copy)]
struct Editor {
    book: Cage<Book>,
    sheet: Cage<usize>,
    active: Cage<(u32, u32)>,
    anchor: Cage<(u32, u32)>,
    end: Cage<(u32, u32)>,
    extra: Cage<Vec<(u32, u32)>>,
    extend: Cage<bool>,
    additive: Cage<bool>,
    filter: Cage<Option<(u32, String)>>,
    formula: Cage<String>,
    status: Cage<String>,
    layout: Cage<u64>,
}
impl Editor {
    fn selected(self) -> Vec<(u32, u32)> {
        current_selection_cells(
            self.active.get(),
            Some(self.anchor.get()),
            Some(self.end.get()),
            &self.extra.get(),
        )
    }
    fn activate(self, point: (u32, u32)) {
        let sheet = self.sheet.get();
        let point = self.book.with(|book| {
            merged_range_at(&book.snapshot.merged_cells, sheet, point)
                .map(|range| range.start)
                .unwrap_or(point)
        });
        self.active.revise(|v| *v = point);
        self.anchor.revise(|v| *v = point);
        self.end.revise(|v| *v = point);
        self.extra.revise(Vec::clear);
        let raw = self.book.with(|b| {
            b.snapshot
                .cells
                .get(&(sheet, point.0, point.1))
                .cloned()
                .unwrap_or_default()
        });
        self.formula.revise(|v| *v = raw);
    }
    fn commit(self, value: String) {
        let si = self.sheet.get();
        let (r, c) = self.active.get();
        let error = self
            .book
            .with(|b| validation_error_for(&b.content, si, r, c, &value));
        if let Some(error) = error {
            self.status.revise(|v| *v = error);
            return;
        }
        self.book.revise(|b| {
            if b.snapshot
                .cells
                .get(&(si, r, c))
                .map(String::as_str)
                .unwrap_or("")
                == value
            {
                return;
            }
            b.checkpoint();
            if value.is_empty() {
                b.snapshot.cells.remove(&(si, r, c));
            } else {
                b.snapshot.cells.insert((si, r, c), value);
            }
        });
    }
    fn format(self, update: impl Fn(&mut CellStyle)) {
        let si = self.sheet.get();
        let cells = self.selected();
        self.book.revise(|b| {
            b.checkpoint();
            for (r, c) in cells {
                update(b.snapshot.styles.entry((si, r, c)).or_default());
            }
        });
    }
    fn history(self, redo: bool) {
        self.book.revise(|b| {
            let previous = if redo { b.redo.pop() } else { b.undo.pop() };
            if let Some((previous, content)) = previous {
                let current = (
                    std::mem::replace(&mut b.snapshot, previous),
                    std::mem::replace(&mut b.content, content),
                );
                if redo {
                    b.undo.push(current)
                } else {
                    b.redo.push(current)
                }
                b.changed();
            }
        });
        let count = self.book.with(|b| b.snapshot.sheets.len());
        self.sheet
            .revise(|v| *v = (*v).min(count.saturating_sub(1)));
        self.layout.revise(|v| *v += 1);
        self.activate(self.active.get());
    }
    fn merge(self, unmerge: bool) {
        let si = self.sheet.get();
        let range = MergedRange {
            start: self.anchor.get(),
            end: self.end.get(),
        }
        .normalized();
        self.book.revise(|b| {
            b.checkpoint();
            let ranges = b.snapshot.merged_cells.entry(si).or_default();
            ranges.retain(|r| !r.overlaps(range));
            if !unmerge && range.start != range.end {
                ranges.push(range);
            }
        });
        self.layout.revise(|v| *v += 1);
    }
    fn row_column(self, column: bool, delete: bool) {
        let si = self.sheet.get();
        let at = if column {
            self.active.get().1
        } else {
            self.active.get().0
        };
        self.book.revise(|b| {
            b.checkpoint();
            fn shift<T>(
                map: &mut HashMap<CellKey, T>,
                si: usize,
                at: u32,
                column: bool,
                delete: bool,
            ) {
                *map = std::mem::take(map)
                    .into_iter()
                    .filter_map(|((s, r, c), value)| {
                        if s != si {
                            return Some(((s, r, c), value));
                        }
                        let n = if column { c } else { r };
                        if delete && n == at {
                            return None;
                        }
                        let shifted = if n >= at {
                            if delete { n.saturating_sub(1) } else { n + 1 }
                        } else {
                            n
                        };
                        let (r, c) = if column { (r, shifted) } else { (shifted, c) };
                        (r < NUM_ROWS && c < NUM_COLS).then_some(((s, r, c), value))
                    })
                    .collect();
            }
            shift(&mut b.snapshot.cells, si, at, column, delete);
            shift(&mut b.snapshot.styles, si, at, column, delete);
            match (column, delete) {
                (false, false) => insert_row_in_merged(&mut b.snapshot.merged_cells, si, at),
                (false, true) => delete_row_from_merged(&mut b.snapshot.merged_cells, si, at),
                (true, false) => insert_col_in_merged(&mut b.snapshot.merged_cells, si, at),
                (true, true) => delete_col_from_merged(&mut b.snapshot.merged_cells, si, at),
            }
        });
        self.layout.revise(|v| *v += 1);
    }
    fn sort(self, descending: bool) {
        let si = self.sheet.get();
        let a = self.anchor.get();
        let z = self.end.get();
        let (first, last) = if a.0 == z.0 {
            (0, NUM_ROWS - 1)
        } else {
            (a.0.min(z.0), a.0.max(z.0))
        };
        let col = self.active.get().1;
        if self.book.with(|b| {
            merged_range_overlaps(
                &b.snapshot.merged_cells,
                si,
                (first, 0),
                (last, NUM_COLS - 1),
            )
        }) {
            self.status
                .revise(|v| *v = "Unmerge the selected rows before sorting".into());
            return;
        }
        self.book.revise(|b| {
            b.checkpoint();
            let mut rows: Vec<_> = (first..=last)
                .map(|r| {
                    let values: Vec<_> = (0..NUM_COLS)
                        .map(|c| {
                            (
                                b.snapshot.cells.get(&(si, r, c)).cloned(),
                                b.snapshot.styles.get(&(si, r, c)).cloned(),
                            )
                        })
                        .collect();
                    (
                        b.snapshot
                            .cells
                            .get(&(si, r, col))
                            .cloned()
                            .unwrap_or_default(),
                        values,
                    )
                })
                .collect();
            rows.sort_by(|a, b| {
                match (a.0.is_empty(), b.0.is_empty()) {
                    (true, false) => return std::cmp::Ordering::Greater,
                    (false, true) => return std::cmp::Ordering::Less,
                    _ => {}
                }
                let ord = match (a.0.parse::<f64>(), b.0.parse::<f64>()) {
                    (Ok(a), Ok(b)) => a.total_cmp(&b),
                    _ => a.0.to_lowercase().cmp(&b.0.to_lowercase()),
                };
                if descending { ord.reverse() } else { ord }
            });
            for (offset, (_, values)) in rows.into_iter().enumerate() {
                for (c, (value, style)) in values.into_iter().enumerate() {
                    let key = (si, first + offset as u32, c as u32);
                    b.snapshot.cells.remove(&key);
                    b.snapshot.styles.remove(&key);
                    if let Some(value) = value {
                        b.snapshot.cells.insert(key, value);
                    }
                    if let Some(style) = style {
                        b.snapshot.styles.insert(key, style);
                    }
                }
            }
        });
    }
    fn fill(self, down: bool) {
        let si = self.sheet.get();
        let range = MergedRange {
            start: self.anchor.get(),
            end: self.end.get(),
        }
        .normalized();
        self.book.revise(|b| {
            b.checkpoint();
            for r in range.start.0..=range.end.0 {
                for c in range.start.1..=range.end.1 {
                    let source = if down {
                        (si, range.start.0, c)
                    } else {
                        (si, r, range.start.1)
                    };
                    let key = (si, r, c);
                    if let Some(value) = b.snapshot.cells.get(&source).cloned() {
                        b.snapshot.cells.insert(key, value);
                    }
                    if let Some(style) = b.snapshot.styles.get(&source).cloned() {
                        b.snapshot.styles.insert(key, style);
                    }
                }
            }
        });
    }
    fn metadata(self, operation: impl FnOnce(&mut serde_json::Value, usize, MergedRange)) {
        let si = self.sheet.get();
        let range = MergedRange {
            start: self.anchor.get(),
            end: self.end.get(),
        }
        .normalized();
        self.book.revise(|b| {
            b.checkpoint();
            b.content = b.serialize();
            operation(&mut b.content, si, range);
        });
    }
    fn duplicate_sheet(self) {
        let si = self.sheet.get();
        let target = self.book.revise(|b| {
            b.checkpoint();
            b.content = b.serialize();
            let target = b.snapshot.sheets.len();
            let name = format!("{} copy", b.snapshot.sheets[si]);
            b.snapshot.sheets.push(name.clone());
            let cells: Vec<_> = b
                .snapshot
                .cells
                .iter()
                .filter(|(k, _)| k.0 == si)
                .map(|((_, r, c), v)| ((target, *r, *c), v.clone()))
                .collect();
            let styles: Vec<_> = b
                .snapshot
                .styles
                .iter()
                .filter(|(k, _)| k.0 == si)
                .map(|((_, r, c), v)| ((target, *r, *c), v.clone()))
                .collect();
            b.snapshot.cells.extend(cells);
            b.snapshot.styles.extend(styles);
            if let Some(ranges) = b.snapshot.merged_cells.get(&si).cloned() {
                b.snapshot.merged_cells.insert(target, ranges);
            }
            let mut sheet = b.content["sheets"][si].clone();
            sheet["id"] = serde_json::json!(uuid::Uuid::new_v4());
            sheet["name"] = name.into();
            append_array_entry(&mut b.content, "sheets", sheet);
            target
        });
        self.sheet.revise(|v| *v = target);
        self.activate((0, 0));
        self.layout.revise(|v| *v += 1);
    }
    fn move_sheet(self, right: bool) {
        let from = self.sheet.get();
        let count = self.book.with(|b| b.snapshot.sheets.len());
        let to = if right {
            (from + 1).min(count - 1)
        } else {
            from.saturating_sub(1)
        };
        if from == to {
            return;
        }
        self.book.revise(|b| {
            b.checkpoint();
            b.content = b.serialize();
            b.snapshot.sheets.swap(from, to);
            let swap = |s| {
                if s == from {
                    to
                } else if s == to {
                    from
                } else {
                    s
                }
            };
            b.snapshot.cells = std::mem::take(&mut b.snapshot.cells)
                .into_iter()
                .map(|((s, r, c), v)| ((swap(s), r, c), v))
                .collect();
            b.snapshot.styles = std::mem::take(&mut b.snapshot.styles)
                .into_iter()
                .map(|((s, r, c), v)| ((swap(s), r, c), v))
                .collect();
            swap_sheets_in_merged(&mut b.snapshot.merged_cells, from, to);
            if let Some(sheets) = b.content["sheets"].as_array_mut() {
                sheets.swap(from, to);
            }
        });
        self.sheet.revise(|v| *v = to);
        self.layout.revise(|v| *v += 1);
    }
}

fn action(label: &str, callback: impl FnMut() + 'static) -> Box<dyn Widget> {
    let caption = match label {
        "Bold" => "B",
        "Italic" => "I",
        "Underline" => "U",
        "Align left" => "⇤",
        "Align center" => "↔",
        "Align right" => "⇥",
        "Currency" => "$",
        "Percent" => "%",
        "Merge cells" => "Merge",
        "Unmerge cells" => "Unmerge",
        "Sort ascending" => "A↓Z",
        "Sort descending" => "Z↓A",
        "All borders" => "▦",
        "Clear borders" => "□",
        _ => label,
    };
    Box::new(
        sapid::button()
            .theme(crate::components::ui::theme())
            .variant(sapid::ButtonVariant::Secondary)
            .size(sapid::Size::Small)
            .class("toolbar-btn")
            .name(label.to_owned())
            .label(caption.to_owned())
            .on_press(callback),
    )
}
fn control(
    name: &str,
    value: impl Into<Vary<String>>,
    callback: impl FnMut(String) + 'static,
) -> sapid::TextField {
    sapid::text_field()
        .theme(crate::components::ui::theme())
        .name(name.to_owned())
        .value(value)
        .on_value_change(callback)
}

fn format_control(
    name: &str,
    default: &str,
    callback: impl FnMut(String) + 'static,
) -> impl Widget {
    col()
        .class(format!(
            "calc-format-field {}",
            match name {
                "Font size" => "font-size-field",
                "Font family" => "font-family-field",
                _ => "advanced-format-field",
            }
        ))
        .fill((
            text().class("toolbar-label").content(name.to_owned()),
            sapid::text_field()
                .theme(crate::components::ui::theme())
                .size(sapid::Size::Small)
                .name(name.to_owned())
                .default_value(default)
                .on_value_change(callback),
        ))
}

/// Browser workbook editor. Every mutation stays in application-owned Cages.
#[allow(non_snake_case)]
pub fn Calc(scope: Scope, auth: Cage<AuthState>, id: String) -> impl Widget {
    let ed = Editor {
        book: scope.cage(Book::default()),
        sheet: scope.cage(0),
        active: scope.cage((0, 0)),
        anchor: scope.cage((0, 0)),
        end: scope.cage((0, 0)),
        extra: scope.cage(Vec::new()),
        extend: scope.cage(false),
        additive: scope.cage(false),
        filter: scope.cage(None),
        formula: scope.cage(String::new()),
        status: scope.cage("Loading document…".to_owned()),
        layout: scope.cage(0),
    };
    let loaded = scope.cage(false);
    let title = scope.cage("Spreadsheet".to_owned());
    let connected = scope.cage(ConnectionState::Connecting);
    let collaborators = scope.cage(Vec::<String>::new());
    let session = scope.cage(None::<CollabHandle>);
    let token = auth.with(|a| a.token.clone().unwrap_or_default());
    let self_id = auth.with(|a| {
        a.user
            .as_ref()
            .map(|u| u.id.to_string())
            .unwrap_or_default()
    });
    let request_id = id.clone();
    let load_token = token.clone();
    let _ = scope.spawn(async move {
        match api_get::<DocumentResponse>(&format!("/documents/{request_id}"), &load_token).await {
            Ok(doc) => {
                title.revise(|v| *v = doc.name);
                ed.book.revise(|b| b.hydrate(doc.content));
                ed.status.revise(|v| *v = "Saved".into());
                loaded.revise(|v| *v = true);
                ed.layout.revise(|v| *v += 1);
            }
            Err(error) => ed
                .status
                .revise(|v| *v = format!("Could not load document: {error}")),
        }
    });
    #[cfg(target_arch = "wasm32")]
    {
        let weak = scope.downgrade();
        let author = self_id.clone();
        let handle = connect_with_reconnect(
            id.clone(),
            token.clone(),
            move |message| {
                if weak.upgrade().is_none() {
                    return;
                }
                match message {
                    CollabMessage::Operation(payload) => {
                        let replacement =
                            if let Some((user, patch)) = cell_patch_from_operation(&payload) {
                                let mut content = ed.book.with(Book::serialize);
                                if user == author || !apply_cell_patch(&mut content, &patch) {
                                    return;
                                }
                                Some(content)
                            } else {
                                workbook_replace_from_operation(&payload)
                                    .and_then(|(user, v)| (user != author).then_some(v))
                            };
                        if let Some(content) = replacement {
                            ed.book.revise(|b| b.hydrate(content));
                            ed.layout.revise(|v| *v += 1);
                        }
                    }
                    CollabMessage::DocumentState { content, .. } if !ed.book.with(|b| b.dirty) => {
                        ed.book.revise(|b| b.hydrate(content));
                        ed.layout.revise(|v| *v += 1);
                    }
                    CollabMessage::UserJoined {
                        user_id, username, ..
                    } => collaborators.revise(|v| {
                        if !v
                            .iter()
                            .any(|entry| entry.starts_with(&format!("{user_id}:")))
                        {
                            v.push(format!("{user_id}:{username}"));
                        }
                    }),
                    CollabMessage::UserLeft { user_id } => collaborators
                        .revise(|v| v.retain(|s| !s.starts_with(&format!("{user_id}:")))),
                    CollabMessage::Error { message } => ed.status.revise(|v| *v = message),
                    _ => {}
                }
            },
            {
                let weak = scope.downgrade();
                move |state| {
                    if weak.upgrade().is_some() {
                        connected.revise(|v| *v = state)
                    }
                }
            },
        );
        session.revise(|v| *v = Some(handle.clone()));
        let _ = scope.on_cleanup(move || handle.close());
    }
    let timer = Rc::new(RefCell::new(None::<Timeout>));
    let saving = scope.cage(false);
    let save: Rc<dyn Fn()> = Rc::new({
        let weak = scope.downgrade();
        let id = id.clone();
        let token = token.clone();
        let user = self_id;
        move || {
            if !loaded.get() || saving.get() || !ed.book.with(|b| b.dirty) {
                return;
            }
            let Some(scope) = weak.upgrade() else {
                return;
            };
            let (content, revision, previous) = ed
                .book
                .with(|b| (b.serialize(), b.revision, b.content.clone()));
            let operation = if let Some(patch) = single_cell_patch(&previous, &content) {
                serde_json::json!({"type":"SetProperty","path":["workbook","sheets",patch.sheet.to_string(),"cells"],
                    "key":format!("{}:{}",patch.row,patch.col),"value":patch.new_value,"old_value":patch.old_value})
            } else {
                serde_json::json!({"type":"Replace","path":["workbook"],"old_value":previous,"new_value":content})
            };
            session.with(|handle| {
                if let Some(handle) = handle {
                    let _ = handle.send_operation(&user, operation);
                }
            });
            let id = id.clone();
            let token = token.clone();
            saving.revise(|v| *v = true);
            ed.status.revise(|v| *v = "Saving…".into());
            let _ = scope.spawn(async move {
                let req = serde_json::json!({"content":content});
                match api_put::<serde_json::Value, _>(&format!("/documents/{id}"), &req, &token)
                    .await
                {
                    Ok(_) => {
                        ed.book.revise(|b| {
                            b.content = content;
                            if b.revision == revision {
                                b.dirty = false;
                            }
                        });
                        ed.status.revise(|v| {
                            *v = if ed.book.with(|b| b.dirty) {
                                "Unsaved changes".into()
                            } else {
                                "Saved".into()
                            }
                        });
                    }
                    Err(error) => ed.status.revise(|v| *v = format!("Save failed: {error}")),
                }
                saving.revise(|v| *v = false);
            });
        }
    });
    scope.effect({
        let timer = timer.clone();
        let save = save.clone();
        move || {
            let _revision = ed.book.with(|b| b.revision);
            let busy = saving.get();
            timer.borrow_mut().take();
            if loaded.get() && ed.book.with(|b| b.dirty) && !busy {
                let save = save.clone();
                *timer.borrow_mut() = Some(Timeout::new(1500, move || save()));
            }
        }
    });
    let _ = scope.on_cleanup({
        let timer = timer.clone();
        move || {
            timer.borrow_mut().take();
        }
    });
    let formula_node = scope.cage(None);
    let address = scope.cage("A1".to_owned());
    let selection = scope.cage("A1:A1".to_owned());
    let sheet_name = scope.cage("Sheet1".to_owned());
    let find = scope.cage(String::new());
    let replace = scope.cage(String::new());
    let clipboard = scope.cage(String::new());
    let show_chart = scope.cage(false);
    let comments_open = scope.cage(false);
    let versions_open = scope.cage(false);
    let ai_open = scope.cage(false);
    let advanced_open = scope.cage(false);
    let clipboard_open = scope.cage(false);
    let find_open = scope.cage(false);
    let selection_open = scope.cage(false);
    let sheets_open = scope.cage(false);
    let metadata_name = scope.cage("Selection".to_owned());
    let threshold = scope.cage("0".to_owned());
    let filter_query = scope.cage(String::new());
    let chrome: Vec<Box<dyn Widget>> = vec![
        Box::new(
            link()
                .target("/dashboard")
                .fill(text().content("← Dashboard")),
        ),
        Box::new(text().class("calc-document-name").content(title)),
        Box::new(text().class("save-status").content(ed.status)),
        action("Save", {
            let save = save.clone();
            move || save()
        }),
        action("Comments", move || comments_open.revise(|v| *v = !*v)),
        action("Version history", move || {
            versions_open.revise(|v| *v = !*v)
        }),
        action("AI assist", move || ai_open.revise(|v| *v = !*v)),
        Box::new(text().content(scope.bond(move || format!("{:?}", connected.get())))),
        Box::new(text().content(
            scope.bond(move || collaborators.with(|v| format!("{} collaborators", v.len()))),
        )),
    ];
    let toolbar: Vec<Box<dyn Widget>> = vec![
        action("Bold", move || ed.format(|s| s.bold = !s.bold)),
        action("Italic", move || ed.format(|s| s.italic = !s.italic)),
        action("Underline", move || {
            ed.format(|s| s.underline = !s.underline)
        }),
        action("Align left", move || ed.format(|s| s.align = "left".into())),
        action("Align center", move || {
            ed.format(|s| s.align = "center".into())
        }),
        action("Align right", move || {
            ed.format(|s| s.align = "right".into())
        }),
        action("Currency", move || {
            ed.format(|s| s.number_format = "currency".into())
        }),
        action("Percent", move || {
            ed.format(|s| s.number_format = "percent".into())
        }),
        action("Merge cells", move || ed.merge(false)),
        action("Unmerge cells", move || ed.merge(true)),
        action("Insert row", move || ed.row_column(false, false)),
        action("Delete row", move || ed.row_column(false, true)),
        action("Insert column", move || ed.row_column(true, false)),
        action("Delete column", move || ed.row_column(true, true)),
        action("Sort ascending", move || ed.sort(false)),
        action("Sort descending", move || ed.sort(true)),
        action("Fill down", move || ed.fill(true)),
        action("Fill right", move || ed.fill(false)),
        action("Clear cells", move || {
            let si = ed.sheet.get();
            let cells = ed.selected();
            ed.book.revise(|b| {
                b.checkpoint();
                for (r, c) in cells {
                    b.snapshot.cells.remove(&(si, r, c));
                    b.snapshot.styles.remove(&(si, r, c));
                }
            });
            ed.formula.revise(String::clear);
        }),
        Box::new(
            sapid::copy_button()
                .theme(crate::components::ui::theme())
                .label("Copy selection")
                .source(scope.bond(move || {
                    let value = selection_text(ed);
                    if value.len() <= glory::render::MAX_CLIPBOARD_BYTES {
                        value
                    } else {
                        String::new()
                    }
                }))
                .enabled(
                    scope.bond(move || {
                        selection_text(ed).len() <= glory::render::MAX_CLIPBOARD_BYTES
                    }),
                )
                .on_result(move |outcome| {
                    clipboard.revise(|v| *v = selection_text(ed));
                    ed.status.revise(|v| {
                        *v = match outcome {
                            glory::render::ClipboardOutcome::Written => "Selection copied".into(),
                            _ => "Clipboard unavailable; use the editable selection data field"
                                .into(),
                        }
                    });
                }),
        ),
        action("Paste selection", move || {
            let si = ed.sheet.get();
            let (r, c) = ed.active.get();
            let value = clipboard.get();
            ed.book.revise(|b| {
                b.checkpoint();
                for (dr, line) in value.lines().enumerate() {
                    for (dc, v) in line.split('\t').enumerate() {
                        if r + (dr as u32) < NUM_ROWS && c + (dc as u32) < NUM_COLS {
                            b.snapshot
                                .cells
                                .insert((si, r + dr as u32, c + dc as u32), v.into());
                        }
                    }
                }
            });
        }),
        action("Chart selection", move || {
            ed.metadata(|content, si, range| {
                append_array_entry(
                    &mut content["sheets"][si],
                    "charts",
                    serde_json::json!({
                        "id":uuid::Uuid::new_v4(),"title":"Chart","chart_type":"Bar",
                        "data_start":{"row":range.start.0,"col":range.start.1},
                        "data_end":{"row":range.end.0,"col":range.end.1},
                        "anchor_start":{"row":range.start.0,"col":range.end.1+2},
                        "anchor_end":{"row":range.start.0+12,"col":range.end.1+10}
                    }),
                );
            });
            show_chart.revise(|v| *v = !*v);
        }),
        action("All borders", move || {
            ed.format(|s| {
                s.borders = CellBorders {
                    top: true,
                    right: true,
                    bottom: true,
                    left: true,
                }
            })
        }),
        action("Clear borders", move || {
            ed.format(|s| s.borders = CellBorders::default())
        }),
    ];
    let mut formats: Vec<Box<dyn Widget>> = vec![
        Box::new(format_control("Font size", "14", move |v| {
            if v.parse::<f64>().is_ok_and(f64::is_finite) {
                ed.format(|s| s.font_size = v.clone());
            }
        })),
        Box::new(format_control("Font family", "sans-serif", move |v| {
            ed.format(|s| s.font_family = v.clone())
        })),
        Box::new(format_control(
            "Text color (#rrggbb)",
            "#172033",
            move |v| {
                if color(&v).is_some() {
                    ed.format(|s| s.text_color = v.clone());
                }
            },
        )),
        Box::new(format_control(
            "Fill color (#rrggbb)",
            "#ffffff",
            move |v| {
                if color(&v).is_some() {
                    ed.format(|s| s.fill_color = v.clone());
                }
            },
        )),
        Box::new(format_control("Number format", "general", move |v| {
            ed.format(|s| s.number_format = v.clone())
        })),
        Box::new(format_control("Column width", "80", move |value| {
            let Some(width) = value
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && (24.0..=800.0).contains(v))
            else {
                return;
            };
            let col = ed.active.get().1.to_string();
            ed.metadata(|content, si, _| {
                if !content["sheets"][si]["column_widths"].is_object() {
                    content["sheets"][si]["column_widths"] = serde_json::json!({});
                }
                content["sheets"][si]["column_widths"][col] = serde_json::json!(width);
            });
        })),
        Box::new(format_control("Row height", "24", move |value| {
            let Some(height) = value
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && (18.0..=400.0).contains(v))
            else {
                return;
            };
            let row = ed.active.get().0.to_string();
            ed.metadata(|content, si, _| {
                if !content["sheets"][si]["row_heights"].is_object() {
                    content["sheets"][si]["row_heights"] = serde_json::json!({});
                }
                content["sheets"][si]["row_heights"][row] = serde_json::json!(height);
            });
        })),
    ];
    let extra_formats = formats.split_off(2);
    let export: Vec<Box<dyn Widget>> = ["xlsx", "csv", "pdf", "ods"]
        .into_iter()
        .map(|format| {
            let id = id.clone();
            let token = token.clone();
            let weak = scope.downgrade();
            action(&format!("Export {format}"), move || {
                let Some(scope) = weak.upgrade() else {
                    return;
                };
                let id = id.clone();
                let token = token.clone();
                let _ = scope.spawn(async move {
                    if let Err(error) = export_document(&id, format, &token).await {
                        ed.status.revise(|v| *v = error);
                    }
                });
            })
        })
        .collect();
    let file_save = save.clone();
    let menu_bar = row().class("menubar calc-menubar").fill((
        sapid::dropdown_menu("calc-file")
            .theme(crate::components::ui::theme())
            .label(crate::i18n::t("menubar.file"))
            .item(
                sapid::menu_item("save")
                    .label(crate::i18n::t("menu.file.save"))
                    .on_activate(move || file_save()),
            )
            .item(
                sapid::menu_item("open")
                    .label(crate::i18n::t("menu.file.open"))
                    .on_activate(|| crate::components::ui::go("/files")),
            )
            .item(
                sapid::menu_item("print")
                    .label(crate::i18n::t("menu.file.print"))
                    .on_activate(|| {
                        #[cfg(target_arch = "wasm32")]
                        if let Some(window) = web_sys::window() {
                            let _ = window.print();
                        }
                    }),
            )
            .item(
                sapid::menu_item("close")
                    .label(crate::i18n::t("menu.file.close"))
                    .on_activate(|| crate::components::ui::go("/dashboard")),
            ),
        sapid::dropdown_menu("calc-edit")
            .theme(crate::components::ui::theme())
            .label(crate::i18n::t("menubar.edit"))
            .item(
                sapid::menu_item("undo")
                    .label(crate::i18n::t("menu.edit.undo"))
                    .on_activate(move || ed.history(false)),
            )
            .item(
                sapid::menu_item("redo")
                    .label(crate::i18n::t("menu.edit.redo"))
                    .on_activate(move || ed.history(true)),
            )
            .item(
                sapid::menu_item("clipboard")
                    .label("Clipboard")
                    .on_activate(move || clipboard_open.revise(|value| *value = !*value)),
            )
            .item(
                sapid::menu_item("find")
                    .label(crate::i18n::t("menu.edit.find"))
                    .on_activate(move || find_open.revise(|value| *value = !*value)),
            ),
        sapid::dropdown_menu("calc-view")
            .theme(crate::components::ui::theme())
            .label(crate::i18n::t("menubar.view"))
            .item(
                sapid::menu_item("selection-tools")
                    .label("Selection tools")
                    .on_activate(move || selection_open.revise(|value| *value = !*value)),
            )
            .item(
                sapid::menu_item("sheet-tools")
                    .label("Sheet tools")
                    .on_activate(move || sheets_open.revise(|value| *value = !*value)),
            )
            .item(
                sapid::menu_item("freeze-rows")
                    .label(crate::i18n::t("menu.view.freeze_rows"))
                    .on_activate(move || {
                        ed.metadata(|content, si, range| {
                            let frozen = content["sheets"][si]["frozen_rows"].as_u64().unwrap_or(0);
                            content["sheets"][si]["frozen_rows"] =
                                serde_json::json!(if frozen > 0 { 0 } else { range.start.0 + 1 });
                        })
                    }),
            )
            .item(
                sapid::menu_item("freeze-cols")
                    .label(crate::i18n::t("menu.view.freeze_cols"))
                    .on_activate(move || {
                        ed.metadata(|content, si, range| {
                            let frozen = content["sheets"][si]["frozen_cols"].as_u64().unwrap_or(0);
                            content["sheets"][si]["frozen_cols"] =
                                serde_json::json!(if frozen > 0 { 0 } else { range.start.1 + 1 });
                        })
                    }),
            ),
        sapid::dropdown_menu("calc-insert")
            .theme(crate::components::ui::theme())
            .label(crate::i18n::t("menubar.insert"))
            .item(
                sapid::menu_item("row")
                    .label(crate::i18n::t("menu.insert.row_above"))
                    .on_activate(move || ed.row_column(false, false)),
            )
            .item(
                sapid::menu_item("column")
                    .label(crate::i18n::t("menu.insert.col_left"))
                    .on_activate(move || ed.row_column(true, false)),
            )
            .item(
                sapid::menu_item("sheet")
                    .label(crate::i18n::t("menu.insert.sheet"))
                    .on_activate(move || {
                        ed.book.revise(|book| {
                            book.checkpoint();
                            let n = book.snapshot.sheets.len() + 1;
                            book.snapshot.sheets.push(format!("Sheet{n}"));
                        })
                    }),
            ),
        sapid::dropdown_menu("calc-format")
            .theme(crate::components::ui::theme())
            .label(crate::i18n::t("menubar.format"))
            .item(
                sapid::menu_item("bold")
                    .label("Bold")
                    .on_activate(move || ed.format(|style| style.bold = !style.bold)),
            )
            .item(
                sapid::menu_item("italic")
                    .label("Italic")
                    .on_activate(move || ed.format(|style| style.italic = !style.italic)),
            )
            .item(
                sapid::menu_item("merge")
                    .label("Merge cells")
                    .on_activate(move || ed.merge(false)),
            )
            .item(
                sapid::menu_item("unmerge")
                    .label("Unmerge cells")
                    .on_activate(move || ed.merge(true)),
            ),
        sapid::dropdown_menu("calc-tools")
            .theme(crate::components::ui::theme())
            .label(crate::i18n::t("menubar.tools"))
            .item(
                sapid::menu_item("data")
                    .label("Data tools")
                    .on_activate(move || advanced_open.revise(|value| *value = !*value)),
            )
            .item(
                sapid::menu_item("comments")
                    .label("Comments")
                    .on_activate(move || comments_open.revise(|value| *value = !*value)),
            )
            .item(
                sapid::menu_item("history")
                    .label("Version history")
                    .on_activate(move || versions_open.revise(|value| *value = !*value)),
            )
            .item(
                sapid::menu_item("ai")
                    .label("AI assist")
                    .on_activate(move || ai_open.revise(|value| *value = !*value)),
            ),
    ));
    col().class("editor-layout calc-editor")
        .fill(crate::components::navbar::Navbar(scope.clone(),auth))
        .fill(row().class("editor-navbar").fill(chrome))
        .fill(menu_bar)
        .fill((
        row().class("calc-ribbon").fill((row().class("calc-format-toolbar").fill(formats),
        row().class("calc-toolbar").fill(toolbar))),
        row().class("metadata-toolbar").style(advanced_open.map(|open|Style::new().display(if *open { Display::Flex } else { Display::None }))).fill((
            row().class("calc-advanced-formats").fill(extra_formats),
            control("Named range name",metadata_name,move|v|metadata_name.revise(|s|*s=v)),
            sapid::button().theme(crate::components::ui::theme()).label("Define named range").on_press(move||{
                let name=metadata_name.get();if name.trim().is_empty(){return;}
                ed.metadata(|content,si,range|{
                    let sheet_id=content["sheets"][si]["id"].clone();
                    append_array_entry(content,"named_ranges",serde_json::json!({"name":name,"sheet_id":sheet_id,
                        "start":{"row":range.start.0,"col":range.start.1},"end":{"row":range.end.0,"col":range.end.1}}));
                });
            }),
            control("Conditional threshold",threshold,move|v|threshold.revise(|s|*s=v)),
            sapid::button().theme(crate::components::ui::theme()).label("Highlight greater than").on_press(move||{
                let Ok(limit)=threshold.get().parse::<f64>()else{return;};
                ed.metadata(|content,si,range|{
                    let format=style_to_json(&CellStyle{fill_color:"#fff3bf".into(),..Default::default()});
                    append_array_entry(&mut content["sheets"][si],"conditional_formats",serde_json::json!({
                        "range_start":{"row":range.start.0,"col":range.start.1},"range_end":{"row":range.end.0,"col":range.end.1},
                        "rule":{"GreaterThan":limit},"format":format
                    }));
                });
            }),
            sapid::button().theme(crate::components::ui::theme()).label("Validate numbers only").on_press(move||ed.metadata(|content,si,range|{
                append_array_entry(&mut content["sheets"][si],"data_validations",serde_json::json!({
                    "range_start":{"row":range.start.0,"col":range.start.1},"range_end":{"row":range.end.0,"col":range.end.1},
                    "rule":{"DecimalBetween":[-1000000000000000.0,1000000000000000.0]},"error_message":"Enter a number"
                }));
            })),
            sapid::button().theme(crate::components::ui::theme()).label("Freeze rows").on_press(move||ed.metadata(|content,si,range|{
                let current=content["sheets"][si]["frozen_rows"].as_u64().unwrap_or(0);
                content["sheets"][si]["frozen_rows"]=serde_json::json!(if current>0{0}else{range.start.0+1});
            })),
            sapid::button().theme(crate::components::ui::theme()).label("Freeze columns").on_press(move||ed.metadata(|content,si,range|{
                let current=content["sheets"][si]["frozen_cols"].as_u64().unwrap_or(0);
                content["sheets"][si]["frozen_cols"]=serde_json::json!(if current>0{0}else{range.start.1+1});
            })),
            sapid::button().theme(crate::components::ui::theme()).label("Pivot summary").on_press(move||{
                let si=ed.sheet.get();let range=MergedRange{start:ed.anchor.get(),end:ed.end.get()}.normalized();
                ed.book.revise(|b|{
                    b.checkpoint();b.content=b.serialize();
                    let target=b.snapshot.sheets.len();let name=format!("Pivot{target}");
                    let values=pivot_sum_rows(&b.snapshot.cells,si,range.start,range.end);
                    b.snapshot.sheets.push(name.clone());
                    b.snapshot.cells.insert((target,0,0),"Group".into());b.snapshot.cells.insert((target,0,1),"Sum".into());
                    for(i,(label,value))in values.into_iter().take((NUM_ROWS-1)as usize).enumerate(){
                        b.snapshot.cells.insert((target,i as u32+1,0),label);b.snapshot.cells.insert((target,i as u32+1,1),value.to_string());
                    }
                    let target_id=uuid::Uuid::new_v4();let source=b.content["sheets"][si]["id"].clone();
                    append_array_entry(&mut b.content,"sheets",serde_json::json!({"id":target_id,"name":name,"cells":[]}));
                    append_array_entry(&mut b.content,"pivot_tables",serde_json::json!({"id":uuid::Uuid::new_v4(),"name":name,
                        "source_sheet_id":source,"source_start":{"row":range.start.0,"col":range.start.1},"source_end":{"row":range.end.0,"col":range.end.1},
                        "target_sheet_id":target_id,"target":{"row":0,"col":0},"row_field_col":range.start.1,
                        "value_field_col":(range.start.1+1).min(range.end.1),"aggregation":"Sum"}));
                    ed.sheet.revise(|v|*v=target);
                });ed.activate((0,0));ed.layout.revise(|v|*v+=1);
            }),
        )),
        row().class("formula-bar").fill((
            control("Cell address or named range", address, move |v| {
                address.revise(|s| *s = v)
            }),
            sapid::button().theme(crate::components::ui::theme()).name("Go to cell").label("↵").on_press(move || {
                let address = address.get();
                if let Some(point) =
                    parse_cell_ref(&address).filter(|p| p.0 < NUM_ROWS && p.1 < NUM_COLS)
                {
                    ed.activate(point);
                } else if let Some((si, range)) =
                    ed.book.with(|b| resolve_named_range(&b.content, &address))
                {
                    ed.sheet.revise(|v| *v = si);
                    ed.activate(range.start);
                    ed.end.revise(|v| *v = range.end);
                    ed.layout.revise(|v| *v += 1);
                } else {
                    ed.status
                        .revise(|v| *v = "Unknown cell address or named range".into());
                }
            }),
            sapid::button().theme(crate::components::ui::theme())
                .class("formula-icon")
                .label("f(x)")
                .on_press({
                    let weak = scope.downgrade();
                    move || {
                        ed.formula.revise(|v| {
                            if !v.starts_with('=') {
                                v.insert(0, '=');
                            }
                        });
                        if let (Some(scope), Some(node)) = (weak.upgrade(), formula_node.get()) {
                            let _ = scope.action(glory::render::Action::Focus(node));
                        }
                    }
                }),
            input()
                .name("Formula")
                .class("calc-formula-input formula-input")
                .value(ed.formula)
                .node_ref(formula_node)
                .semantics(Some(Box::new(Semantics {
                    id: Some(SemanticId::new("calc-formula-input").expect("static id")),
                    ..Default::default()
                })))
                .on(EventKind::ValueChanged, move |event| {
                    if let glory::render::Event::ValueChanged { value, .. } = event {
                        ed.formula.revise(|v| *v = value);
                    }
                })
                .on(EventKind::Commit, move |event| {
                    if let glory::render::Event::Commit { value, .. } = event {
                        ed.commit(value);
                    }
                })
                .on(EventKind::Keyboard, move |event| {
                    if let glory::render::Event::Keyboard { command, .. } = event {
                        match command {
                            glory::render::KeyboardCommand::Submit => {
                                ed.commit(ed.formula.get());
                                let (r, c) = ed.active.get();
                                ed.activate(((r + 1).min(NUM_ROWS - 1), c));
                            }
                            glory::render::KeyboardCommand::Dismiss => ed.activate(ed.active.get()),
                            _ => {}
                        }
                    }
                }),
            sapid::button().theme(crate::components::ui::theme())
                .name("Apply formula").label("✓")
                .on_press(move || ed.commit(ed.formula.get())),
        )),
        row().class("clipboard-toolbar").style(clipboard_open.map(|open|Style::new().display(if *open { Display::Flex } else { Display::None }))).fill((
            sapid::text_field().theme(crate::components::ui::theme()).name("Selection data (tab separated)").multiline(true).value(clipboard)
                .on_value_change(move|value|clipboard.revise(|v|*v=value)),
            sapid::button().theme(crate::components::ui::theme()).label("Load selection data").on_press(move||clipboard.revise(|v|*v=selection_text(ed))),
        )),
        row().class("selection-toolbar").style(selection_open.map(|open|Style::new().display(if *open { Display::Flex } else { Display::None }))).fill((
            control("Selection range", selection, move |v| {
                selection.revise(|s| *s = v)
            }),
            sapid::button().theme(crate::components::ui::theme()).label("Select range").on_press(move || {
                let value = selection.get();
                let mut parts = value.split(':');
                if let Some(a) = parts.next().and_then(parse_cell_ref) {
                    let z = parts.next().and_then(parse_cell_ref).unwrap_or(a);
                    if a.0 < NUM_ROWS && z.0 < NUM_ROWS && a.1 < NUM_COLS && z.1 < NUM_COLS {
                        ed.activate(a);
                        ed.end.revise(|v| *v = z);
                    }
                }
            }),
            sapid::checkbox().theme(crate::components::ui::theme())
                .label("Extend selection")
                .checked(ed.extend)
                .on_checked_change({
                    move |value| {
                        ed.extend.revise(|v| *v = value);
                    }
                }),
            sapid::checkbox().theme(crate::components::ui::theme()).label("Add cells to selection").checked(ed.additive)
                .on_checked_change(move|value|ed.additive.revise(|v|*v=value)),
            control("Column filter",filter_query,move|value|filter_query.revise(|v|*v=value)),
            sapid::button().theme(crate::components::ui::theme()).label("Filter column").on_press(move||{
                if ed.book.with(|b|b.snapshot.merged_cells.get(&ed.sheet.get()).is_some_and(|v|!v.is_empty())) {
                    ed.status.revise(|v|*v="Unmerge cells before filtering rows".into());return;
                }
                ed.filter.revise(|v|*v=Some((ed.active.get().1,filter_query.get())));ed.layout.revise(|v|*v+=1);
            }),
            sapid::button().theme(crate::components::ui::theme()).label("Clear filter").on_press(move||{ed.filter.revise(|v|*v=None);ed.layout.revise(|v|*v+=1);}),
        )),
        when(loaded, {
            let weak = scope.downgrade();
            move || {
                let weak = weak.clone();
                each(scope_bond(&weak, move || vec![ed.layout.get()]))
                    .key(|generation| *generation)
                    .children(move |_| {
                        widget(move|scope| grid(scope,ed).build(scope))
                    })
            }
        }),
        row().class("sheet-tabs").fill((
            each(
                scope.bond(move || {
                    ed.book.with(|b| {
                        b.snapshot
                            .sheets
                            .iter()
                            .enumerate()
                            .map(|(i, n)| (i, n.clone()))
                            .collect::<Vec<_>>()
                    })
                }))
                .key(|(i, _)| *i)
                .children(move |row| {
                    let sheet = row.value;
                    let index = sheet.get().0;
                    sapid::button().theme(crate::components::ui::theme())
                        .variant(sapid::ButtonVariant::Secondary).size(sapid::Size::Small)
                        .class(ed.sheet.map(move |active|if *active==index { "sheet-tab active".to_string() } else { "sheet-tab".to_string() }))
                        .label(scope_label(sheet)).on_press(move || {
                        ed.sheet.revise(|v| *v = index);
                        ed.activate((0, 0));
                        ed.layout.revise(|v| *v += 1);
                    })
                }),
            sapid::button().theme(crate::components::ui::theme()).name("Add sheet").label("+").class("sheet-tab-add").on_press(move || {
                ed.book.revise(|b| {
                    b.checkpoint();
                    let count = b.snapshot.sheets.len();
                    b.snapshot.sheets.push(format!("Sheet{}", count + 1));
                });
            }),
            row().class("sheet-management").style(sheets_open.map(|open|Style::new().display(if *open { Display::Flex } else { Display::None }))).fill((
            sapid::button().theme(crate::components::ui::theme()).label("Duplicate sheet").on_press(move||ed.duplicate_sheet()),
            sapid::button().theme(crate::components::ui::theme()).label("Move sheet left").on_press(move||ed.move_sheet(false)),
            sapid::button().theme(crate::components::ui::theme()).label("Move sheet right").on_press(move||ed.move_sheet(true)),
            control("Sheet name", sheet_name, move |v| {
                sheet_name.revise(|n| *n = v)
            }),
            sapid::button().theme(crate::components::ui::theme()).label("Rename sheet").on_press(move || {
                let si = ed.sheet.get();
                let name = sheet_name.get();
                if !name.trim().is_empty() {
                    ed.book.revise(|b| {
                        b.checkpoint();
                        b.snapshot.sheets[si] = name;
                    });
                }
            }),
            sapid::button().theme(crate::components::ui::theme()).label("Delete sheet").on_press(move || {
                let si = ed.sheet.get();
                ed.book.revise(|b| {
                    if b.snapshot.sheets.len() < 2 {
                        return;
                    }
                    b.checkpoint();
                    b.snapshot.sheets.remove(si);
                    b.snapshot.cells = std::mem::take(&mut b.snapshot.cells)
                        .into_iter()
                        .filter_map(|((s, r, c), v)| {
                            (s != si).then_some(((if s > si { s - 1 } else { s }, r, c), v))
                        })
                        .collect();
                    b.snapshot.styles = std::mem::take(&mut b.snapshot.styles)
                        .into_iter()
                        .filter_map(|((s, r, c), v)| {
                            (s != si).then_some(((if s > si { s - 1 } else { s }, r, c), v))
                        })
                        .collect();
                    delete_sheet_from_merged(&mut b.snapshot.merged_cells, si);
                    if let Some(sheets) = b.content.get_mut("sheets").and_then(|s| s.as_array_mut())
                        && si < sheets.len()
                    {
                        sheets.remove(si);
                    }
                });
                ed.sheet.revise(|v| *v = 0);
                ed.activate((0, 0));
                ed.layout.revise(|v| *v += 1);
            }),)),
        )),
        row().class("find-toolbar").style(find_open.map(|open|Style::new().display(if *open { Display::Flex } else { Display::None }))).fill((
            control("Find", find, move |v| find.revise(|s| *s = v)),
            control("Replace with", replace, move |v| replace.revise(|s| *s = v)),
            sapid::button().theme(crate::components::ui::theme()).label("Find next").on_press(move || {
                let query = find.get();
                if query.is_empty() {
                    return;
                }
                let si = ed.sheet.get();
                let active = ed.active.get();
                let mut found = ed.book.with(|b| {
                    b.snapshot
                        .cells
                        .iter()
                        .filter(|((s, _, _), v)| *s == si && v.contains(&query))
                        .map(|((_, r, c), _)| (*r, *c))
                        .collect::<Vec<_>>()
                });
                found.sort_unstable();
                if let Some(point) = found.iter().find(|p| **p > active).or(found.first()) {
                    ed.activate(*point);
                } else {
                    ed.status.revise(|v| *v = "No matches".into());
                }
            }),
            sapid::button().theme(crate::components::ui::theme()).label("Replace all").on_press(move || {
                let query = find.get();
                let replacement = replace.get();
                if !query.is_empty() {
                    ed.book.revise(|b| {
                        b.checkpoint();
                        for value in b.snapshot.cells.values_mut() {
                            *value = value.replace(&query, &replacement);
                        }
                    });
                }
            }),
        )),
        when(show_chart, move || widget(move|scope|chart_view(scope,ed).build(scope))),
        group().fill((crate::components::comments_pane::CommentsPane(scope.clone(),auth,id.clone(),comments_open),
        crate::components::version_history::VersionHistory(scope.clone(),auth,id.clone(),versions_open),
        when(ai_open,{let id=id.clone();move||{let id=id.clone();widget(move|scope|crate::components::ai_assist::AiAssist(scope.clone(),auth,id.clone()).build(scope))}}),)),
        row().class("status-bar").fill((
            text().content(scope.bond(move || {
                let (r, c) = ed.active.get();
                let si = ed.sheet.get();
                let selected = ed.selected();
                ed.book.with(|b| {
                    let values: Vec<_> = selected
                        .iter()
                        .filter_map(|(r, c)| {
                            b.snapshot
                                .cells
                                .get(&(si, *r, *c))
                                .and_then(|v| evaluated_numeric(v, si, &b.snapshot.cells))
                        })
                        .collect();
                    let sum: f64 = values.iter().sum();
                    let avg = if values.is_empty() {
                        0.0
                    } else {
                        sum / values.len() as f64
                    };
                    format!(
                        "Cell: {} · Sum: {} · Average: {} · Count: {}",
                        cell_ref(r, c),
                        format_number_default(sum),
                        format_number_default(avg),
                        values.len()
                    )
                })
            })),
            row().fill(export),
        )),
    ))
}

fn scope_bond<T: 'static>(weak: &glory::ui::WeakScope, read: impl Fn() -> T + 'static) -> Bond<T> {
    weak.upgrade().expect("live workbook scope").bond(read)
}
fn scope_label(sheet: Cage<(usize, String)>) -> Vary<String> {
    // Each owns this Cage; read through its runtime owner for live tab renames.
    sheet.map(|(_, name)| name.clone()).into()
}

fn selection_text(ed: Editor) -> String {
    let si = ed.sheet.get();
    let range = MergedRange {
        start: ed.anchor.get(),
        end: ed.end.get(),
    }
    .normalized();
    ed.book.with(|b| {
        (range.start.0..=range.end.0)
            .map(|r| {
                (range.start.1..=range.end.1)
                    .map(|c| {
                        b.snapshot
                            .cells
                            .get(&(si, r, c))
                            .cloned()
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn color(value: &str) -> Option<Color> {
    let value = value.strip_prefix('#')?;
    if value.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(value, 16).ok()?;
    Some(Color((n >> 16) as u8, (n >> 8) as u8, n as u8, 255))
}
fn cell_style(style: &CellStyle) -> Style {
    let mut out = Style::new()
        .width(Length::Percent(100.0))
        .min_height(Length::Px(24.0));
    if style.bold {
        out.font_weight = Some(700.0);
    }
    if let Some(value) = color(&style.text_color) {
        out.color = Some(value);
    }
    if let Some(value) = color(&style.fill_color) {
        out.background = Some(value);
    }
    if let Some(value) = style
        .font_size
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
    {
        out.font_size = Some(value.clamp(6.0, 96.0));
    }
    if !style.font_family.is_empty() {
        out.font_family = Some(vec![Family::Named(style.font_family.clone())]);
    }
    out
}
fn grid(scope: &Scope, ed: Editor) -> impl Widget {
    let si = ed.sheet.get();
    let ranges = ed.book.with(|b| {
        b.snapshot
            .merged_cells
            .get(&si)
            .cloned()
            .unwrap_or_default()
    });
    let mut head = sapid::table_row("head").cell(
        sapid::table_header_cell("corner")
            .class("corner-cell row-header")
            .text(""),
    );
    for c in 0..NUM_COLS {
        head = head.cell(
            sapid::table_header_cell(format!("h{c}"))
                .class("col-header")
                .text(col_letter(c).to_string()),
        );
    }
    let mut nodes = HashMap::new();
    let mut body = sapid::table_body();
    for r in 0..NUM_ROWS {
        if r > 0
            && ed.filter.with(|filter| {
                filter.as_ref().is_some_and(|(col, query)| {
                    ed.book.with(|b| {
                        !b.snapshot
                            .cells
                            .get(&(si, r, *col))
                            .is_some_and(|v| v.to_lowercase().contains(&query.to_lowercase()))
                    })
                })
            })
        {
            continue;
        }
        let mut row = sapid::table_row(format!("r{r}")).cell(
            sapid::table_row_header(format!("rh{r}"))
                .class("row-header")
                .text((r + 1).to_string()),
        );
        for c in 0..NUM_COLS {
            let merged = ranges.iter().find(|range| range.contains((r, c)));
            if merged.is_some_and(|range| range.start != (r, c)) {
                continue;
            }
            let node = scope.cage(None);
            nodes.insert((r, c), node);
            let name = cell_ref(r, c);
            let cell_scope = scope.downgrade();
            let mut cell = sapid::table_cell(format!("c{c}"))
                .style(scope.bond(move || {
                    ed.book.with(|b| {
                        let sheet = &b.content["sheets"][si];
                        let width = sheet["column_widths"][c.to_string()]
                            .as_f64()
                            .unwrap_or(80.0);
                        let height = sheet["row_heights"][r.to_string()].as_f64().unwrap_or(24.0);
                        let mut style = Style::new()
                            .width(Length::Px(width.clamp(24.0, 800.0)))
                            .height(Length::Px(height.clamp(18.0, 400.0)));
                        if (r as u64) < sheet["frozen_rows"].as_u64().unwrap_or(0) {
                            style.position = Some(Position::Sticky);
                            style.top = Some(Length::Px((r + 1) as f64 * 24.0));
                        }
                        if (c as u64) < sheet["frozen_cols"].as_u64().unwrap_or(0) {
                            style.position = Some(Position::Sticky);
                            style.left = Some(Length::Px(c as f64 * 80.0 + 80.0));
                        }
                        style
                    })
                }))
                .class(scope.bond(move || {
                    let active = ed.active.get() == (r, c);
                    let selected = in_selection((r, c), ed.anchor.get(), ed.end.get())
                        || ed.extra.with(|v| v.contains(&(r, c)));
                    let extra = ed.book.with(|b| {
                        let style = b
                            .snapshot
                            .styles
                            .get(&(si, r, c))
                            .cloned()
                            .unwrap_or_default();
                        format!(
                            "{}{}{}{}{}{}{}{}",
                            if style.bold { " cell-bold" } else { "" },
                            if style.italic { " cell-italic" } else { "" },
                            if style.underline {
                                " cell-underline"
                            } else {
                                ""
                            },
                            if style.borders.top { " border-t" } else { "" },
                            if style.borders.right { " border-r" } else { "" },
                            if style.borders.bottom {
                                " border-b"
                            } else {
                                ""
                            },
                            if style.borders.left { " border-l" } else { "" },
                            match style.align.as_str() {
                                "left" => " cell-align-left",
                                "center" => " cell-align-center",
                                "right" => " cell-align-right",
                                _ => "",
                            }
                        )
                    });
                    format!(
                        "cell{}{}{}",
                        if active { " active" } else { "" },
                        if selected { " selected" } else { "" },
                        extra
                    )
                }))
                .content(move || {
                    let scope = cell_scope.upgrade().expect("live spreadsheet cell");
                    let value = scope.bond(move || {
                        ed.book.with(|b| {
                            let raw = b
                                .snapshot
                                .cells
                                .get(&(si, r, c))
                                .map(String::as_str)
                                .unwrap_or("");
                            let value = evaluate_cell_display(raw, si, &b.snapshot.cells);
                            let style = b
                                .snapshot
                                .styles
                                .get(&(si, r, c))
                                .cloned()
                                .unwrap_or_default();
                            format_number(&value, &style.number_format)
                        })
                    });
                    glory::ui::button()
                        .label(value)
                        .name(name.clone())
                        .class("cell-button")
                        .node_ref(node)
                        .style(scope.bond(move || {
                            ed.book.with(|b| {
                                let base = b
                                    .snapshot
                                    .styles
                                    .get(&(si, r, c))
                                    .cloned()
                                    .unwrap_or_default();
                                let raw = b
                                    .snapshot
                                    .cells
                                    .get(&(si, r, c))
                                    .map(String::as_str)
                                    .unwrap_or("");
                                let style = conditional_style_for(&b.content, si, r, c, raw)
                                    .unwrap_or(base);
                                cell_style(&style)
                            })
                        }))
                        .on(EventKind::Activate, move |_| {
                            if ed.extend.get() {
                                ed.end.revise(|v| *v = (r, c));
                                ed.active.revise(|v| *v = (r, c));
                            } else if ed.additive.get() {
                                let old = ed.active.get();
                                ed.extra.revise(|v| {
                                    if !v.contains(&old) {
                                        v.push(old)
                                    }
                                });
                                ed.active.revise(|v| *v = (r, c));
                            } else {
                                ed.activate((r, c));
                            }
                            let (row, col) = ed.active.get();
                            let value = ed.book.with(|b| {
                                b.snapshot
                                    .cells
                                    .get(&(si, row, col))
                                    .cloned()
                                    .unwrap_or_default()
                            });
                            ed.formula.revise(|v| *v = value);
                        })
                });
            if let Some(range) = merged {
                cell = cell
                    .row_span(range.row_span() as u16)
                    .column_span(range.col_span() as u16);
            }
            row = row.cell(cell);
        }
        body = body.row(row);
    }
    let weak = scope.downgrade();
    view()
        .role(Role::Grid)
        .semantics(Some(Box::new(Semantics {
            grid_navigation: Some(glory::render::Direction::Ltr),
            grid_multiple: true,
            ..Default::default()
        })))
        .on(EventKind::GridNavigate, move |event| {
            if let glory::render::Event::GridNavigate { command, .. } = event {
                let (r, c) = ed.active.get();
                let target = match command {
                    GridCommand::Left => (r, c.saturating_sub(1)),
                    GridCommand::Right => (r, (c + 1).min(NUM_COLS - 1)),
                    GridCommand::Up => (r.saturating_sub(1), c),
                    GridCommand::Down => ((r + 1).min(NUM_ROWS - 1), c),
                    GridCommand::FirstInRow => (r, 0),
                    GridCommand::LastInRow => (r, NUM_COLS - 1),
                    GridCommand::PreviousPage => (r.saturating_sub(10), c),
                    GridCommand::NextPage => ((r + 10).min(NUM_ROWS - 1), c),
                    GridCommand::PreviousLargePage => (0, c),
                    GridCommand::NextLargePage => (NUM_ROWS - 1, c),
                };
                ed.activate(target);
                if let (Some(scope), Some(node)) = (
                    weak.upgrade(),
                    nodes.get(&ed.active.get()).and_then(|n| n.get()),
                ) {
                    let _ = scope.action(glory::render::Action::Focus(node));
                }
            }
        })
        .class("sheet-scroll spreadsheet")
        .style(Style::new().overflow_x(Overflow::Auto))
        .fill(
            sapid::table("workbook")
                .theme(crate::components::ui::theme())
                .class("sheet-grid")
                .name("Spreadsheet")
                .bordered(true)
                .head(sapid::table_head().row(head))
                .body(body),
        )
}
fn chart_view(scope: &Scope, ed: Editor) -> impl Widget {
    let data = scope.bond(move || {
        let range = MergedRange {
            start: ed.anchor.get(),
            end: ed.end.get(),
        }
        .normalized();
        let si = ed.sheet.get();
        ed.book.with(|b| {
            let points: Vec<_> = (range.start.0..=range.end.0)
                .take(128)
                .map(|r| {
                    let label = b
                        .snapshot
                        .cells
                        .get(&(si, r, range.start.1))
                        .cloned()
                        .unwrap_or_else(|| (r + 1).to_string());
                    let label: String = label
                        .replace(['\n', '\r', '\0'], " ")
                        .chars()
                        .scan(0usize, |bytes, ch| {
                            *bytes += ch.len_utf8();
                            (*bytes <= 256).then_some(ch)
                        })
                        .collect();
                    let value = b
                        .snapshot
                        .cells
                        .get(&(si, r, range.end.1))
                        .and_then(|v| evaluated_numeric(v, si, &b.snapshot.cells))
                        .filter(|value| {
                            value.is_finite() && value.abs() <= sapid::charts::MAX_CHART_VALUE
                        });
                    (label, value)
                })
                .collect();
            sapid::charts::ChartData {
                categories: points
                    .iter()
                    .enumerate()
                    .map(|(i, (label, _))| {
                        sapid::charts::ChartCategory::new(
                            format!("r{i}"),
                            if label.is_empty() {
                                "Row".into()
                            } else {
                                label.clone()
                            },
                        )
                    })
                    .collect(),
                series: vec![sapid::charts::ChartSeries::new(
                    "values",
                    "Values",
                    points.into_iter().map(|(_, v)| v),
                )],
            }
        })
    });
    sapid::charts::bar_chart("selection-chart")
        .theme(crate::components::ui::theme())
        .name("Selection chart")
        .data(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workbook_save_preserves_ids_metadata_and_format_only_cells() {
        let mut book = Book::default();
        book.hydrate(serde_json::json!({
            "name":"Budget", "named_ranges":[{"name":"Revenue"}],
            "sheets":[{"id":"stable-sheet", "name":"Sheet1", "cells":[],
                "column_widths":{"0":180}, "frozen_rows":2,
                "conditional_formats":[{"rule":"IsEmpty"}], "charts":[{"title":"Sales"}]}]
        }));
        book.snapshot.cells.insert((0, 0, 0), "=SUM(A2:A3)".into());
        book.snapshot.styles.insert(
            (0, 3, 2),
            CellStyle {
                bold: true,
                ..Default::default()
            },
        );
        let output = book.serialize();
        assert_eq!(output["sheets"][0]["id"], "stable-sheet");
        assert_eq!(output["sheets"][0]["column_widths"]["0"], 180);
        assert_eq!(output["sheets"][0]["frozen_rows"], 2);
        assert_eq!(output["sheets"][0]["charts"][0]["title"], "Sales");
        assert_eq!(output["named_ranges"][0]["name"], "Revenue");
        let (_, cells, styles, _) = hydrate_workbook(&output);
        assert_eq!(cells[&(0, 0, 0)], "=SUM(A2:A3)");
        assert!(styles[&(0, 3, 2)].bold);
    }

    #[test]
    fn merged_workbook_grid_mounts_through_semantic_controls() {
        let app = App::memory();
        app.mount(widget(move |scope| {
            let mut book = Book::default();
            book.snapshot.cells.insert((0, 0, 0), "=SUM(A2:A3)".into());
            book.snapshot.merged_cells.insert(
                0,
                vec![MergedRange {
                    start: (0, 0),
                    end: (1, 1),
                }],
            );
            let ed = Editor {
                book: scope.cage(book),
                sheet: scope.cage(0),
                active: scope.cage((0, 0)),
                anchor: scope.cage((0, 0)),
                end: scope.cage((0, 0)),
                extra: scope.cage(Vec::new()),
                extend: scope.cage(false),
                additive: scope.cage(false),
                filter: scope.cage(None),
                formula: scope.cage(String::new()),
                status: scope.cage(String::new()),
                layout: scope.cage(0),
            };
            grid(scope, ed).build(scope)
        }))
        .expect("valid merged grid and keyboard boundary");
        assert!(app.stats().nodes > 1300);
        app.clear().expect("grid disposes its Owners");
        assert_eq!(app.stats().nodes, 0);
    }
}

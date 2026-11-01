//! The composer's slash commands and `@` mentions: the autocomplete popup above the box, its
//! keyboard (↑ ↓ Tab Enter Esc), the attached-item chips, and running a command. The parsing and
//! ranking live in [`crate::commands`]; this is the dock side (state, IO, drawing).
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ai::{AiMessage, CancellationToken};
use mv_ui::theme::{icon as icon_px, radius};
use mv_ui::toast::{self, Toast};
use mv_ui::{glyphs, kbd, list_row, ph, ph_icon, popup_surface, size, tip, Ph, Tokens, Weight};

use super::Dock;
use crate::commands::{self as cmds, Action, Candidate, Command, Mention, MentionKind, MentionQuery};
use crate::shared::shared;
use crate::store::{now_ms, ChatItem, ItemStatus};

/// Rows the popup shows at most.
const MAX_ROWS: usize = 8;
/// Videos the `@video` list offers at most (newest first).
const MAX_VIDEOS: usize = 60;

/// One row of the composer popup.
#[derive(Clone)]
pub(super) enum Row {
    Cmd(&'static Command),
    Cand(Candidate),
    Hint(String),
}

/// Mention items loaded in the background: the context they were loaded for, and the items.
#[derive(Default)]
pub(super) struct MentionCache {
    pub items: HashMap<MentionKind, (String, Vec<Mention>)>,
    pub loading: HashMap<MentionKind, String>,
}

fn command_icon(c: &Command) -> Ph {
    match c.name {
        "new" => ph::NOTE_PENCIL,
        "clear" => ph::TRASH_SIMPLE,
        "model" => ph::ROBOT,
        "help" => ph::LIGHTBULB,
        "retry" => ph::ARROW_CLOCKWISE,
        "stop" => ph::STOP_CIRCLE,
        "export" => ph::DOWNLOAD_SIMPLE,
        "compact" => ph::CORNERS_IN,
        "tools" => ph::WRENCH,
        "make" => glyphs::VIDEO,
        "download" => ph::CLOUD_ARROW_DOWN,
        "status" => ph::SPEEDOMETER,
        "recap" => ph::FILE_VIDEO,
        "motion" => ph::MAGIC_WAND,
        "play" => ph::PLAY,
        "guide" => ph::COMPASS,
        _ => ph::CODE,
    }
}

fn kind_icon(k: MentionKind) -> Ph {
    match k {
        MentionKind::Manga => glyphs::MANGA,
        MentionKind::Chapter => ph::BOOK_OPEN,
        MentionKind::Page => glyphs::IMAGE,
        MentionKind::Video => ph::FILE_VIDEO,
        MentionKind::File => ph::PAPERCLIP,
    }
}

impl Dock {
    fn composer_state(&self, cx: &App) -> (String, usize) {
        let s = self.input.read(cx);
        (s.value().to_string(), s.cursor())
    }

    /// The `@` token being typed, if the popup is about mentions.
    fn mention_at(&self, cx: &App) -> Option<MentionQuery> {
        let (text, cursor) = self.composer_state(cx);
        if cmds::slash_query(&text).is_some() {
            return None;
        }
        cmds::mention_query(&text, cursor)
    }

    /// The context chapter / page lists depend on: the attached manga and chapter.
    fn mention_ctx_key(&self, kind: MentionKind) -> String {
        let of = |k: MentionKind| self.mentions.iter().rev().find(|m| m.kind == k).map(|m| m.id.clone()).unwrap_or_default();
        match kind {
            MentionKind::Chapter => of(MentionKind::Manga),
            MentionKind::Page => of(MentionKind::Chapter),
            _ => String::new(),
        }
    }

    /// The popup's rows, or `None` when it is closed.
    pub(super) fn popup_rows(&self, cx: &App) -> Option<(&'static str, Vec<Row>)> {
        let (text, cursor) = self.composer_state(cx);
        if self.popup_closed.as_deref() == Some(text.as_str()) {
            return None;
        }
        if let Some(q) = cmds::slash_query(&text) {
            let rows: Vec<Row> = cmds::filter_commands(q).into_iter().map(Row::Cmd).collect();
            return (!rows.is_empty()).then_some(("Commands", rows));
        }
        let q = cmds::mention_query(&text, cursor)?;
        let kind = q.kind.unwrap_or(MentionKind::Manga);
        let items: &[Mention] = self.mention_cache.items.get(&kind).filter(|(k, _)| *k == self.mention_ctx_key(kind)).map(|(_, v)| v.as_slice()).unwrap_or(&[]);
        let mut rows: Vec<Row> = cmds::mention_candidates(&q, items, MAX_ROWS).into_iter().map(Row::Cand).collect();
        if rows.is_empty() || (q.kind.is_some() && rows.iter().all(|r| !matches!(r, Row::Cand(Candidate::Item(_))))) {
            let loading = self.mention_cache.loading.contains_key(&kind);
            rows.push(Row::Hint(match (q.kind, loading) {
                (_, true) => "Loading…".into(),
                (Some(MentionKind::Chapter), _) if self.mention_ctx_key(MentionKind::Chapter).is_empty() && items.is_empty() => "No downloaded chapters. Attach a @manga to see all of its chapters.".into(),
                (Some(MentionKind::Page), _) if self.mention_ctx_key(MentionKind::Page).is_empty() => "Attach a downloaded @chapter first, then pick its pages.".into(),
                (Some(MentionKind::File), _) => "Press Enter to pick a file".into(),
                _ => format!("Nothing matches “{}”", q.query),
            }));
        }
        Some(("Attach", rows))
    }

    pub(super) fn popup_open(&self, cx: &App) -> bool {
        self.popup_rows(cx).is_some()
    }

    /// Called on every composer change: re-opens a popup closed for other text, keeps the
    /// highlight in range and loads what the mention popup needs.
    pub(super) fn composer_changed(&mut self, cx: &mut Context<Self>) {
        let (text, _) = self.composer_state(cx);
        if self.popup_closed.as_deref().is_some_and(|t| t != text) {
            self.popup_closed = None;
        }
        let n = self.popup_rows(cx).map_or(0, |(_, r)| r.len());
        if self.popup_ix >= n {
            self.popup_ix = 0;
        }
        if let Some(q) = self.mention_at(cx) {
            let kind = q.kind.unwrap_or(MentionKind::Manga);
            if kind != MentionKind::File {
                self.load_mentions(kind, cx);
            }
        }
    }

    fn load_mentions(&mut self, kind: MentionKind, cx: &mut Context<Self>) {
        let key = self.mention_ctx_key(kind);
        let fresh = self.mention_cache.items.get(&kind).is_some_and(|(k, _)| *k == key);
        if fresh || self.mention_cache.loading.get(&kind) == Some(&key) {
            return;
        }
        self.mention_cache.loading.insert(kind, key.clone());
        let chapter_dir = self.mentions.iter().rev().find(|m| m.kind == MentionKind::Chapter).and_then(|m| m.path.clone());
        let chapter_label = self.mentions.iter().rev().find(|m| m.kind == MentionKind::Chapter).map(|m| m.label.clone()).unwrap_or_default();
        let history = self.shared.read(cx).store.active_thread().map(|t| t.history.clone()).unwrap_or_default();
        let ctx_key = key.clone();
        cx.spawn(async move |this, cx| {
            let items = mv_ui::bg::blocking(move || match kind {
                MentionKind::Manga => manga_items(&history),
                MentionKind::Chapter => chapter_items(&ctx_key),
                MentionKind::Page => page_items(&ctx_key, &chapter_label, chapter_dir.as_deref()),
                MentionKind::Video => video_items(),
                MentionKind::File => vec![],
            })
            .await
            .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                if this.mention_cache.loading.get(&kind) == Some(&key) {
                    this.mention_cache.loading.remove(&kind);
                }
                this.mention_cache.items.insert(kind, (key, items));
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn popup_move(&mut self, delta: i32, cx: &mut Context<Self>) {
        let n = self.popup_rows(cx).map_or(0, |(_, r)| r.len()) as i32;
        if n > 0 {
            self.popup_ix = (self.popup_ix as i32 + delta).rem_euclid(n) as usize;
            cx.notify();
        }
    }

    pub(super) fn close_popup(&mut self, cx: &mut Context<Self>) {
        self.popup_closed = Some(self.composer_state(cx).0);
        cx.notify();
    }

    /// Enter / Tab on the popup: takes the highlighted row. False when nothing was taken.
    pub(super) fn popup_accept(&mut self, run: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((_, rows)) = self.popup_rows(cx) else { return false };
        let Some(row) = rows.get(self.popup_ix.min(rows.len().saturating_sub(1))).cloned() else { return false };
        self.take_row(row, run, window, cx)
    }

    fn set_composer(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.insert(text.to_string(), window, cx);
            s.focus(window, cx);
        });
        self.popup_ix = 0;
    }

    /// Takes a popup row: a command runs (or waits for its arguments), a kind narrows the list,
    /// an item becomes a chip.
    pub(super) fn take_row(&mut self, row: Row, run: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match row {
            Row::Hint(_) => {
                if let Some(q) = self.mention_at(cx).filter(|q| q.kind == Some(MentionKind::File)) {
                    self.drop_token(&q, window, cx);
                    self.pick_file(cx);
                    return true;
                }
                false
            }
            Row::Cmd(c) => {
                if run && (c.args.is_empty() || c.args.starts_with('[')) {
                    self.set_composer("", window, cx);
                    self.run_command(c, String::new(), window, cx);
                } else {
                    self.set_composer(&format!("/{} ", c.name), window, cx);
                }
                cx.notify();
                true
            }
            Row::Cand(Candidate::Kind(MentionKind::File)) => {
                if let Some(q) = self.mention_at(cx) {
                    self.drop_token(&q, window, cx);
                }
                self.pick_file(cx);
                true
            }
            Row::Cand(Candidate::Kind(k)) => {
                let (text, cursor) = self.composer_state(cx);
                if let Some(q) = cmds::mention_query(&text, cursor) {
                    let head = format!("{}@{}:", &text[..q.at], k.key());
                    let tail = text[cursor.min(text.len())..].to_string();
                    self.set_composer(&format!("{head}{tail}"), window, cx);
                    self.load_mentions(k, cx);
                }
                cx.notify();
                true
            }
            Row::Cand(Candidate::Item(m)) => {
                if let Some(q) = self.mention_at(cx) {
                    self.drop_token(&q, window, cx);
                }
                self.attach_mention(m, cx);
                true
            }
        }
    }

    fn drop_token(&mut self, q: &MentionQuery, window: &mut Window, cx: &mut Context<Self>) {
        let (text, cursor) = self.composer_state(cx);
        let rest = cmds::remove_token(&text, q, cursor);
        self.set_composer(&rest, window, cx);
    }

    pub(super) fn attach_mention(&mut self, m: Mention, cx: &mut Context<Self>) {
        if !self.mentions.iter().any(|x| x.kind == m.kind && x.label == m.label && x.id == m.id && x.path == m.path) {
            self.mentions.push(m);
        }
        cx.notify();
    }

    fn pick_file(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Attach files for the assistant".into()) });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update(cx, |this, cx| {
                    for p in paths {
                        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string();
                        this.attach_mention(Mention::new(MentionKind::File, name).path(p), cx);
                    }
                });
            }
        })
        .detach();
    }

    // ── Running commands ──

    /// Runs a slash command typed in the composer. Returns false when the text is not one.
    pub(super) fn try_command(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((c, args)) = cmds::parse(text) else { return false };
        self.set_composer("", window, cx);
        self.run_command(c, args, window, cx);
        true
    }

    pub(super) fn run_command(&mut self, c: &'static Command, args: String, window: &mut Window, cx: &mut Context<Self>) {
        let busy = self.run.is_some();
        match c.action {
            Action::Stop => self.stop(cx),
            _ if busy && !matches!(c.action, Action::Help | Action::Tools | Action::Export | Action::Model) => {
                let why = self.wait_reason(cx);
                toast::show(cx, Toast::info(why));
            }
            Action::New => self.new_chat(window, cx),
            Action::Clear => self.clear_chat(cx),
            Action::Retry => self.retry(window, cx),
            Action::Help => self.local_reply(format!("/{}", c.name), cmds::help_markdown(), cx),
            Action::Tools => self.local_reply(format!("/tools {args}").trim().to_string(), tools_markdown(&args), cx),
            Action::Model => self.switch_model(&args, window, cx),
            Action::Export => self.export_chat(cx),
            Action::Compact => self.compact_chat(cx),
            Action::Prompt(t) => self.send(cmds::expand(t, &args), window, cx),
        }
        cx.notify();
    }

    /// A reply the app writes itself (`/help`, `/tools`): shown in the chat, not sent to the model.
    fn local_reply(&mut self, asked: String, text: String, cx: &mut Context<Self>) {
        self.shared.update(cx, |s, cx| {
            let id = match s.store.active.clone().filter(|id| s.store.thread(id).is_some()) {
                Some(id) => id,
                None => s.store.new_thread(),
            };
            if let Some(t) = s.store.thread_mut(&id) {
                if t.items.is_empty() {
                    t.title = asked.trim_start_matches('/').to_string();
                }
                t.items.push(ChatItem::user(asked));
                let mut a = ChatItem::assistant("");
                a.model = None;
                a.text = text;
                a.status = ItemStatus::Done;
                t.items.push(a);
                t.updated_at = now_ms();
            }
            s.save();
            cx.notify();
        });
        self.follow = true;
        self.scroll.scroll_to_bottom();
    }

    /// `/clear`: empties this chat (messages and what the model remembers), with Undo.
    fn clear_chat(&mut self, cx: &mut Context<Self>) {
        let removed = self.shared.update(cx, |s, cx| {
            let id = s.store.active.clone()?;
            let t = s.store.thread_mut(&id)?;
            if t.items.is_empty() {
                return None;
            }
            let items = std::mem::take(&mut t.items);
            let history = std::mem::take(&mut t.history);
            s.save();
            cx.notify();
            Some((id, items, history))
        });
        let Some((tid, items, history)) = removed else { return };
        toast::show(
            cx,
            Toast::success("Cleared this chat").icon(ph::TRASH_SIMPLE).action("Undo", move |_, cx| {
                shared(cx).update(cx, |s, cx| {
                    if let Some(t) = s.store.thread_mut(&tid).filter(|t| t.items.is_empty()) {
                        t.items = items.clone();
                        t.history = history.clone();
                        s.save();
                        cx.notify();
                    }
                });
            }),
        );
    }

    /// `/model`: opens the picker, or switches straight to the one model whose name matches.
    fn switch_model(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        let q = query.trim().to_lowercase();
        if !q.is_empty() {
            let hit = {
                let s = self.shared.read(cx);
                let mut hits: Vec<(mv_ai::AiProviderId, String, String)> = vec![];
                for (p, models) in &s.models {
                    for m in models {
                        let name = m.label.clone();
                        if m.id.to_lowercase().contains(&q) || name.to_lowercase().contains(&q) {
                            hits.push((*p, m.id.clone(), name));
                        }
                    }
                }
                // An exact id wins; otherwise only an unambiguous match switches.
                hits.iter().find(|h| h.1.to_lowercase() == q).cloned().or_else(|| (hits.len() == 1).then(|| hits[0].clone()))
            };
            if let Some((p, id, name)) = hit {
                self.shared.update(cx, |s, cx| s.use_in_assistant(p, &id, cx));
                toast::show(cx, Toast::success(format!("Now using {}", if name.is_empty() { id } else { name })).icon(ph::ROBOT));
                return;
            }
        }
        let provider = self.shared.read(cx).store.prefs.provider;
        self.open_picker(provider, window, cx);
    }

    /// `/export`: saves the chat as Markdown where the user picks.
    fn export_chat(&mut self, cx: &mut Context<Self>) {
        let Some((title, md)) = self.shared.read(cx).store.active_thread().filter(|t| !t.items.is_empty()).map(|t| (t.title.clone(), cmds::to_markdown(&t.title, &t.items))) else {
            toast::show(cx, Toast::info("Nothing to export yet"));
            return;
        };
        let dir = documents_dir();
        let rx = cx.prompt_for_new_path(&dir, Some(&cmds::export_name(&title)));
        cx.spawn(async move |_, cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            let path = if path.extension().is_none() { path.with_extension("md") } else { path };
            let res = std::fs::write(&path, md);
            let _ = cx.update(|cx| match res {
                Ok(()) => {
                    let shown = path.clone();
                    toast::show(cx, Toast::success(format!("Saved {}", path.file_name().and_then(|n| n.to_str()).unwrap_or("the chat"))).icon(ph::DOWNLOAD_SIMPLE).action("Show in folder", move |_, cx| cx.reveal_path(&shown)));
                }
                Err(e) => toast::show(cx, Toast::warning(format!("Could not save the chat: {e}"))),
            });
        })
        .detach();
    }

    /// `/compact`: the older part of the conversation becomes a short summary the model reads
    /// instead (the messages stay on screen).
    fn compact_chat(&mut self, cx: &mut Context<Self>) {
        if self.compacting {
            return;
        }
        let (tid, history, provider, model) = {
            let s = self.shared.read(cx);
            let Some(t) = s.store.active_thread() else { return };
            (t.id.clone(), t.history.clone(), s.store.prefs.provider, s.store.prefs.model.clone())
        };
        let Some(cut) = cmds::compact_cut(&history, 2) else {
            toast::show(cx, Toast::info("This chat is still short: nothing to compact yet"));
            return;
        };
        self.compacting = true;
        toast::show(cx, Toast::info("Summarising the earlier messages…").icon(ph::CORNERS_IN));
        let registry = self.shared.read(cx).registry.clone();
        let old: Vec<AiMessage> = history[..cut].to_vec();
        let prompt = cmds::compact_prompt(&old);
        let fallback = cmds::local_summary(&old);
        let job = async move {
            if model.is_empty() {
                return Err("no model".to_string());
            }
            crate::agent::ask_once(&registry, provider, &model, prompt, CancellationToken::new()).await
        };
        cx.spawn(async move |this, cx| {
            let summary = mv_ui::bg::run(job).await.ok().and_then(Result::ok);
            let used_model = summary.is_some();
            let summary = summary.unwrap_or(fallback);
            let _ = this.update(cx, |this, cx| {
                this.compacting = false;
                this.shared.update(cx, |s, cx| {
                    if let Some(t) = s.store.thread_mut(&tid) {
                        // Only if nothing was sent meanwhile (the cut still points at the same turn).
                        if t.history.len() >= history.len() && t.history[..cut] == history[..cut] {
                            t.history = cmds::compacted(&t.history, cut, &summary);
                        }
                    }
                    s.save();
                    cx.notify();
                });
                let n = old.len();
                toast::show(cx, Toast::success(format!("Compacted {n} earlier messages{}", if used_model { "" } else { " (a simple summary: the model did not answer)" })).icon(ph::CORNERS_IN));
            });
        })
        .detach();
    }

    // ── Drawing ──

    /// The popup above the composer box, when open.
    pub(super) fn popup_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (title, rows) = self.popup_rows(cx)?;
        let sel = self.popup_ix.min(rows.len().saturating_sub(1));
        let mut list = div().id("composer-pop-list").flex().flex_col().gap(px(1.)).max_h(px(300.)).overflow_y_scroll();
        for (i, row) in rows.iter().cloned().enumerate() {
            list = list.child(self.popup_row(i, i == sel, row, cx));
        }
        let head = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .pt(px(4.))
            .pb(px(4.))
            .text_size(px(size::XS))
            .text_color(Tokens::text_3())
            .child(div().flex_1().child(title))
            .child(kbd("up"))
            .child(kbd("down"))
            .child("to choose")
            .child(div().w(px(4.)))
            .child(kbd("enter"))
            .child(div().w(px(4.)))
            .child(kbd("escape"))
            .child("to close");
        Some(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom(relative(1.))
                .pb(px(6.))
                .child(mv_ui::appear("composer-pop", popup_surface().id("composer-pop").w_full().child(head).child(list)))
                .into_any_element(),
        )
    }

    fn popup_row(&self, i: usize, selected: bool, row: Row, cx: &mut Context<Self>) -> AnyElement {
        let (icon, label, hint, detail): (Ph, String, String, String) = match &row {
            Row::Cmd(c) => (command_icon(c), format!("/{}", c.name), c.args.to_string(), c.about.to_string()),
            Row::Cand(Candidate::Kind(k)) => (kind_icon(*k), format!("@{}", k.key()), String::new(), k.about().to_string()),
            Row::Cand(Candidate::Item(m)) => (kind_icon(m.kind), m.label.clone(), String::new(), m.detail.clone()),
            Row::Hint(h) => {
                return div().px(px(10.)).py(px(8.)).text_size(px(size::SM)).text_color(Tokens::text_3()).child(h.clone()).into_any_element();
            }
        };
        list_row(("composer-pop-row", i), selected)
            .min_h(px(32.))
            .py(px(4.))
            .gap(px(8.))
            .text_size(px(size::SM))
            .child(ph_icon(icon, Weight::Bold, if selected { Tokens::text_1() } else { Tokens::text_3() }, icon_px::XS))
            .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(label))
            .when(!hint.is_empty(), |d| d.child(div().flex_none().text_color(Tokens::text_3()).child(hint)))
            .child(div().flex_1().min_w_0().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(detail))
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                if this.popup_ix != i {
                    this.popup_ix = i;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, w, cx| {
                this.take_row(row.clone(), true, w, cx);
            }))
            .into_any_element()
    }

    /// The attached items above the text, each with a remove button.
    pub(super) fn mention_chips(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.mentions.is_empty() {
            return None;
        }
        let chips = self.mentions.iter().enumerate().map(|(i, m)| {
            let full = m.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| m.label.clone());
            div()
                .id(("mention", i))
                .flex()
                .items_center()
                .gap(px(5.))
                .h(px(24.))
                .pl(px(8.))
                .pr(px(2.))
                .max_w(px(220.))
                .rounded(px(radius::MD))
                .border_1()
                .border_color(Tokens::border())
                .bg(Tokens::surface_2())
                .text_size(px(size::XS))
                .text_color(Tokens::text_2())
                .tooltip(tip(format!("@{} · {full}", m.kind.key())))
                .child(ph_icon(kind_icon(m.kind), Weight::Bold, Tokens::text_3(), icon_px::XS))
                .child(div().min_w_0().truncate().child(m.label.clone()))
                .child(
                    div()
                        .id(("mention-x", i))
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(18.))
                        .rounded_full()
                        .role(Role::Button)
                        .aria_label("Remove")
                        .hover(|s| s.bg(Tokens::hover()))
                        .child(ph_icon(ph::X, Weight::Bold, Tokens::text_3(), icon_px::XS))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if i < this.mentions.len() {
                                this.mentions.remove(i);
                            }
                            cx.notify();
                        })),
                )
        });
        Some(div().flex().flex_wrap().gap(px(6.)).px(px(10.)).pt(px(10.)).children(chips).into_any_element())
    }
}

/// Chips under a sent message: what it carried.
pub(super) fn sent_chips(item: &ChatItem) -> Option<Div> {
    if item.mentions.is_empty() {
        return None;
    }
    Some(div().flex().flex_wrap().justify_end().gap(px(4.)).max_w(relative(0.88)).children(item.mentions.iter().map(|m| {
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .h(px(20.))
            .px(px(6.))
            .rounded(px(radius::SM))
            .bg(Tokens::surface_2())
            .text_size(px(size::XS))
            .text_color(Tokens::text_3())
            .child(ph_icon(kind_icon(m.kind), Weight::Bold, Tokens::text_3(), icon_px::XS))
            .child(div().max_w(px(180.)).truncate().child(m.label.clone()))
    })))
}

/// `/tools`: the tools by group, or the best matches for a search.
fn tools_markdown(query: &str) -> String {
    let specs = crate::tools::tool_specs();
    if !query.trim().is_empty() {
        let hits = crate::tools::rank_tools(query, &specs);
        if hits.is_empty() {
            return format!("No tool matches “{}”.", query.trim());
        }
        let mut s = format!("**Tools for “{}”**\n\n", query.trim());
        for (name, _) in hits.into_iter().take(10) {
            if let Some(t) = specs.iter().find(|t| t.name == name) {
                s.push_str(&format!("- `{}` — {}\n", t.name, t.description.split(". ").next().unwrap_or(&t.description)));
            }
        }
        return s;
    }
    let mut s = format!("**{} tools the assistant can use**, by area. Small models get a few at a time and find the rest with `find_tools`.\n", specs.len());
    for g in crate::tools::Group::ALL {
        let names: Vec<String> = specs.iter().filter(|t| crate::tools::group_of(&t.name) == g).map(|t| format!("`{}`", t.name)).collect();
        if !names.is_empty() {
            s.push_str(&format!("\n**{}** — {}\n", capital(g.key()), names.join(", ")));
        }
    }
    s.push_str("\nAsk `/tools <words>` to search, e.g. `/tools export reels`.");
    s
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn documents_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let docs = home.join("Documents");
    if docs.is_dir() {
        docs
    } else {
        home
    }
}

// ── Mention sources (run off the UI thread) ──

fn manga_items(history: &[AiMessage]) -> Vec<Mention> {
    let mut out = cmds::search_mentions(history);
    let rows = mv_core::library::open_readonly().and_then(|c| mv_core::library::list_manga(&c)).unwrap_or_default();
    for m in rows {
        if out.iter().any(|o| o.label == m.title) {
            continue;
        }
        let detail = format!("library manga id {}{}", m.id, if m.is_favorite { ", favourite" } else { "" });
        out.push(Mention::new(MentionKind::Manga, m.title).id(m.id).detail(detail));
    }
    out
}

fn is_downloaded(c: &mv_core::repo::chapter::Chapter) -> bool {
    c.pages_dir.as_deref().is_some_and(|d| Path::new(d).is_dir())
}

fn chapter_label(title: &str, c: &mv_core::repo::chapter::Chapter) -> String {
    let n = c.number.map(|n| if n.fract() == 0. { format!("{n:.0}") } else { n.to_string() }).unwrap_or_else(|| "?".into());
    format!("{title} · Ch. {n}{}", c.title.as_deref().filter(|t| !t.trim().is_empty()).map(|t| format!(": {t}")).unwrap_or_default())
}

fn chapter_mention(title: &str, c: &mv_core::repo::chapter::Chapter) -> Mention {
    let dl = is_downloaded(c);
    let mut m = Mention::new(MentionKind::Chapter, chapter_label(title, c)).id(c.id.clone()).detail(format!("manga id {}, {}", c.manga_id, if dl { "downloaded" } else { "not downloaded" }));
    if dl {
        m.path = c.pages_dir.as_ref().map(PathBuf::from);
    }
    m
}

/// The attached manga's chapters, or (none attached) downloaded chapters across the library.
fn chapter_items(manga_id: &str) -> Vec<Mention> {
    let Ok(conn) = mv_core::library::open_readonly() else { return vec![] };
    let manga = mv_core::library::list_manga(&conn).unwrap_or_default();
    if !manga_id.is_empty() {
        let title = manga.iter().find(|m| m.id == manga_id).map(|m| m.title.clone()).unwrap_or_default();
        let mut list = mv_core::repo::chapter::list(&conn, manga_id).unwrap_or_default();
        list.sort_by(|a, b| b.number.partial_cmp(&a.number).unwrap_or(std::cmp::Ordering::Equal));
        return list.iter().map(|c| chapter_mention(&title, c)).collect();
    }
    let mut out = vec![];
    for m in manga.iter().take(30) {
        let mut list: Vec<_> = mv_core::repo::chapter::list(&conn, &m.id).unwrap_or_default().into_iter().filter(is_downloaded).collect();
        list.sort_by(|a, b| b.number.partial_cmp(&a.number).unwrap_or(std::cmp::Ordering::Equal));
        out.extend(list.iter().take(20).map(|c| chapter_mention(&m.title, c)));
        if out.len() >= 200 {
            break;
        }
    }
    out
}

const PAGE_EXTS: [&str; 5] = ["jpg", "jpeg", "png", "webp", "gif"];

fn page_items(chapter_id: &str, chapter_label: &str, dir: Option<&Path>) -> Vec<Mention> {
    let Some(dir) = dir.filter(|d| d.is_dir()) else { return vec![] };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| PAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))).collect()).unwrap_or_default();
    files.sort();
    files
        .into_iter()
        .enumerate()
        .map(|(i, p)| Mention::new(MentionKind::Page, format!("{chapter_label} · page {}", i + 1)).detail(format!("page {} of chapter id {chapter_id}", i + 1)).path(p))
        .collect()
}

/// Studio's chapter videos, exports and motion renders, newest first.
fn video_items() -> Vec<Mention> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default();
    let roots = [mv_core::paths::studio(), home.join("Videos").join("MangaVox")];
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = vec![];
    for r in roots {
        walk_videos(&r, 0, &mut found);
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found
        .into_iter()
        .take(MAX_VIDEOS)
        .map(|(_, p)| {
            let parent = p.parent().and_then(|d| d.file_name()).and_then(|n| n.to_str()).unwrap_or("").to_string();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("video").to_string();
            let label = if name.starts_with("video_final") { format!("{parent} recap") } else { name };
            Mention::new(MentionKind::Video, label).detail(p.display().to_string()).path(p)
        })
        .collect()
}

fn walk_videos(dir: &Path, depth: usize, out: &mut Vec<(std::time::SystemTime, PathBuf)>) {
    if depth > 4 || out.len() > 2000 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            // Work folders hold intermediate clips, not videos to attach.
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !matches!(n, "work" | "clips" | "tmp" | "cache" | "layers" | "thumbnails") {
                walk_videos(&p, depth + 1, out);
            }
        } else if p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("mp4")) {
            let t = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            out.push((t, p));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn tools_listing_and_search() {
        let all = tools_markdown("");
        assert!(all.contains("**Core**") && all.contains("`navigate`"));
        assert!(tools_markdown("anime").contains("search_anime"));
        assert!(tools_markdown("zzzqqq").starts_with("No tool matches"));
    }

    #[::core::prelude::v1::test]
    fn every_command_has_an_icon_and_pages_list_images() {
        for c in cmds::COMMANDS {
            assert_ne!(command_icon(c), ph::CODE, "{}", c.name);
        }
        let dir = std::env::temp_dir().join(format!("mv-pages-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for n in ["002.jpg", "001.png", "notes.txt"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        let pages = page_items("c1", "Solo · Ch. 1", Some(&dir));
        assert_eq!(pages.len(), 2);
        assert!(pages[0].path.as_ref().unwrap().ends_with("001.png") && pages[0].label.ends_with("page 1"));
        assert!(page_items("c1", "x", None).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}

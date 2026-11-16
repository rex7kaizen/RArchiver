//! Arayüz denetleyicisi: Slint ekranını `rarchiver-core` gezgin durumuna bağlar.
//! Android'e özgü her şey `Platform` arkasında; bu dosya masaüstünde de test edilebilir.

use crate::{MainWindow, MenuEntry, RowData, Theme};
use rarchiver_core::archive::{self, ArchiveFormat, CompressOptions};
use rarchiver_core::browser::{Action, Browser, ClipMode, Clipboard, Item, ItemKind, LongPress, Location, Tap};
use rarchiver_core::fsops::{self, Conflict};
use rarchiver_core::listing::SortKey;
use rarchiver_core::search::{self, SearchHit, SearchQuery};
use rarchiver_core::{badge_label, classify, format_date, format_size, Ctx, Error, FileClass, Settings, ViewMode};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel, Weak};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub trait Platform {
    /// "Tüm dosyalara erişim" verildi mi?
    fn has_access(&self) -> bool;
    fn open_access_settings(&self);
    fn data_dir(&self) -> Option<PathBuf>;
}

#[derive(Clone)]
enum Op {
    Paste { clip: Clipboard, dest: PathBuf, policy: Conflict },
    Extract { archive: PathBuf, dest: PathBuf, entries: Vec<String>, base: String, password: Option<String>, policy: Conflict },
    Test { archive: PathBuf, password: Option<String> },
    OpenArchive { file: PathBuf, password: Option<String> },
}

enum Pending {
    None,
    NewFolder,
    NewFile,
    Rename(PathBuf),
    Delete(Vec<PathBuf>),
    Password(Op),
    Conflict(Op),
    Compress,
    Message,
}

enum MenuAct {
    Act(Action, Option<usize>),
    Go(PathBuf),
    SetGrid(bool),
    SetSort(SortKey),
    ToggleDesc,
    ToggleFoldersFirst,
    ToggleHidden,
    ToggleTime,
    CycleTheme,
    MultiSelect,
    SelectAll,
    Invert,
    NewFolder,
    NewFile,
    Refresh,
    About,
    Noop,
}

enum JobOut {
    Done,
    Text(String, String),
}

struct Job {
    ctx: Ctx,
    slot: Arc<Mutex<Option<Result<JobOut, Error>>>>,
    retry: Option<Op>,
}

struct SearchJob {
    ctx: Ctx,
    hits: Arc<Mutex<Vec<SearchHit>>>,
    done: Arc<Mutex<bool>>,
    query: String,
    shown: usize,
}

struct ArcDlg {
    sources: Vec<PathBuf>,
    fmt: usize,
    level: usize,
    encrypt_names: bool,
    delete: bool,
    separate: bool,
}

const LEVELS: [(&str, u32); 4] = [("Yok", 0), ("Hızlı", 1), ("Normal", 5), ("Maksimum", 9)];
const THEME_NAMES: [&str; 3] = ["Koyu Mor", "Açık Lavanta", "Klasik (ZArchiver)"];

struct State {
    b: Browser,
    settings_path: Option<PathBuf>,
    search_mode: bool,
    chips: [bool; 6],
    menu: Vec<MenuAct>,
    pending: Pending,
    job: Option<Job>,
    search: Option<SearchJob>,
    arc: ArcDlg,
    err_text: Option<String>,
    access: bool,
    tick: u32,
}

pub struct Controller {
    ui: Weak<MainWindow>,
    plat: Rc<dyn Platform>,
    st: RefCell<State>,
    timer: Timer,
}

fn action_label(a: Action, item_name: &str) -> String {
    match a {
        Action::View => "Görüntüle".into(),
        Action::Open => "Aç".into(),
        Action::Install => "Yükle".into(),
        Action::ExtractHere => "Buraya çıkar".into(),
        Action::ExtractToFolder => {
            let stem = item_name.rsplit_once('.').map(|x| x.0).unwrap_or(item_name);
            format!("./{stem}/ klasörüne çıkar")
        }
        Action::ExtractTo => "Çıkar...".into(),
        Action::CompressZip => "zip olarak sıkıştır".into(),
        Action::Compress7z => "7z olarak sıkıştır".into(),
        Action::CompressDialog => "Sıkıştır...".into(),
        Action::Test => "Arşivi sına".into(),
        Action::Copy => "Kopyala".into(),
        Action::Cut => "Kes".into(),
        Action::Delete => "Sil".into(),
        Action::Rename => "Yeniden adlandır".into(),
        Action::Info => "Bilgi".into(),
        Action::AddFavorite => "Sık kullanılanlara ekle".into(),
        Action::RemoveFavorite => "Sık kullanılanlardan kaldır".into(),
        Action::Share => "Paylaş".into(),
        Action::OpenAsArchive => "Arşiv olarak aç".into(),
    }
}

fn glyph_for(name: &str) -> i32 {
    match name.to_lowercase().as_str() {
        "alarms" => 1,
        "android" => 2,
        "dcim" => 3,
        "documents" | "belgeler" => 4,
        "download" | "downloads" | "indirilenler" => 5,
        "movies" | "videolar" => 6,
        "music" | "müzik" => 7,
        "pictures" | "resimler" => 8,
        _ => 0,
    }
}

fn fclass_num(c: FileClass) -> i32 {
    match c {
        FileClass::Archive => 1,
        FileClass::Apk => 2,
        FileClass::Image => 3,
        FileClass::Video => 4,
        FileClass::Audio => 5,
        FileClass::Document => 6,
        FileClass::Text => 7,
        _ => 0,
    }
}

fn item_info(it: &Item) -> String {
    if let Some(sub) = &it.subtitle {
        return sub.clone();
    }
    match it.kind {
        ItemKind::Up => String::new(),
        ItemKind::Folder => match it.children {
            Some(0) => "Boş".into(),
            Some(n) => format!("{n} öğe"),
            None => "—".into(),
        },
        ItemKind::File => format_size(it.size),
    }
}

fn row_of(it: &Item, dir_glyphs: bool) -> RowData {
    RowData {
        name: it.name.clone().into(),
        info: item_info(it).into(),
        date: it.modified.map(format_date).unwrap_or_default().into(),
        hidden: it.hidden,
        selected: it.selected,
        kind: match it.kind {
            ItemKind::File => 0,
            ItemKind::Folder => 1,
            ItemKind::Up => 2,
        },
        glyph: if dir_glyphs && it.kind == ItemKind::Folder { glyph_for(&it.name) } else { 0 },
        fclass: fclass_num(it.class),
        label: if it.kind == ItemKind::File { badge_label(&it.name).into() } else { SharedString::new() },
    }
}

fn menu_model(items: &[(String, i32)]) -> ModelRc<MenuEntry> {
    ModelRc::new(VecModel::from(
        items.iter().map(|(l, c)| MenuEntry { label: l.as_str().into(), check: *c }).collect::<Vec<_>>(),
    ))
}

impl Controller {
    pub fn install(ui: &MainWindow, plat: Rc<dyn Platform>, start: PathBuf) -> Rc<Controller> {
        let settings_path = plat.data_dir().map(|d| d.join("settings.txt"));
        let settings = settings_path.as_ref().map(|p| Settings::load(p)).unwrap_or_default();
        ui.global::<Theme>().set_index(settings.theme as i32);
        let st = State {
            b: Browser::new(start, settings),
            settings_path,
            search_mode: false,
            chips: [false; 6],
            menu: Vec::new(),
            pending: Pending::None,
            job: None,
            search: None,
            arc: ArcDlg { sources: vec![], fmt: 1, level: 2, encrypt_names: false, delete: false, separate: false },
            err_text: None,
            access: true,
            tick: 0,
        };
        let ctl = Rc::new(Controller { ui: ui.as_weak(), plat, st: RefCell::new(st), timer: Timer::default() });

        macro_rules! hook0 {
            ($on:ident, $m:ident) => {{
                let c = ctl.clone();
                ui.$on(move || c.$m());
            }};
        }
        macro_rules! hook1 {
            ($on:ident, $m:ident) => {{
                let c = ctl.clone();
                ui.$on(move |a| c.$m(a));
            }};
        }
        hook0!(on_back, back);
        hook0!(on_location_tapped, location_menu);
        hook0!(on_search_open, search_open);
        hook0!(on_search_close, search_close);
        hook0!(on_search_submit, search_submit);
        hook1!(on_chip_toggled, chip_toggled);
        hook0!(on_view_menu, view_menu);
        hook0!(on_more_menu, more_menu);
        hook0!(on_select_all, select_all_toggle);
        hook0!(on_select_exit, select_exit);
        hook0!(on_paste_cancel, paste_cancel);
        hook0!(on_fab_tapped, fab);
        hook1!(on_row_tapped, row_tapped);
        hook1!(on_row_icon_tapped, row_icon_tapped);
        hook1!(on_row_long, row_long);
        hook0!(on_retry, retry);
        hook0!(on_request_permission, request_permission);
        hook1!(on_menu_picked, menu_picked);
        hook0!(on_overlay_ok, overlay_ok);
        hook0!(on_overlay_cancel, overlay_cancel);
        hook1!(on_overlay_extra, overlay_extra);
        hook0!(on_arc_pick_format, arc_pick_format);
        hook0!(on_arc_pick_level, arc_pick_level);
        hook0!(on_arc_toggle_names, arc_toggle_names);
        hook0!(on_arc_toggle_delete, arc_toggle_delete);
        hook0!(on_arc_toggle_separate, arc_toggle_separate);

        {
            let c = ctl.clone();
            ctl.timer.start(TimerMode::Repeated, Duration::from_millis(120), move || c.poll());
        }
        ctl.refresh();
        ctl
    }

    // ---------- yardımcılar ----------

    fn ui(&self) -> Option<MainWindow> {
        self.ui.upgrade()
    }

    fn save_settings(&self) {
        let st = self.st.borrow();
        if let Some(p) = &st.settings_path {
            let _ = st.b.settings.save(p);
        }
    }

    /// Dizini yeniden okur ve ekranı günceller.
    pub fn refresh(&self) {
        {
            let mut st = self.st.borrow_mut();
            st.access = self.plat.has_access();
            st.err_text = None;
            if st.access {
                if let Err(e) = st.b.refresh() {
                    st.err_text = Some(e.to_string());
                }
            }
        }
        self.sync();
    }

    /// Tüm Slint özelliklerini durumdan yeniden hesaplar.
    pub fn sync(&self) {
        let Some(ui) = self.ui() else { return };
        let st = self.st.borrow();
        let b = &st.b;
        let dir_glyphs = matches!(b.location, Location::Dir(_));
        let rows: Vec<RowData> = b.items.iter().map(|i| row_of(i, dir_glyphs)).collect();
        ui.set_rows(ModelRc::new(VecModel::from(rows)));
        ui.set_folder_name(b.title().into());
        ui.set_sub_path(b.subtitle().into());
        ui.set_grid(b.settings.view == ViewMode::Grid);
        ui.set_show_time(b.settings.show_time);
        ui.set_select_count(b.selection_count() as i32);
        ui.set_select_mode(b.multi_select);
        ui.set_paste_mode(b.is_paste_mode());
        ui.set_paste_title(
            match b.clipboard.as_ref().map(|c| &c.mode) {
                Some(ClipMode::Extract { .. }) => "Çıkar",
                Some(ClipMode::Cut) => "Taşı",
                _ => "Yapıştır",
            }
            .into(),
        );
        ui.set_search_mode(st.search_mode);
        ui.set_searching(st.search.as_ref().map(|s| !*s.done.lock().unwrap()).unwrap_or(false));
        ui.set_chips(ModelRc::new(VecModel::from(st.chips.to_vec())));
        let (state, text) = if !st.access {
            (3, "RArchiver'ın dosyalarını gösterebilmesi için \"Tüm dosyalara erişim\" iznini açın. İzni verip geri döndüğünüzde liste otomatik yenilenir.".to_string())
        } else if let Some(e) = &st.err_text {
            (2, e.clone())
        } else if b.items.iter().all(|i| i.kind == ItemKind::Up) && !matches!(b.location, Location::Search { .. }) {
            (1, String::new())
        } else {
            (0, String::new())
        };
        ui.set_state(state);
        ui.set_state_text(text.into());
        ui.global::<Theme>().set_index(b.settings.theme as i32);
    }

    // ---------- yer paylaşımı yardımcıları ----------

    fn set_overlay(&self, kind: i32) {
        if let Some(ui) = self.ui() {
            ui.set_overlay(kind);
        }
    }

    fn open_menu(&self, title: &str, entries: Vec<(String, i32, MenuAct)>) {
        let (labels, acts): (Vec<_>, Vec<_>) = entries.into_iter().map(|(l, c, a)| ((l, c), a)).unzip();
        self.st.borrow_mut().menu = acts;
        if let Some(ui) = self.ui() {
            ui.set_overlay_title(title.into());
            ui.set_menu_items(menu_model(&labels));
            ui.set_overlay(1);
        }
    }

    fn message(&self, title: &str, text: &str) {
        self.st.borrow_mut().pending = Pending::Message;
        if let Some(ui) = self.ui() {
            ui.set_overlay_title(title.into());
            ui.set_overlay_message(text.into());
            ui.set_ok_label("TAMAM".into());
            ui.set_show_cancel(false);
            ui.set_overlay(3);
        }
    }

    fn confirm(&self, title: &str, text: &str, ok: &str, pending: Pending) {
        self.st.borrow_mut().pending = pending;
        if let Some(ui) = self.ui() {
            ui.set_overlay_title(title.into());
            ui.set_overlay_message(text.into());
            ui.set_ok_label(ok.into());
            ui.set_show_cancel(true);
            ui.set_overlay(3);
        }
    }

    fn ask_text(&self, title: &str, hint: &str, initial: &str, ok: &str, pending: Pending) {
        self.st.borrow_mut().pending = pending;
        if let Some(ui) = self.ui() {
            ui.set_overlay_title(title.into());
            ui.set_input_hint(hint.into());
            ui.set_input_text(initial.into());
            ui.set_ok_label(ok.into());
            ui.set_overlay(2);
        }
    }

    fn close_overlay(&self) {
        self.st.borrow_mut().pending = Pending::None;
        self.set_overlay(0);
    }

    // ---------- üst çubuk ----------

    fn back(&self) {
        let r = self.st.borrow_mut().b.go_up();
        match r {
            Ok(_) => {}
            Err(e) => self.st.borrow_mut().err_text = Some(e.to_string()),
        }
        self.sync();
    }

    fn location_menu(&self) {
        let (favs, start) = {
            let st = self.st.borrow();
            (st.b.settings.favorites.clone(), st.b.start.clone())
        };
        let mut e: Vec<(String, i32, MenuAct)> = vec![
            ("Dahili depolama".into(), -1, MenuAct::Go(start.clone())),
            ("İndirilenler".into(), -1, MenuAct::Go(start.join("Download"))),
            ("Kök (/)".into(), -1, MenuAct::Go(PathBuf::from("/"))),
        ];
        for f in favs {
            let name = f.to_string_lossy().into_owned();
            e.push((format!("★ {name}"), -1, MenuAct::Go(f)));
        }
        e.push(("Yenile".into(), -1, MenuAct::Refresh));
        self.open_menu("Konum", e);
    }

    fn search_open(&self) {
        self.st.borrow_mut().search_mode = true;
        if let Some(ui) = self.ui() {
            ui.set_search_text("".into());
        }
        self.sync();
    }

    fn search_close(&self) {
        {
            let mut st = self.st.borrow_mut();
            if let Some(s) = st.search.take() {
                s.ctx.cancel.cancel();
            }
            st.search_mode = false;
            if matches!(st.b.location, Location::Search { .. }) {
                let _ = st.b.go_up();
            }
        }
        self.sync();
    }

    fn chip_toggled(&self, i: i32) {
        if let Some(c) = self.st.borrow_mut().chips.get_mut(i as usize) {
            *c = !*c;
        }
        self.sync();
    }

    fn search_submit(&self) {
        let Some(ui) = self.ui() else { return };
        let text = ui.get_search_text().to_string();
        let mut st = self.st.borrow_mut();
        let classes: Vec<FileClass> = [FileClass::Archive, FileClass::Apk, FileClass::Image, FileClass::Video, FileClass::Document, FileClass::Audio]
            .into_iter()
            .zip(st.chips)
            .filter(|(_, on)| *on)
            .map(|(c, _)| c)
            .collect();
        if text.trim().is_empty() && classes.is_empty() {
            return;
        }
        let origin = match &st.b.location {
            Location::Dir(p) => p.clone(),
            Location::Search { origin, .. } => origin.clone(),
            Location::Archive { file, .. } => file.parent().map(|p| p.to_path_buf()).unwrap_or(st.b.start.clone()),
        };
        if let Some(old) = st.search.take() {
            old.ctx.cancel.cancel();
        }
        let ctx = Ctx::new();
        let hits = Arc::new(Mutex::new(Vec::new()));
        let done = Arc::new(Mutex::new(false));
        {
            let (ctx, hits, done) = (ctx.clone(), hits.clone(), done.clone());
            let q = SearchQuery { pattern: text.trim().to_string(), classes, max_results: 3000 };
            std::thread::spawn(move || {
                let h2 = hits.clone();
                let _ = search::search(&origin, &q, &ctx, &mut |h| h2.lock().unwrap().push(h));
                *done.lock().unwrap() = true;
            });
        }
        let q = if text.trim().is_empty() { "*".to_string() } else { text.trim().to_string() };
        st.search = Some(SearchJob { ctx, hits, done, query: q.clone(), shown: usize::MAX });
        st.b.show_search_results(&q, Vec::new());
        drop(st);
        self.sync();
    }

    fn view_menu(&self) {
        let (grid, sort, desc, ff) = {
            let s = &self.st.borrow().b.settings;
            (s.view == ViewMode::Grid, s.sort, s.descending, s.folders_first)
        };
        let c = |b: bool| if b { 1 } else { 0 };
        self.open_menu(
            "Görünüm",
            vec![
                ("Liste".into(), c(!grid), MenuAct::SetGrid(false)),
                ("Izgara".into(), c(grid), MenuAct::SetGrid(true)),
                ("Ada göre".into(), c(sort == SortKey::Name), MenuAct::SetSort(SortKey::Name)),
                ("Tarihe göre".into(), c(sort == SortKey::Modified), MenuAct::SetSort(SortKey::Modified)),
                ("Boyuta göre".into(), c(sort == SortKey::Size), MenuAct::SetSort(SortKey::Size)),
                ("Azalan sıra".into(), c(desc), MenuAct::ToggleDesc),
                ("Klasörler önce".into(), c(ff), MenuAct::ToggleFoldersFirst),
            ],
        );
    }

    fn more_menu(&self) {
        let (hidden, time, theme, sel_active, in_archive) = {
            let st = self.st.borrow();
            (st.b.settings.show_hidden, st.b.settings.show_time, st.b.settings.theme, st.b.selection_active(), st.b.in_archive())
        };
        if sel_active {
            let (acts, names): (Vec<Action>, Vec<String>) = {
                let st = self.st.borrow();
                let first = st.b.items.iter().position(|i| i.selected);
                let name = first.and_then(|i| st.b.items.get(i)).map(|i| i.name.clone()).unwrap_or_default();
                (st.b.selection_menu(), vec![name])
            };
            let e = acts.into_iter().map(|a| (action_label(a, &names[0]), -1, MenuAct::Act(a, None))).collect();
            self.open_menu("Seçili öğeler", e);
            return;
        }
        let c = |b: bool| if b { 1 } else { 0 };
        let mut e: Vec<(String, i32, MenuAct)> = vec![
            ("Çoklu seçim".into(), -1, MenuAct::MultiSelect),
            ("Tümünü seç".into(), -1, MenuAct::SelectAll),
            ("Seçimi ters çevir".into(), -1, MenuAct::Invert),
        ];
        if !in_archive {
            e.push(("Yeni klasör".into(), -1, MenuAct::NewFolder));
            e.push(("Yeni dosya".into(), -1, MenuAct::NewFile));
        }
        e.push(("Gizli dosyaları göster".into(), c(hidden), MenuAct::ToggleHidden));
        e.push(("Tarihi göster".into(), c(time), MenuAct::ToggleTime));
        e.push((format!("Tema: {}", THEME_NAMES[theme.min(2) as usize]), -1, MenuAct::CycleTheme));
        e.push(("Yenile".into(), -1, MenuAct::Refresh));
        e.push(("Hakkında".into(), -1, MenuAct::About));
        self.open_menu("Menü", e);
    }

    fn select_all_toggle(&self) {
        self.st.borrow_mut().b.select_all_toggle();
        self.sync();
    }

    fn select_exit(&self) {
        self.st.borrow_mut().b.select_none();
        self.sync();
    }

    fn paste_cancel(&self) {
        self.st.borrow_mut().b.cancel_paste();
        self.sync();
    }

    fn request_permission(&self) {
        self.plat.open_access_settings();
    }

    fn retry(&self) {
        self.refresh();
    }

    // ---------- FAB ----------

    fn fab(&self) {
        let (paste, in_dir) = {
            let st = self.st.borrow();
            (st.b.is_paste_mode(), st.b.current_dir().is_some())
        };
        if paste {
            self.paste();
        } else if in_dir {
            self.open_menu(
                "Oluştur",
                vec![
                    ("Yeni klasör".into(), -1, MenuAct::NewFolder),
                    ("Yeni dosya".into(), -1, MenuAct::NewFile),
                ],
            );
        } else {
            self.message("Oluşturulamaz", "Burada yeni öğe oluşturulamaz. Önce normal bir klasöre geçin.");
        }
    }

    fn paste(&self) {
        let (clip, dest) = {
            let st = self.st.borrow();
            (st.b.clipboard.clone(), st.b.current_dir())
        };
        let (Some(clip), Some(dest)) = (clip, dest) else {
            self.message("Yapıştırılamaz", "Buraya yapıştırılamaz. Normal bir klasöre geçin.");
            return;
        };
        let (conflicts, op): (Vec<String>, Op) = match &clip.mode {
            ClipMode::Copy | ClipMode::Cut => (
                fsops::find_conflicts(&clip.paths, &dest),
                Op::Paste { clip: clip.clone(), dest: dest.clone(), policy: Conflict::Skip },
            ),
            ClipMode::Extract { archive, entries, base, password } => {
                let names = top_level_names(archive, password.as_deref(), entries, base);
                let c = names.into_iter().filter(|n| dest.join(n).exists()).collect();
                (c, Op::Extract { archive: archive.clone(), dest: dest.clone(), entries: entries.clone(), base: base.clone(), password: password.clone(), policy: Conflict::Skip })
            }
        };
        self.run_or_ask_conflict(conflicts, op);
    }

    fn run_or_ask_conflict(&self, conflicts: Vec<String>, op: Op) {
        if conflicts.is_empty() {
            self.start_op(op);
            return;
        }
        let shown: Vec<_> = conflicts.iter().take(3).cloned().collect();
        let more = if conflicts.len() > 3 { format!(" ve {} öğe daha", conflicts.len() - 3) } else { String::new() };
        let msg = format!("Hedefte zaten var: {}{}", shown.join(", "), more);
        self.st.borrow_mut().pending = Pending::Conflict(op);
        if let Some(ui) = self.ui() {
            ui.set_overlay_title("Çakışma".into());
            ui.set_overlay_message(msg.into());
            ui.set_overlay(8);
        }
    }

    // ---------- satırlar ----------

    fn row_tapped(&self, i: i32) {
        let r = self.st.borrow_mut().b.tap(i as usize);
        match r {
            Ok(Tap::ShowMenu(idx)) => {
                self.sync();
                self.file_menu(idx, false);
                return;
            }
            Ok(_) => {}
            Err(Error::NeedPassword) | Err(Error::WrongPassword) => {}
            Err(e) => {
                self.sync();
                self.message("Açılamadı", &e.to_string());
                return;
            }
        }
        self.sync();
    }

    fn row_icon_tapped(&self, i: i32) {
        self.st.borrow_mut().b.tap_icon(i as usize);
        self.sync();
    }

    fn row_long(&self, i: i32) {
        let r = self.st.borrow_mut().b.long_press(i as usize);
        self.sync();
        if let LongPress::ShowMenu(idx) = r {
            self.file_menu(idx, true);
        }
    }

    fn file_menu(&self, idx: usize, extra: bool) {
        let (acts, name) = {
            let st = self.st.borrow();
            let name = st.b.items.get(idx).map(|i| i.name.clone()).unwrap_or_default();
            (if extra { st.b.extra_menu(idx) } else { st.b.main_menu(idx) }, name)
        };
        if acts.is_empty() {
            return;
        }
        let e = acts.into_iter().map(|a| (action_label(a, &name), -1, MenuAct::Act(a, Some(idx)))).collect();
        self.open_menu(&name, e);
    }

    // ---------- menü seçimi ----------

    fn menu_picked(&self, i: i32) {
        let act = {
            let mut st = self.st.borrow_mut();
            let i = i as usize;
            if i >= st.menu.len() {
                return;
            }
            std::mem::replace(&mut st.menu[i], MenuAct::Noop)
        };
        self.set_overlay(0);
        match act {
            MenuAct::Noop => {}
            MenuAct::Act(a, idx) => self.do_action(a, idx),
            MenuAct::Go(p) => {
                let r = self.st.borrow_mut().b.go_to(p);
                if let Err(e) = r {
                    self.message("Açılamadı", &e.to_string());
                }
                self.sync();
            }
            MenuAct::SetGrid(g) => {
                self.st.borrow_mut().b.settings.view = if g { ViewMode::Grid } else { ViewMode::List };
                self.save_settings();
                self.sync();
            }
            MenuAct::SetSort(k) => {
                let desc = self.st.borrow().b.settings.descending;
                let _ = self.st.borrow_mut().b.set_sort(k, desc);
                self.save_settings();
                self.sync();
            }
            MenuAct::ToggleDesc => {
                let (k, d) = {
                    let s = &self.st.borrow().b.settings;
                    (s.sort, s.descending)
                };
                let _ = self.st.borrow_mut().b.set_sort(k, !d);
                self.save_settings();
                self.sync();
            }
            MenuAct::ToggleFoldersFirst => {
                {
                    let mut st = self.st.borrow_mut();
                    st.b.settings.folders_first = !st.b.settings.folders_first;
                    let _ = st.b.refresh();
                }
                self.save_settings();
                self.sync();
            }
            MenuAct::ToggleHidden => {
                let _ = self.st.borrow_mut().b.toggle_hidden();
                self.save_settings();
                self.sync();
            }
            MenuAct::ToggleTime => {
                {
                    let mut st = self.st.borrow_mut();
                    st.b.settings.show_time = !st.b.settings.show_time;
                }
                self.save_settings();
                self.sync();
            }
            MenuAct::CycleTheme => {
                {
                    let mut st = self.st.borrow_mut();
                    st.b.settings.theme = (st.b.settings.theme + 1) % 3;
                }
                self.save_settings();
                self.sync();
            }
            MenuAct::MultiSelect => {
                self.st.borrow_mut().b.set_multi_select(true);
                self.sync();
            }
            MenuAct::SelectAll => {
                {
                    let mut st = self.st.borrow_mut();
                    st.b.multi_select = true;
                    st.b.select_all();
                }
                self.sync();
            }
            MenuAct::Invert => {
                self.st.borrow_mut().b.invert_selection();
                self.sync();
            }
            MenuAct::NewFolder => self.ask_text("Yeni klasör", "Klasör adı", "", "OLUŞTUR", Pending::NewFolder),
            MenuAct::NewFile => self.ask_text("Yeni dosya", "Dosya adı", "", "OLUŞTUR", Pending::NewFile),
            MenuAct::Refresh => self.refresh(),
            MenuAct::About => self.message("RArchiver", "Rust ile yazılmış arşiv ve dosya yöneticisi.\nSürüm 0.0.5 (M2)\nBiçimler: ZIP, 7z, TAR, TAR.GZ, TAR.BZ2, TAR.XZ, GZ, BZ2, XZ"),
        }
    }

    fn do_action(&self, a: Action, idx: Option<usize>) {
        let (paths, inner, cur_dir, name, archive_file, base, password) = {
            let st = self.st.borrow();
            let name = idx.and_then(|i| st.b.items.get(i)).map(|i| i.name.clone()).unwrap_or_default();
            (
                st.b.target_paths(idx),
                st.b.target_inner_paths(idx),
                st.b.current_dir(),
                name,
                st.b.archive_file(),
                st.b.archive_inner(),
                st.b.archive_password(),
            )
        };
        match a {
            Action::View | Action::Open | Action::OpenAsArchive => {
                if let (Action::Open, Some(f)) = (a, idx.and_then(|i| self.st.borrow().b.items.get(i).cloned())) {
                    if !(f.class == FileClass::Archive) && archive::detect_by_name(&f.name).is_none() {
                        self.message("Henüz yok", "Dosyaları harici uygulamayla açmak bir sonraki sürümde (FileProvider gerekiyor).");
                        return;
                    }
                }
                if let Some(p) = paths.first() {
                    self.start_op(Op::OpenArchive { file: p.clone(), password: None });
                }
            }
            Action::Install | Action::Share => self.message("Henüz yok", "Bu özellik bir sonraki sürümde gelecek (FileProvider gerekiyor)."),
            Action::ExtractHere | Action::ExtractToFolder => {
                let (archive, entries, base_, pw, dest) = if let Some(af) = archive_file {
                    // arşiv içindeyiz: seçilenleri arşivin klasörüne çıkar
                    let dest = af.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                    (af, inner, base, password, dest)
                } else {
                    let Some(ar) = paths.first().cloned() else { return };
                    let Some(cur) = cur_dir else { return };
                    let dest = if a == Action::ExtractToFolder {
                        let n = ar.file_name().unwrap_or_default().to_string_lossy().into_owned();
                        let stem = archive_stem(&n);
                        cur.join(stem)
                    } else {
                        cur
                    };
                    (ar, Vec::new(), String::new(), None, dest)
                };
                let names = top_level_names(&archive, pw.as_deref(), &entries, &base_);
                let conflicts: Vec<String> = names.into_iter().filter(|n| dest.join(n).exists()).collect();
                let op = Op::Extract { archive, dest, entries, base: base_, password: pw, policy: Conflict::Skip };
                self.run_or_ask_conflict(conflicts, op);
            }
            Action::ExtractTo => {
                let mut st = self.st.borrow_mut();
                if let Some(af) = archive_file {
                    st.b.begin_extract_to(af, inner, base, password);
                } else if let Some(p) = paths.first().cloned() {
                    st.b.begin_extract_to(p, Vec::new(), String::new(), None);
                }
                drop(st);
                self.sync();
            }
            Action::Test => {
                if let Some(p) = paths.first() {
                    self.start_op(Op::Test { archive: p.clone(), password: None });
                }
            }
            Action::Copy => {
                self.st.borrow_mut().b.begin_copy(idx);
                self.sync();
            }
            Action::Cut => {
                self.st.borrow_mut().b.begin_cut(idx);
                self.sync();
            }
            Action::Delete => {
                if paths.is_empty() {
                    return;
                }
                let msg = if paths.len() == 1 {
                    format!("\"{name}\" kalıcı olarak silinsin mi? Çöp kutusu yoktur.")
                } else {
                    format!("{} öğe kalıcı olarak silinsin mi? Çöp kutusu yoktur.", paths.len())
                };
                self.confirm("Sil", &msg, "SİL", Pending::Delete(paths));
            }
            Action::Rename => {
                if let Some(p) = paths.first() {
                    let n = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    self.ask_text("Yeniden adlandır", "Yeni ad", &n, "TAMAM", Pending::Rename(p.clone()));
                }
            }
            Action::Info => self.show_info(paths),
            Action::AddFavorite | Action::RemoveFavorite => {
                if let Some(p) = paths.first().cloned() {
                    {
                        let mut st = self.st.borrow_mut();
                        if a == Action::AddFavorite {
                            st.b.settings.add_favorite(&p);
                        } else {
                            st.b.settings.remove_favorite(&p);
                        }
                    }
                    self.save_settings();
                }
                self.sync();
            }
            Action::CompressZip => self.quick_compress(paths, ArchiveFormat::Zip),
            Action::Compress7z => self.quick_compress(paths, ArchiveFormat::SevenZ),
            Action::CompressDialog => self.open_compress_dialog(paths),
        }
    }

    // ---------- sıkıştırma ----------

    fn quick_compress(&self, sources: Vec<PathBuf>, fmt: ArchiveFormat) {
        let Some(cur) = self.st.borrow().b.current_dir() else {
            self.message("Sıkıştırılamaz", "Önce normal bir klasöre geçin.");
            return;
        };
        if sources.is_empty() {
            return;
        }
        let out = fsops::unique_path(&cur, &archive::default_archive_name(&sources, fmt));
        let opts = CompressOptions { format: fmt, ..Default::default() };
        self.run_job("Sıkıştırılıyor", None, move |ctx| {
            archive::compress(&sources, &out, &opts, ctx)?;
            Ok(JobOut::Done)
        });
    }

    fn open_compress_dialog(&self, sources: Vec<PathBuf>) {
        if sources.is_empty() || self.st.borrow().b.current_dir().is_none() {
            self.message("Sıkıştırılamaz", "Önce normal bir klasöre geçin.");
            return;
        }
        let name = {
            let mut st = self.st.borrow_mut();
            st.arc = ArcDlg { sources: sources.clone(), fmt: 1, level: 2, encrypt_names: false, delete: false, separate: false };
            st.pending = Pending::Compress;
            archive::default_archive_name(&sources, ArchiveFormat::ALL[1])
        };
        if let Some(ui) = self.ui() {
            ui.set_overlay_title("Arşiv oluştur".into());
            ui.set_input_text(name.into());
            ui.set_arc_password("".into());
        }
        self.sync_arc();
        self.set_overlay(6);
    }

    fn sync_arc(&self) {
        let Some(ui) = self.ui() else { return };
        let st = self.st.borrow();
        let fmt = ArchiveFormat::ALL[st.arc.fmt];
        ui.set_arc_format(fmt.extension().into());
        ui.set_arc_level(LEVELS[st.arc.level].0.into());
        ui.set_arc_pw_ok(fmt.supports_password());
        ui.set_arc_names_ok(fmt == ArchiveFormat::SevenZ);
        ui.set_arc_encrypt_names(st.arc.encrypt_names);
        ui.set_arc_delete(st.arc.delete);
        ui.set_arc_separate(st.arc.separate);
        ui.set_arc_sep_ok(st.arc.sources.len() > 1);
    }

    fn arc_pick_format(&self) {
        let new_name = {
            let mut st = self.st.borrow_mut();
            st.arc.fmt = (st.arc.fmt + 1) % ArchiveFormat::ALL.len();
            let fmt = ArchiveFormat::ALL[st.arc.fmt];
            archive::default_archive_name(&st.arc.sources, fmt)
        };
        if let Some(ui) = self.ui() {
            ui.set_input_text(new_name.into());
        }
        self.sync_arc();
    }

    fn arc_pick_level(&self) {
        {
            let mut st = self.st.borrow_mut();
            st.arc.level = (st.arc.level + 1) % LEVELS.len();
        }
        self.sync_arc();
    }

    fn arc_toggle_names(&self) {
        {
            let mut st = self.st.borrow_mut();
            st.arc.encrypt_names = !st.arc.encrypt_names;
        }
        self.sync_arc();
    }

    fn arc_toggle_delete(&self) {
        {
            let mut st = self.st.borrow_mut();
            st.arc.delete = !st.arc.delete;
        }
        self.sync_arc();
    }

    fn arc_toggle_separate(&self) {
        {
            let mut st = self.st.borrow_mut();
            st.arc.separate = !st.arc.separate;
        }
        self.sync_arc();
    }

    fn finish_compress_dialog(&self) {
        let Some(ui) = self.ui() else { return };
        let name = ui.get_input_text().to_string();
        let pw = ui.get_arc_password().to_string();
        let (sources, fmt, level, enc, del, sep, cur) = {
            let st = self.st.borrow();
            (
                st.arc.sources.clone(),
                ArchiveFormat::ALL[st.arc.fmt],
                LEVELS[st.arc.level].1,
                st.arc.encrypt_names,
                st.arc.delete,
                st.arc.separate,
                st.b.current_dir(),
            )
        };
        let Some(cur) = cur else { return };
        if let Err(e) = fsops::validate_name(&name) {
            self.message("Geçersiz ad", &e.to_string());
            return;
        }
        let opts = CompressOptions {
            format: fmt,
            level,
            password: if fmt.supports_password() && !pw.is_empty() { Some(pw) } else { None },
            encrypt_names: enc,
            delete_sources: del,
        };
        self.close_overlay();
        if sep && sources.len() > 1 {
            self.run_job("Sıkıştırılıyor", None, move |ctx| {
                archive::compress_separate(&sources, &cur, &opts, ctx)?;
                Ok(JobOut::Done)
            });
        } else {
            let out = cur.join(name.trim());
            self.run_job("Sıkıştırılıyor", None, move |ctx| {
                archive::compress(&sources, &out, &opts, ctx)?;
                Ok(JobOut::Done)
            });
        }
    }

    // ---------- bilgi ----------

    fn show_info(&self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        self.run_job("Bilgi", None, move |ctx| {
            let d = fsops::info(&paths, ctx)?;
            let mut t = String::new();
            if let Some((name, path, is_dir, mtime)) = &d.single {
                t += &format!("Ad: {name}\nYol: {path}\nTür: {}\n", if *is_dir { "Klasör" } else { "Dosya" });
                if let Some(m) = mtime {
                    t += &format!("Değiştirilme: {}\n", format_date(*m));
                }
            } else {
                t += &format!("{} öğe seçili\n", d.count);
            }
            t += &format!("Boyut: {} ({} bayt)\n", format_size(d.stats.bytes), d.stats.bytes);
            if d.single.as_ref().map(|s| s.2).unwrap_or(true) {
                t += &format!("İçerik: {} dosya, {} klasör", d.stats.files, d.stats.dirs.saturating_sub(if d.single.is_some() { 1 } else { 0 }));
            }
            Ok(JobOut::Text("Bilgi".into(), t))
        });
    }

    // ---------- yer paylaşımı sonuçları ----------

    fn overlay_ok(&self) {
        let Some(ui) = self.ui() else { return };
        let input = ui.get_input_text().to_string();
        let pending = std::mem::replace(&mut self.st.borrow_mut().pending, Pending::None);
        match pending {
            Pending::None | Pending::Message => self.set_overlay(0),
            Pending::NewFolder | Pending::NewFile => {
                let Some(dir) = self.st.borrow().b.current_dir() else { return };
                let r = if matches!(pending, Pending::NewFolder) { fsops::create_dir(&dir, &input) } else { fsops::create_file(&dir, &input) };
                self.set_overlay(0);
                match r {
                    Ok(_) => self.refresh(),
                    Err(e) => self.message("Oluşturulamadı", &e.to_string()),
                }
            }
            Pending::Rename(p) => {
                let r = fsops::rename(&p, &input);
                self.set_overlay(0);
                match r {
                    Ok(_) => self.refresh(),
                    Err(e) => self.message("Yeniden adlandırılamadı", &e.to_string()),
                }
            }
            Pending::Delete(paths) => {
                self.set_overlay(0);
                self.run_job("Siliniyor", None, move |ctx| {
                    fsops::delete(&paths, ctx)?;
                    Ok(JobOut::Done)
                });
            }
            Pending::Password(op) => {
                let pw = Some(input);
                let op = match op {
                    Op::Extract { archive, dest, entries, base, policy, .. } => Op::Extract { archive, dest, entries, base, password: pw, policy },
                    Op::Test { archive, .. } => Op::Test { archive, password: pw },
                    Op::OpenArchive { file, .. } => Op::OpenArchive { file, password: pw },
                    other => other,
                };
                ui.set_input_text("".into());
                self.set_overlay(0);
                self.start_op(op);
            }
            Pending::Compress => {
                self.st.borrow_mut().pending = Pending::Compress;
                self.finish_compress_dialog();
            }
            Pending::Conflict(_) => {}
        }
        if self.ui().map(|u| u.get_overlay() == 0).unwrap_or(false) {
            self.sync();
        }
    }

    fn overlay_cancel(&self) {
        let Some(ui) = self.ui() else { return };
        if ui.get_overlay() == 5 {
            if let Some(j) = &self.st.borrow().job {
                j.ctx.cancel.cancel();
            }
            ui.set_progress_current("İptal ediliyor...".into());
            return;
        }
        self.close_overlay();
        self.sync();
    }

    fn overlay_extra(&self, choice: i32) {
        let pending = std::mem::replace(&mut self.st.borrow_mut().pending, Pending::None);
        self.set_overlay(0);
        let Pending::Conflict(op) = pending else { return };
        let policy = match choice {
            0 => Conflict::Skip,
            1 => Conflict::Overwrite,
            _ => Conflict::KeepBoth,
        };
        let op = match op {
            Op::Paste { clip, dest, .. } => Op::Paste { clip, dest, policy },
            Op::Extract { archive, dest, entries, base, password, .. } => Op::Extract { archive, dest, entries, base, password, policy },
            other => other,
        };
        self.start_op(op);
    }

    // ---------- işler ----------

    fn start_op(&self, op: Op) {
        let retry = Some(op.clone());
        match op {
            Op::OpenArchive { file, password } => {
                let r = self.st.borrow_mut().b.open_archive(&file, password);
                match r {
                    Ok(()) => self.sync(),
                    Err(e @ (Error::NeedPassword | Error::WrongPassword)) => {
                        self.ask_password(&e, Op::OpenArchive { file, password: None });
                    }
                    Err(e) => {
                        self.sync();
                        self.message("Arşiv açılamadı", &e.to_string());
                    }
                }
            }
            Op::Paste { clip, dest, policy } => {
                let title = match clip.mode {
                    ClipMode::Cut => "Taşınıyor",
                    _ => "Kopyalanıyor",
                };
                self.run_job(title, retry, move |ctx| {
                    match clip.mode {
                        ClipMode::Cut => fsops::move_(&clip.paths, &dest, policy, ctx)?,
                        _ => fsops::copy(&clip.paths, &dest, policy, ctx)?,
                    }
                    Ok(JobOut::Done)
                });
                self.st.borrow_mut().b.cancel_paste();
            }
            Op::Extract { archive, dest, entries, base, password, policy } => {
                self.run_job("Çıkarılıyor", retry, move |ctx| {
                    archive::extract(&archive, &dest, &entries, &base, password.as_deref(), policy, ctx)?;
                    Ok(JobOut::Done)
                });
                self.st.borrow_mut().b.cancel_paste();
            }
            Op::Test { archive, password } => {
                self.run_job("Sınanıyor", retry, move |ctx| {
                    let r = archive::test(&archive, password.as_deref(), ctx)?;
                    let body = if r.errors.is_empty() {
                        format!("{} öğe sınandı, hata yok.", r.checked)
                    } else {
                        format!("{} öğe sınandı, {} hata:\n{}", r.checked, r.errors.len(), r.errors.join("\n"))
                    };
                    Ok(JobOut::Text("Sınama sonucu".into(), body))
                });
            }
        }
    }

    fn ask_password(&self, why: &Error, op: Op) {
        self.st.borrow_mut().pending = Pending::Password(op);
        if let Some(ui) = self.ui() {
            ui.set_overlay_title("Parola".into());
            ui.set_overlay_message(match why {
                Error::WrongPassword => "Parola yanlış, tekrar deneyin.".into(),
                _ => "Bu arşiv parola ile korunuyor.".into(),
            });
            ui.set_input_text("".into());
            ui.set_overlay(4);
        }
    }

    fn run_job(&self, title: &str, retry: Option<Op>, work: impl FnOnce(&Ctx) -> Result<JobOut, Error> + Send + 'static) {
        let ctx = Ctx::new();
        let slot: Arc<Mutex<Option<Result<JobOut, Error>>>> = Arc::new(Mutex::new(None));
        {
            let (ctx, slot) = (ctx.clone(), slot.clone());
            std::thread::spawn(move || {
                let r = work(&ctx);
                *slot.lock().unwrap() = Some(r);
            });
        }
        {
            let mut st = self.st.borrow_mut();
            st.job = Some(Job { ctx, slot, retry });
            st.b.select_none(); // işlem başlayınca seçim biter (ZArchiver gibi)
        }
        if let Some(ui) = self.ui() {
            ui.set_overlay_title(title.into());
            ui.set_progress_value(0.0);
            ui.set_progress_current("".into());
            ui.set_progress_text("".into());
            ui.set_overlay(5);
        }
        self.sync();
    }

    /// Her 120 ms: iş ilerlemesi, arama sonuçları ve izin kontrolü.
    fn poll(&self) {
        enum Next {
            None,
            Finished(Result<JobOut, Error>, Option<Op>),
        }
        let mut next = Next::None;
        {
            let mut st = self.st.borrow_mut();
            st.tick = st.tick.wrapping_add(1);
            if let Some(job) = &st.job {
                let snap = job.ctx.progress.snapshot();
                if let Some(ui) = self.ui() {
                    if ui.get_overlay() == 5 && !job.ctx.cancel.is_cancelled() {
                        ui.set_progress_value(snap.fraction());
                        ui.set_progress_current(snap.current.as_str().into());
                        ui.set_progress_text(
                            if snap.total_bytes > 0 {
                                format!("{} / {}", format_size(snap.done_bytes), format_size(snap.total_bytes))
                            } else {
                                format!("{} / {}", snap.done_files, snap.total_files)
                            }
                            .into(),
                        );
                    }
                }
                let res = job.slot.lock().unwrap().take();
                if let Some(r) = res {
                    let retry = job.retry.clone();
                    st.job = None;
                    next = Next::Finished(r, retry);
                }
            }
        }
        if let Next::Finished(r, retry) = next {
            self.finish_job(r, retry);
        }

        // arama
        let upd = {
            let mut st = self.st.borrow_mut();
            let mut upd = None;
            if let Some(s) = &mut st.search {
                let done = *s.done.lock().unwrap();
                let hits = s.hits.lock().unwrap();
                if hits.len() != s.shown || done {
                    s.shown = hits.len();
                    upd = Some((s.query.clone(), hits.clone(), done));
                }
            }
            upd
        };
        if let Some((q, hits, done)) = upd {
            {
                let mut st = self.st.borrow_mut();
                st.b.show_search_results(&q, hits);
                if done {
                    st.search = None;
                }
            }
            self.sync();
        }

        // izin ekranındayken ~1 sn'de bir denetle
        let need = {
            let st = self.st.borrow();
            !st.access && st.tick % 8 == 0
        };
        if need && self.plat.has_access() {
            self.refresh();
        }
    }

    fn finish_job(&self, r: Result<JobOut, Error>, retry: Option<Op>) {
        match r {
            Ok(JobOut::Done) => {
                self.set_overlay(0);
                self.refresh();
            }
            Ok(JobOut::Text(title, body)) => {
                self.refresh();
                self.st.borrow_mut().pending = Pending::Message;
                if let Some(ui) = self.ui() {
                    ui.set_overlay_title(title.as_str().into());
                    ui.set_overlay_message(body.as_str().into());
                    ui.set_overlay(7);
                }
            }
            Err(Error::Cancelled) => {
                self.set_overlay(0);
                self.refresh();
            }
            Err(e @ (Error::NeedPassword | Error::WrongPassword)) if retry.is_some() => {
                self.refresh();
                self.ask_password(&e, retry.unwrap());
            }
            Err(e) => {
                self.refresh();
                self.message("Hata", &e.to_string());
            }
        }
    }
}

fn archive_stem(name: &str) -> String {
    let l = name.to_lowercase();
    for ext in [".tar.gz", ".tar.bz2", ".tar.xz"] {
        if l.ends_with(ext) {
            return name[..name.len() - ext.len()].to_string();
        }
    }
    name.rsplit_once('.').map(|x| x.0.to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| format!("{name}_çıktı"))
}

/// Çıkarılacak öğelerin hedefteki üst düzey adları (çakışma denetimi için).
fn top_level_names(archive_file: &Path, password: Option<&str>, entries: &[String], base: &str) -> Vec<String> {
    let Ok(list) = archive::list(archive_file, password) else { return Vec::new() };
    let mut out: Vec<String> = Vec::new();
    for e in list {
        let selected = entries.is_empty() || entries.iter().any(|s| e.path == *s || e.path.starts_with(&format!("{s}/")));
        if !selected {
            continue;
        }
        let rel = if base.is_empty() { Some(e.path.as_str()) } else { e.path.strip_prefix(&format!("{base}/")) };
        if let Some(rel) = rel {
            let first = rel.split('/').next().unwrap_or("").to_string();
            if !first.is_empty() && !out.contains(&first) {
                out.push(first);
            }
        }
    }
    out
}

#[allow(dead_code)]
fn _unused(_: FileClass) -> FileClass {
    classify("x")
}

slint::include_modules!();

use rarchiver_core::{
    format_date, format_size, list_dir, parent_within, Entry, EntryKind, ListError, ListOptions,
};
use slint::android::AndroidApp;
use slint::{ModelRc, VecModel};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

const ROOT: &str = "/storage/emulated/0";
const PACKAGE_URI: &str = "package:com.kaizen.rarchiver";

// ---------- Android'e özgü minimal glue (JNI) ----------

fn with_env<R>(
    app: &AndroidApp,
    f: impl FnOnce(&mut jni::JNIEnv, jni::objects::JObject) -> jni::errors::Result<R>,
) -> Option<R> {
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr() as *mut jni::sys::JavaVM) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    let activity = unsafe { jni::objects::JObject::from_raw(app.activity_as_ptr() as jni::sys::jobject) };
    match f(&mut env, activity) {
        Ok(v) => Some(v),
        Err(_) => {
            let _ = env.exception_clear();
            None
        }
    }
}

/// Android 11+: "Tüm dosyalara erişim" verildi mi?
fn has_all_files_access(app: &AndroidApp) -> bool {
    with_env(app, |env, _| {
        env.call_static_method("android/os/Environment", "isExternalStorageManager", "()Z", &[])?.z()
    })
    .unwrap_or(false)
}

/// Uygulamanın "Tüm dosyalara erişim" ayar sayfasını açar.
fn open_all_files_settings(app: &AndroidApp) {
    use jni::objects::JValue;
    let _ = with_env(app, |env, activity| {
        let action = env.new_string("android.settings.MANAGE_APP_ALL_FILES_ACCESS_PERMISSION")?;
        let intent = env.new_object(
            "android/content/Intent",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&action)],
        )?;
        let uri_str = env.new_string(PACKAGE_URI)?;
        let uri = env
            .call_static_method(
                "android/net/Uri",
                "parse",
                "(Ljava/lang/String;)Landroid/net/Uri;",
                &[JValue::Object(&uri_str)],
            )?
            .l()?;
        env.call_method(
            &intent,
            "setData",
            "(Landroid/net/Uri;)Landroid/content/Intent;",
            &[JValue::Object(&uri)],
        )?;
        env.call_method(&activity, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent)])?;
        Ok(())
    });
}

// ---------- Uygulama durumu ----------

struct State {
    root: PathBuf,
    cur: PathBuf,
    entries: Vec<Entry>,
    has_up: bool,
}

fn info_text(e: &Entry) -> String {
    match e.kind {
        EntryKind::Dir => match e.children {
            Some(0) => "Boş".to_string(),
            Some(n) => format!("{n} öğe"),
            None => "—".to_string(),
        },
        EntryKind::File => format_size(e.size),
    }
}

fn refresh(ui: &MainWindow, st: &mut State, app: &AndroidApp) {
    st.has_up = parent_within(&st.cur, &st.root).is_some();
    ui.set_can_go_up(st.has_up);
    ui.set_path(st.cur.to_string_lossy().as_ref().into());
    let title = if st.cur == st.root {
        "Dahili depolama".to_string()
    } else {
        st.cur.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    };
    ui.set_folder_name(title.into());

    if !has_all_files_access(app) {
        st.entries.clear();
        ui.set_rows(ModelRc::new(VecModel::from(Vec::<FileRow>::new())));
        ui.set_message("RArchiver'ın dosyalarını gösterebilmesi için \"Tüm dosyalara erişim\" iznini açın. İzni verip geri döndüğünüzde liste otomatik yenilenir.".into());
        ui.set_screen(3);
        return;
    }

    match list_dir(&st.cur, &ListOptions::default()) {
        Ok(entries) => {
            st.entries = entries;
            let mut rows: Vec<FileRow> = Vec::new();
            if st.has_up {
                rows.push(FileRow {
                    name: "..".into(),
                    info: "".into(),
                    date: "".into(),
                    hidden: false,
                    is_dir: false,
                    is_up: true,
                });
            }
            rows.extend(st.entries.iter().map(|e| FileRow {
                name: e.name.clone().into(),
                info: info_text(e).into(),
                date: e.modified.map(format_date).unwrap_or_default().into(),
                hidden: e.hidden,
                is_dir: e.kind == EntryKind::Dir,
                is_up: false,
            }));
            ui.set_rows(ModelRc::new(VecModel::from(rows)));
            ui.set_message("".into());
            ui.set_screen(if st.entries.is_empty() { 1 } else { 0 });
        }
        Err(err) => {
            st.entries.clear();
            ui.set_rows(ModelRc::new(VecModel::from(Vec::<FileRow>::new())));
            let msg = match err {
                ListError::PermissionDenied => "Bu klasörü okuma izniniz yok.".to_string(),
                ListError::NotFound => "Klasör bulunamadı.".to_string(),
                ListError::NotADirectory => "Bu bir klasör değil.".to_string(),
                ListError::Other(s) => format!("Okunamadı: {s}"),
            };
            ui.set_message(msg.into());
            ui.set_screen(2);
        }
    }
}

#[no_mangle]
fn android_main(app: AndroidApp) {
    slint::android::init(app.clone()).unwrap();
    let ui = MainWindow::new().unwrap();

    let root = PathBuf::from(ROOT);
    let state = Rc::new(RefCell::new(State { root: root.clone(), cur: root, entries: Vec::new(), has_up: false }));
    refresh(&ui, &mut state.borrow_mut(), &app);

    {
        let (weak, st, app) = (ui.as_weak(), state.clone(), app.clone());
        ui.on_open(move |idx| {
            let Some(ui) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            let idx = idx as usize;
            if s.has_up && idx == 0 {
                if let Some(p) = parent_within(&s.cur, &s.root) {
                    s.cur = p;
                }
            } else {
                let i = idx - s.has_up as usize;
                let target = s.entries.get(i).filter(|e| e.kind == EntryKind::Dir).map(|e| s.cur.join(&e.name));
                match target {
                    Some(p) => s.cur = p,
                    None => return, // dosyalar M2'de
                }
            }
            refresh(&ui, &mut s, &app);
        });
    }
    {
        let (weak, st, app) = (ui.as_weak(), state.clone(), app.clone());
        ui.on_go_up(move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            if let Some(p) = parent_within(&s.cur, &s.root) {
                s.cur = p;
                refresh(&ui, &mut s, &app);
            }
        });
    }
    {
        let (weak, st, app) = (ui.as_weak(), state.clone(), app.clone());
        ui.on_retry(move || {
            if let Some(ui) = weak.upgrade() {
                refresh(&ui, &mut st.borrow_mut(), &app);
            }
        });
    }
    {
        let app = app.clone();
        ui.on_request_permission(move || open_all_files_settings(&app));
    }

    // İzin ekranındayken her saniye kontrol et: izin verilince liste kendiliğinden açılır.
    let timer = slint::Timer::default();
    {
        let (weak, st, app) = (ui.as_weak(), state.clone(), app.clone());
        timer.start(slint::TimerMode::Repeated, std::time::Duration::from_secs(1), move || {
            let Some(ui) = weak.upgrade() else { return };
            if ui.get_screen() == 3 && has_all_files_access(&app) {
                refresh(&ui, &mut st.borrow_mut(), &app);
            }
        });
    }

    ui.run().unwrap();
    drop(timer);
}

slint::include_modules!();

mod controller;

use controller::{Controller, Platform};
use slint::android::AndroidApp;
use slint::ComponentHandle;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

const START_DIR: &str = "/storage/emulated/0";
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
        let intent = env.new_object("android/content/Intent", "(Ljava/lang/String;)V", &[JValue::Object(&action)])?;
        let uri_str = env.new_string(PACKAGE_URI)?;
        let uri = env
            .call_static_method("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[JValue::Object(&uri_str)])?
            .l()?;
        env.call_method(&intent, "setData", "(Landroid/net/Uri;)Landroid/content/Intent;", &[JValue::Object(&uri)])?;
        env.call_method(&activity, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent)])?;
        Ok(())
    });
}

struct AndroidPlatform {
    app: AndroidApp,
}

impl Platform for AndroidPlatform {
    fn has_access(&self) -> bool {
        has_all_files_access(&self.app)
    }
    fn open_access_settings(&self) {
        open_all_files_settings(&self.app)
    }
    fn data_dir(&self) -> Option<PathBuf> {
        self.app.internal_data_path()
    }
}

/// Durum/gezinme çubuğu pay değerlerini (varsa) arayüze aktarır.
fn update_insets(app: &AndroidApp, ui: &MainWindow) {
    let scale = ui.window().scale_factor().max(0.5);
    let r = app.content_rect();
    let top = r.top.max(0) as f32 / scale;
    let bottom = app.native_window().map(|w| (w.height() - r.bottom).max(0) as f32 / scale).unwrap_or(0.0);
    ui.set_inset_top(top);
    ui.set_inset_bottom(bottom);
}

#[no_mangle]
fn android_main(app: AndroidApp) {
    slint::android::init(app.clone()).unwrap();
    let ui = MainWindow::new().unwrap();
    update_insets(&app, &ui);
    let ctl = Controller::install(&ui, Rc::new(AndroidPlatform { app: app.clone() }), PathBuf::from(START_DIR));

    // Döndürme vb. sonrası güvenli alanı güncel tut
    let insets = slint::Timer::default();
    {
        let (weak, app) = (ui.as_weak(), app.clone());
        insets.start(slint::TimerMode::Repeated, Duration::from_millis(700), move || {
            if let Some(ui) = weak.upgrade() {
                update_insets(&app, &ui);
            }
        });
    }

    ui.run().unwrap();
    drop(insets);
    drop(ctl);
}

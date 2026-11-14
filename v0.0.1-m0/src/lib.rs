slint::include_modules!();
use slint::{ModelRc, VecModel};

fn build_rows() -> Vec<FileRow> {
    let base = [
        ("..", "", "", false),
        (".dlprovider", "1 öğe", "1.06.26", true),
        (".trash-storage", "Boş", "26.07.26", true),
        ("Alarms", "Boş", "14.03.22", false),
        ("Android", "4 öğe", "10.12.23", false),
        ("Audiobooks", "Boş", "14.03.22", false),
        ("DCIM", "11 öğe", "22.03.26", false),
        ("Documents", "3 öğe", "08.02.26", false),
        ("Download", "67 öğe", "13.11.26", false),
        ("Movies", "6 öğe", "19.04.24", false),
        ("Music", "1 öğe", "14.03.22", false),
        ("Pictures", "9 öğe", "02.05.25", false),
    ];
    let mut v: Vec<FileRow> = base
        .iter()
        .map(|(n, i, d, h)| FileRow {
            name: (*n).into(),
            info: (*i).into(),
            date: (*d).into(),
            hidden: *h,
        })
        .collect();
    for n in 1..=200 {
        v.push(FileRow {
            name: format!("Klasör_{n}").into(),
            info: format!("{} öğe", n % 17).into(),
            date: "01.01.26".into(),
            hidden: false,
        });
    }
    v
}

#[no_mangle]
fn android_main(app: slint::android::AndroidApp) {
    slint::android::init(app).unwrap();
    let ui = MainWindow::new().unwrap();
    ui.set_rows(ModelRc::new(VecModel::from(build_rows())));
    ui.run().unwrap();
}

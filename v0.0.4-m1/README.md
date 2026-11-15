# v0.0.4-m1: çekirdek + gerçek dosya listesi
- `core/` (rarchiver-core): saf Rust. Dizin listeleme, sıralama, boyut/tarih biçimi. `cargo test` ile test edilir.
- `app/`: Slint arayüzü + minimal Android glue (JNI ile "Tüm dosyalara erişim" kontrolü/ayar sayfası).
- Gizli klasörler varsayılan görünür. `..` satırı ile üst klasöre dönülür.
- Ekranlar: liste, boş klasör, hata, izin gerekli.
- Bilinen sınırlar: tarihler UTC; Android geri tuşu uygulamayı kapatır (üstteki "<" kullanılır);
  dosyalara dokunma henüz işlevsiz; ikonlar geçici şekiller.

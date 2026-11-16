# v0.0.5-m2: ZArchiver arayüzü + backend

## Yapı
- `core/` (rarchiver-core, saf Rust, 30 birim testi)
  - `listing`: dizin okuma/sıralama, boyut ve tarih biçimi
  - `fsops`: klasör/dosya oluştur, yeniden adlandır, sil (kalıcı), kopyala, taşı, bilgi; çakışma politikaları (atla / üzerine yaz / ikisini de tut)
  - `job`: iptal ve ilerleme altyapısı
  - `search`: `*` ve `?` jokerleri, tür çipleri (arşiv, APK, resim, video, belge, ses), iptal edilebilir
  - `archive`: liste, çıkarma, sınama, oluşturma. ZIP (AES-256), 7z (AES-256, adları şifreleme), TAR, TAR.GZ, TAR.BZ2, TAR.XZ, GZ, BZ2, XZ. Zip-slip koruması, seçili çıkarma, "arşiv olarak aç" (içerikten tanıma)
  - `browser`: konum (klasör / arşiv içi / arama), seçim (ikon, aralık, hepsi), yapıştırma modu, menüler
  - `settings`: kalıcı ayarlar ve sık kullanılanlar
- `app/`: Slint arayüzü (`ui/`), denetleyici (`src/controller.rs`) ve Android glue (`src/lib.rs`, JNI)

## Arayüz (ZArchiver ekran görüntüsünden ölçüldü)
Satır 49,5dp, ikon 44x36dp, üst çubuk 55dp, FAB 56dp, sağda 7,7dp hızlı kaydırma şeridi,
başlık = klasör adı + üst yol + açılır üçgen, ".." satırı, öğe sayısı/boş, sağda tarih.
Üç tema: Koyu Mor (varsayılan), Açık Lavanta, Klasik (ZArchiver renkleri). Menü ⋮ > Tema ile değişir.

## Davranışlar
Dokunma: klasör aç / dosyada ana menü. İkona dokunma: seç. Uzun basma: ek menü; seçiliyken aralık seçimi;
".." uzun basma: tümünü seç/bırak. Yapıştırma modunda FAB yapıştır olur. Silme kalıcıdır ve onay ister.

## Bilinen sınırlar
- Dosyayı harici uygulamayla açma, APK yükleme, paylaşma henüz yok (FileProvider gerekiyor)
- RAR, LZ4, ZSTD ve bölünmüş (volume) arşivler henüz yok; ZIP şifreleme yalnızca AES-256
- Arşiv açma (liste) ana iş parçacığında yapılır; çok büyük arşivlerde kısa takılma olabilir
- Tarihler UTC; Android geri tuşu uygulamayı kapatır (üstteki geri oku kullanılır)
- Izgara görünümü çok büyük klasörlerde (binlerce öğe) yavaş olabilir

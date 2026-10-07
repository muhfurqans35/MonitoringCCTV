# Firmware dipindah ke paket terpisah

Firmware node pintu (ESP32-S3) sekarang ada di zip terpisah: `doorbell-firmware-esp-idf.zip`,
proyek ESP-IDF native (C), bukan lagi Arduino/PlatformIO. Protokol MQTT dan JSON-nya identik
dengan yang dipakai `crates/doorbell` di repo ini (`kost/door/{door_id}/...`), jadi tidak ada
perubahan yang diperlukan di sisi backend/hub.

Lihat `docs/DOORBELL.md` di repo ini untuk protokol lengkap, dan README di dalam zip firmware
untuk cara build (`idf.py build`) dan status pengujiannya.

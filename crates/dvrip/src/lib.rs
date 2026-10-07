//! Klien NVR XMEye/Xiongmai lama lewat protokol DVRIP **biner** (port 34567), hanya std.
//! Dipakai `api` (daftar rekaman + klip playback) dan `vision` (alarm motion seketika).

mod client;
mod md5;
mod time;
mod xm;

pub use client::{AlarmEvent, Clip, Error, Nvr, RecFile, Result};
pub use md5::sofia_hash;
pub use time::NvrTime;
pub use xm::{Frame, XmDemux};

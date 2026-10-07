//! Klien DVRIP biner (blocking, std::net). Dipanggil dari tokio lewat `spawn_blocking` / thread sendiri.
//!
//! Direkonstruksi dari rekaman Wireshark aplikasi CMS ke NVR XMEye lama (port 34567):
//!   header 20 byte LE: FF ver 00 00 | session u32 | seq u32 | tot u8 cur u8 | msg u16 | len u32
//!   1000/1001 login   : user[32] + sofia_hash[32] + u32 1 + u32 2  -> ret@8 (100 = OK), session@16
//!   1440/1441 cari    : maks 64 file per halaman, rekaman 184 byte mulai offset 0x18
//!   1424/1425 (koneksi data baru) klaim playback  -> lalu 1422 berisi isi file XM
//!   1420 (kontrol) a,b: 0,0 mulai | 0,4 seek | 4,0e jeda | 4,0f lanjut | 0,1 stop
//!   1500 langganan alarm -> 1504: ch@0x10 (0-based) event@0x14 (4 = motion) status@0x18 (0 mulai, 1 selesai) jam@0x1C
//!   1006 keepalive

use std::fmt;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::md5::sofia_hash;
use crate::time::NvrTime;
use crate::xm::XmDemux;

pub const MSG_LOGIN: u16 = 1000;
pub const MSG_KEEPALIVE: u16 = 1006;
pub const MSG_PLAY_CTRL: u16 = 1420;
pub const MSG_PLAY_DATA: u16 = 1422;
pub const MSG_PLAY_CLAIM: u16 = 1424;
pub const MSG_FIND: u16 = 1440;
pub const MSG_ALARM_SUB: u16 = 1500;
pub const MSG_ALARM: u16 = 1504;

#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error(format!("koneksi NVR: {e}"))
    }
}
pub type Result<T> = std::result::Result<T, Error>;

fn err<T>(s: impl Into<String>) -> Result<T> {
    Err(Error(s.into()))
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    if b.len() < i + 4 {
        return 0;
    }
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// Kode hasil di balasan (offset 4): 100 = OK.
fn ret_code(body: &[u8]) -> u32 {
    u32_at(body, 4)
}

// --------------------------------------------------------------------------------- koneksi

struct Conn {
    s: TcpStream,
    session: u32,
    seq: u32,
    rbuf: Vec<u8>,
}

impl Conn {
    fn connect(host: &str, port: u16, timeout: Duration) -> Result<Self> {
        let addr = (host, port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| Error(format!("alamat NVR {host}:{port} tidak valid")))?;
        let s = TcpStream::connect_timeout(&addr, timeout)
            .map_err(|e| Error(format!("tidak bisa tersambung ke NVR {host}:{port}: {e}")))?;
        s.set_nodelay(true).ok();
        s.set_write_timeout(Some(timeout)).ok();
        Ok(Conn { s, session: 0, seq: 0, rbuf: Vec::new() })
    }

    fn send(&mut self, msg: u16, body: &[u8], seq: Option<u32>) -> Result<()> {
        let seq = seq.unwrap_or_else(|| {
            self.seq += 1;
            self.seq
        });
        let mut b = Vec::with_capacity(20 + body.len());
        b.extend_from_slice(&[0xFF, 0, 0, 0]);
        b.extend_from_slice(&self.session.to_le_bytes());
        b.extend_from_slice(&seq.to_le_bytes());
        b.extend_from_slice(&[0, 0]);
        b.extend_from_slice(&msg.to_le_bytes());
        b.extend_from_slice(&(body.len() as u32).to_le_bytes());
        b.extend_from_slice(body);
        self.s.write_all(&b)?;
        Ok(())
    }

    /// Satu pesan utuh, atau None bila `wait` habis. Aman terhadap timeout di tengah pesan
    /// (sisa byte disimpan di buffer, tidak hilang).
    fn recv(&mut self, wait: Duration) -> Result<Option<(u16, Vec<u8>)>> {
        let deadline = Instant::now() + wait;
        loop {
            if self.rbuf.len() >= 20 {
                if self.rbuf[0] != 0xFF {
                    return err(format!("header NVR tidak dikenal: {:02x?}", &self.rbuf[..20]));
                }
                let msg = u16::from_le_bytes([self.rbuf[14], self.rbuf[15]]);
                let len = u32_at(&self.rbuf, 16) as usize;
                if len > 32 * 1024 * 1024 {
                    return err(format!("pesan NVR terlalu besar ({len} byte)"));
                }
                if self.rbuf.len() >= 20 + len {
                    let body = self.rbuf[20..20 + len].to_vec();
                    self.rbuf.drain(..20 + len);
                    return Ok(Some((msg, body)));
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            self.s.set_read_timeout(Some(left.max(Duration::from_millis(10))))?;
            let mut tmp = [0u8; 64 * 1024];
            match self.s.read(&mut tmp) {
                Ok(0) => return err("NVR menutup koneksi"),
                Ok(n) => self.rbuf.extend_from_slice(&tmp[..n]),
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => return Ok(None),
                Err(e) => return Err(e.into()),
            }
        }
    }

    fn recv_msg(&mut self, want: u16, wait: Duration) -> Result<Vec<u8>> {
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.recv(left)? {
                Some((m, b)) if m == want => return Ok(b),
                Some(_) => continue,
                None => return err(format!("NVR tidak membalas (menunggu pesan {want})")),
            }
        }
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        let _ = self.s.shutdown(Shutdown::Both);
    }
}

// --------------------------------------------------------------------------------- data

#[derive(Debug, Clone, PartialEq)]
pub struct RecFile {
    /// Nama lengkap di NVR, mis. /mnt/idea0/2026-10-06/005/07.56.23-07.56.43[M][@25e88][1].h264
    pub name: String,
    pub size_kb: u32,
    pub begin: NvrTime,
    pub end: NvrTime,
    /// Huruf jenis dari nama file: 'M' motion, 'A' alarm, 'R' biasa, 'H' manual, '?' tidak diketahui.
    pub kind: char,
}

impl RecFile {
    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            'M' => "motion",
            'A' => "alarm",
            'R' => "regular",
            'H' => "manual",
            _ => "other",
        }
    }

    fn from_record(r: &[u8]) -> Option<Self> {
        if r.len() < 180 {
            return None;
        }
        let name_raw = &r[8..116];
        let end_nul = name_raw.iter().position(|&c| c == 0).unwrap_or(name_raw.len());
        let name = String::from_utf8_lossy(&name_raw[..end_nul]).to_string();
        if name.is_empty() {
            return None;
        }
        let begin = NvrTime::from_wire(&r[116..148])?;
        let end = NvrTime::from_wire(&r[148..180])?;
        let kind = name.split('[').nth(1).and_then(|s| s.chars().next()).unwrap_or('?');
        Some(RecFile { name, size_kb: u32_at(r, 4), begin, end, kind })
    }
}

#[derive(Debug, Clone)]
pub struct AlarmEvent {
    /// Nomor CAM 1-based (sama dengan channel_number di nvr.toml).
    pub channel: u32,
    /// 4 = motion, 1 = alarm input, 5 = video hilang, 6 = kamera tertutup.
    pub event: u32,
    pub start: bool,
    pub status: u32,
    pub time: Option<NvrTime>,
}

impl AlarmEvent {
    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < 0x1C + 28 {
            return None;
        }
        let status = u32_at(body, 0x18);
        Some(AlarmEvent {
            channel: u32_at(body, 0x10) + 1,
            event: u32_at(body, 0x14),
            start: status == 0,
            status,
            time: NvrTime::from_wire(&body[0x1C..]),
        })
    }

    pub fn event_name(&self) -> &'static str {
        match self.event {
            4 => "motion",
            1 => "alarm_input",
            5 => "video_loss",
            6 => "video_blind",
            _ => "other",
        }
    }
}

#[derive(Debug, Default)]
pub struct Clip {
    /// H.264 Annex-B murni (siap di-remux ffmpeg ke MP4 tanpa encode ulang).
    pub h264: Vec<u8>,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    /// Jam OSD I-frame pertama yang diambil.
    pub first_time: Option<NvrTime>,
    pub files: Vec<String>,
}

impl Clip {
    pub fn seconds(&self) -> f64 {
        if self.fps == 0 {
            0.0
        } else {
            self.frames as f64 / self.fps as f64
        }
    }
}

// --------------------------------------------------------------------------------- klien

#[derive(Debug, Clone)]
pub struct Nvr {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub timeout: Duration,
}

fn play_body(session: u32, a: u32, b: u32, f: &RecFile, t_begin: NvrTime, t_end: NvrTime) -> Vec<u8> {
    let mut v = vec![0u8; 200];
    v[0..4].copy_from_slice(&session.to_le_bytes());
    v[8..12].copy_from_slice(&a.to_le_bytes());
    v[12..16].copy_from_slice(&b.to_le_bytes());
    let name = f.name.as_bytes();
    let n = name.len().min(107);
    v[0x14..0x14 + n].copy_from_slice(&name[..n]);
    v[0x80..0xA0].copy_from_slice(&t_begin.to_wire());
    v[0xA0..0xC0].copy_from_slice(&t_end.to_wire());
    v
}

impl Nvr {
    pub fn new(host: impl Into<String>, port: u16, user: impl Into<String>, password: impl Into<String>) -> Self {
        Nvr { host: host.into(), port, user: user.into(), password: password.into(), timeout: Duration::from_secs(8) }
    }

    fn login(&self) -> Result<Conn> {
        let mut c = Conn::connect(&self.host, self.port, self.timeout)?;
        let mut body = vec![0u8; 72];
        let u = self.user.as_bytes();
        body[..u.len().min(31)].copy_from_slice(&u[..u.len().min(31)]);
        let h = sofia_hash(&self.password);
        body[32..32 + h.len()].copy_from_slice(h.as_bytes());
        body[64..68].copy_from_slice(&1u32.to_le_bytes());
        body[68..72].copy_from_slice(&2u32.to_le_bytes());
        c.send(MSG_LOGIN, &body, Some(0))?;
        let rsp = c.recv_msg(MSG_LOGIN + 1, self.timeout)?;
        let ret = u32_at(&rsp, 8);
        if ret != 100 {
            return err(format!("login NVR gagal (kode {ret}); cek username/password di nvr.toml"));
        }
        c.session = u32_at(&rsp, 16);
        Ok(c)
    }

    /// Cek koneksi + login saja.
    pub fn ping(&self) -> Result<u32> {
        Ok(self.login()?.session)
    }

    /// Semua rekaman satu kamera (1-based) dalam rentang waktu, urut waktu.
    pub fn find_files(&self, channel: u32, from: NvrTime, to: NvrTime) -> Result<Vec<RecFile>> {
        let mut c = self.login()?;
        let ch0 = channel.saturating_sub(1);
        let mut files: Vec<RecFile> = Vec::new();
        let mut cursor = from;
        for _ in 0..200 {
            let mut b = vec![0u8; 140];
            b[0..4].copy_from_slice(&c.session.to_le_bytes());
            b[8..12].copy_from_slice(&ch0.to_le_bytes());
            b[12..16].copy_from_slice(&ch0.to_le_bytes());
            b[0x10..0x14].copy_from_slice(b"h264");
            b[0x28..0x2C].copy_from_slice(&0x2Au32.to_le_bytes());
            b[0x48..0x4C].copy_from_slice(&0xFFFFu32.to_le_bytes());
            b[0x4C..0x6C].copy_from_slice(&cursor.to_wire());
            b[0x6C..0x8C].copy_from_slice(&to.to_wire());
            c.send(MSG_FIND, &b, None)?;
            let rsp = c.recv_msg(MSG_FIND + 1, self.timeout * 2)?;
            if ret_code(&rsp) != 100 {
                break; // tidak ada (lagi) rekaman
            }
            let count = u32_at(&rsp, 0x10).min(64) as usize;
            let mut page = Vec::new();
            for k in 0..count {
                let off = 0x18 + k * 184;
                if off + 180 > rsp.len() {
                    break;
                }
                if let Some(f) = RecFile::from_record(&rsp[off..off + 180]) {
                    page.push(f);
                }
            }
            let full = count >= 64;
            let last_end = page.last().map(|f| f.end);
            for f in page {
                if !files.iter().any(|x| x.name == f.name) {
                    files.push(f);
                }
            }
            match last_end {
                Some(e) if full && e < to => cursor = e.add_secs(1),
                _ => break,
            }
        }
        files.sort_by(|a, b| a.begin.cmp(&b.begin));
        Ok(files)
    }

    /// Ambil video [from, from+seconds) satu kamera; bisa melintasi beberapa file rekaman.
    /// NVR mengirim kira-kira 2x waktu nyata, jadi klip 30 dtk butuh ±15 dtk.
    pub fn clip(&self, channel: u32, from: NvrTime, seconds: u32) -> Result<Clip> {
        let to = from.add_secs(seconds as i64);
        let files: Vec<RecFile> = self
            .find_files(channel, from.add_secs(-3 * 3600), to)?
            .into_iter()
            .filter(|f| f.end > from && f.begin < to)
            .collect();
        if files.is_empty() {
            return err(format!("tidak ada rekaman CAM{channel} pada {from}"));
        }
        let deadline = Instant::now() + Duration::from_secs(seconds as u64 + 40);
        let mut clip = Clip::default();
        for f in &files {
            let need = seconds as f64 - clip.seconds();
            if need < 0.5 || Instant::now() >= deadline {
                break;
            }
            let start = if f.begin > from { f.begin } else { from };
            let want = need.min(f.end.secs_since(&start).max(1) as f64);
            self.play_file(f, start, want, deadline, &mut clip)?;
            clip.files.push(f.name.clone());
        }
        if clip.frames == 0 {
            return err("NVR tidak mengirim video untuk rentang itu");
        }
        Ok(clip)
    }

    fn play_file(&self, f: &RecFile, from: NvrTime, want_secs: f64, deadline: Instant, clip: &mut Clip) -> Result<()> {
        let mut ctl = self.login()?;
        let mut data = Conn::connect(&self.host, self.port, self.timeout)?;
        data.session = ctl.session;
        data.send(MSG_PLAY_CLAIM, &play_body(ctl.session, 0, 7, f, f.begin, f.end), Some(0))?;
        let rsp = data.recv_msg(MSG_PLAY_CLAIM + 1, self.timeout)?;
        if ret_code(&rsp) != 100 {
            return err(format!("NVR menolak playback (kode {})", ret_code(&rsp)));
        }
        ctl.send(MSG_PLAY_CTRL, &play_body(ctl.session, 0, 0, f, f.begin, f.end), None)?;
        let seeking = from.secs_since(&f.begin) > 2;
        if seeking {
            ctl.send(MSG_PLAY_CTRL, &play_body(ctl.session, 0, 4, f, from, f.end), None)?;
        }

        let mut dm = XmDemux::new();
        let mut started = false;
        let (mut first, mut prev): (Option<NvrTime>, Option<NvrTime>) = (None, None);
        let mut shift: i64 = 0; // lompatan jam (seek) yang tidak dihitung sebagai durasi
        let mut file_frames = 0u32;
        let mut last_ka = Instant::now();
        let idle = Duration::from_secs(8);
        'outer: while Instant::now() < deadline {
            if last_ka.elapsed() >= Duration::from_secs(15) {
                let _ = ctl.send(MSG_KEEPALIVE, &[], Some(0));
                last_ka = Instant::now();
            }
            let (msg, body) = match data.recv(idle) {
                Ok(Some(m)) => m,
                Ok(None) | Err(_) => break, // NVR berhenti mengirim / menutup = akhir file
            };
            if msg != MSG_PLAY_DATA {
                continue;
            }
            if body.is_empty() {
                break;
            }
            for fr in dm.feed(&body) {
                if fr.key {
                    if let Some(t) = fr.time {
                        if !started {
                            // Saat seek, abaikan sisa data sebelum posisi yang diminta (toleransi 5 dtk).
                            if !seeking || t.epoch() + 5 >= from.epoch() {
                                started = true;
                                first = Some(t);
                            }
                        } else if let Some(p) = prev {
                            let d = t.secs_since(&p);
                            if !(0..=10).contains(&d) {
                                shift += d;
                            }
                        }
                        if started {
                            prev = Some(t);
                            let got = t.secs_since(&first.unwrap()) - shift;
                            if got as f64 >= want_secs {
                                break 'outer; // berhenti sebelum GOP berikutnya
                            }
                        }
                    }
                }
                if started {
                    if clip.first_time.is_none() {
                        clip.first_time = first;
                    }
                    clip.h264.extend_from_slice(&fr.data);
                    clip.frames += 1;
                    file_frames += 1;
                }
            }
            // batas aman berdasarkan jumlah frame (kalau jam OSD tidak terbaca)
            if dm.fps > 0 && file_frames as f64 / dm.fps as f64 >= want_secs + 1.0 {
                break;
            }
        }
        let _ = ctl.send(MSG_PLAY_CTRL, &play_body(ctl.session, 0, 1, f, f.begin, f.end), None);
        if dm.fps > 0 {
            clip.fps = dm.fps;
            clip.width = dm.width;
            clip.height = dm.height;
        }
        Ok(())
    }

    /// Berlangganan alarm NVR; `on_event` dipanggil untuk tiap 1504. Kembali saat `stop` di-set
    /// atau koneksi putus (pemanggil yang mengatur sambung ulang).
    pub fn alarm_loop(&self, stop: &AtomicBool, mut on_event: impl FnMut(AlarmEvent)) -> Result<()> {
        let mut c = self.login()?;
        let mut b = vec![0u8; 12];
        b[0..4].copy_from_slice(&c.session.to_le_bytes());
        b[4..8].copy_from_slice(&0x5DCu32.to_le_bytes());
        c.send(MSG_ALARM_SUB, &b, None)?;
        let mut last_ka = Instant::now();
        let mut last_rx = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            if last_ka.elapsed() >= Duration::from_secs(15) {
                c.send(MSG_KEEPALIVE, &[], Some(0))?;
                last_ka = Instant::now();
            }
            if last_rx.elapsed() >= Duration::from_secs(60) {
                return err("NVR tidak menjawab keepalive 60 dtk");
            }
            match c.recv(Duration::from_secs(1))? {
                Some((msg, body)) => {
                    last_rx = Instant::now();
                    if msg == MSG_ALARM {
                        if let Some(ev) = AlarmEvent::parse(&body) {
                            on_event(ev);
                        }
                    } else if msg == MSG_ALARM_SUB + 1 && ret_code(&body) != 100 {
                        return err(format!("NVR menolak langganan alarm (kode {})", ret_code(&body)));
                    }
                }
                None => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_alarm_from_real_nvr() {
        // Pesan 1504 asli dari NVR (CAM5 motion, jam NVR 20:15:37)
        let hex = "d6000000640000000000000000000000040000000400000000000000ea0700000a000000060000000200000014000000\
                   0f000000250000000000000000";
        let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
        let mut body = b.clone();
        body.resize(60, 0);
        let ev = AlarmEvent::parse(&body).unwrap();
        assert_eq!(ev.channel, 5);
        assert_eq!(ev.event_name(), "motion");
        assert!(ev.start);
        assert_eq!(ev.time.unwrap().to_string(), "2026-10-06T20:15:37");
    }

    #[test]
    fn parses_file_record_kind() {
        let mut r = vec![0u8; 184];
        let name = b"/mnt/idea0/2026-10-06/005/07.56.23-07.56.43[M][@25e88][1].h264";
        r[8..8 + name.len()].copy_from_slice(name);
        r[4..8].copy_from_slice(&10558u32.to_le_bytes());
        r[116..148].copy_from_slice(&NvrTime::parse("2026-10-06 07:56:23").unwrap().to_wire());
        r[148..180].copy_from_slice(&NvrTime::parse("2026-10-06 07:56:43").unwrap().to_wire());
        let f = RecFile::from_record(&r).unwrap();
        assert_eq!(f.kind, 'M');
        assert_eq!(f.kind_name(), "motion");
        assert_eq!(f.end.secs_since(&f.begin), 20);
        // rekaman sampah (jam tidak valid) diabaikan
        r[116..120].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        assert!(RecFile::from_record(&r).is_none());
    }
}

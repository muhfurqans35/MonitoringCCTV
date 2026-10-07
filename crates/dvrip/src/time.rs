//! Waktu lokal NVR (tanpa zona waktu): NVR menyimpan & menerima jam dinding miliknya sendiri.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NvrTime {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

impl NvrTime {
    pub fn new(year: u32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> Option<Self> {
        let t = NvrTime { year, month, day, hour, minute, second };
        t.is_valid().then_some(t)
    }

    pub fn is_valid(&self) -> bool {
        (2000..=2099).contains(&self.year)
            && (1..=12).contains(&self.month)
            && (1..=31).contains(&self.day)
            && self.hour < 24
            && self.minute < 60
            && self.second < 60
            && {
                // tanggal benar-benar ada (mis. bukan 31 Feb)
                let back = NvrTime::from_epoch(self.epoch());
                back.day == self.day && back.month == self.month
            }
    }

    /// Detik sejak 1970-01-01 00:00:00 (jam dinding NVR, bukan UTC sungguhan).
    pub fn epoch(&self) -> i64 {
        days_from_civil(self.year as i64, self.month as i64, self.day as i64) * 86400
            + self.hour as i64 * 3600
            + self.minute as i64 * 60
            + self.second as i64
    }

    pub fn from_epoch(s: i64) -> Self {
        let (y, m, d) = civil_from_days(s.div_euclid(86400));
        let r = s.rem_euclid(86400);
        NvrTime {
            year: y as u32,
            month: m as u32,
            day: d as u32,
            hour: (r / 3600) as u32,
            minute: (r % 3600 / 60) as u32,
            second: (r % 60) as u32,
        }
    }

    pub fn add_secs(&self, s: i64) -> Self {
        NvrTime::from_epoch(self.epoch() + s)
    }

    pub fn secs_since(&self, other: &NvrTime) -> i64 {
        self.epoch() - other.epoch()
    }

    /// "2026-10-06T07:56:23", "2026-10-06 07:56:23" atau "2026-10-06" (jam 00:00:00).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let (date, time) = match s.find(['T', ' ']) {
            Some(i) => (&s[..i], &s[i + 1..]),
            None => (s, "00:00:00"),
        };
        let d: Vec<u32> = date.split('-').map(|x| x.parse().ok()).collect::<Option<_>>()?;
        let mut t: Vec<u32> = time.split(':').map(|x| x.parse().ok()).collect::<Option<_>>()?;
        if d.len() != 3 || t.is_empty() || t.len() > 3 {
            return None;
        }
        t.resize(3, 0);
        NvrTime::new(d[0], d[1], d[2], t[0], t[1], t[2])
    }

    pub fn date_string(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    pub fn start_of_day(&self) -> Self {
        NvrTime { hour: 0, minute: 0, second: 0, ..*self }
    }

    pub fn end_of_day(&self) -> Self {
        NvrTime { hour: 23, minute: 59, second: 59, ..*self }
    }

    /// 8 x u32 LE: tahun, bulan, tanggal, hari-minggu(0), jam, menit, detik, 0 — persis seperti CMS.
    pub fn to_wire(&self) -> [u8; 32] {
        let mut b = [0u8; 32];
        for (i, v) in [self.year, self.month, self.day, 0, self.hour, self.minute, self.second, 0].iter().enumerate() {
            b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        b
    }

    pub fn from_wire(b: &[u8]) -> Option<Self> {
        if b.len() < 28 {
            return None;
        }
        let u = |i: usize| u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
        NvrTime::new(u(0), u(1), u(2), u(4), u(5), u(6))
    }

    /// Jam yang dikemas dalam header I-frame file XM.
    pub fn from_xm_packed(v: u32) -> Option<Self> {
        NvrTime::new((v >> 26) + 2000, (v >> 22) & 15, (v >> 17) & 31, (v >> 12) & 31, (v >> 6) & 63, v & 63)
    }
}

impl fmt::Display for NvrTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_format_and_arith() {
        let t = NvrTime::parse("2026-10-06 07:56:23").unwrap();
        assert_eq!(t.to_string(), "2026-10-06T07:56:23");
        assert_eq!(NvrTime::parse("2026-10-06T07:56:23"), Some(t));
        assert_eq!(NvrTime::parse("2026-10-06").unwrap().to_string(), "2026-10-06T00:00:00");
        assert_eq!(t.add_secs(3600 * 17).to_string(), "2026-10-07T00:56:23");
        assert_eq!(t.add_secs(-8 * 3600).to_string(), "2026-10-05T23:56:23");
        assert_eq!(t.add_secs(20).secs_since(&t), 20);
        assert!(NvrTime::parse("2026-02-31").is_none());
        assert!(NvrTime::parse("x").is_none());
        assert_eq!(NvrTime::from_wire(&t.to_wire()), Some(t));
    }

    #[test]
    fn xm_packed_time_from_capture() {
        // I-frame pertama rekaman 00.00.00 di file pcap CMS
        assert_eq!(NvrTime::from_xm_packed(1787559936).unwrap().to_string(), "2026-10-06T00:00:00");
    }
}

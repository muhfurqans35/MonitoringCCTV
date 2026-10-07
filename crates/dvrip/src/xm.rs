//! Demux aliran file rekaman XM (isi file `.h264` di NVR) menjadi frame H.264 Annex-B.
//!
//! Setiap frame diawali header XM:
//!   00 00 01 FD  I-frame: [5]=fps, [6]=lebar/8, [7]=tinggi/8, [8..12]=jam terkemas, [12..16]=panjang (16 byte)
//!   00 00 01 FC  P-frame: [4..8]=panjang (8 byte)
//!   00 00 01 FA/F9/F0/F1  audio / info / data pintar: [6..8]=panjang u16 (8 byte) — dibuang
//! Isi I/P-frame sudah berupa NAL Annex-B (00 00 00 01 67 ... SPS/PPS ada di setiap I-frame).

use crate::time::NvrTime;

#[derive(Debug, Clone)]
pub struct Frame {
    pub key: bool,
    /// Jam OSD (hanya I-frame).
    pub time: Option<NvrTime>,
    pub data: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct XmDemux {
    buf: Vec<u8>,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
}

impl XmDemux {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, data: &[u8]) -> Vec<Frame> {
        self.buf.extend_from_slice(data);
        let b = &self.buf;
        let mut out = Vec::new();
        let mut i = 0usize;
        loop {
            if b.len() - i < 8 {
                break;
            }
            if b[i..i + 3] != [0, 0, 1] {
                match find_start(&b[i + 1..]) {
                    Some(j) => {
                        i += 1 + j;
                        continue;
                    }
                    None => {
                        i = b.len().saturating_sub(3).max(i);
                        break;
                    }
                }
            }
            let t = b[i + 3];
            let (hl, len) = match t {
                0xFD => {
                    if b.len() - i < 16 {
                        break;
                    }
                    (16, u32::from_le_bytes([b[i + 12], b[i + 13], b[i + 14], b[i + 15]]) as usize)
                }
                0xFC => (8, u32::from_le_bytes([b[i + 4], b[i + 5], b[i + 6], b[i + 7]]) as usize),
                0xFA | 0xF9 | 0xF0 | 0xF1 => (8, u16::from_le_bytes([b[i + 6], b[i + 7]]) as usize),
                _ => {
                    i += 3; // bukan header XM -> cari lagi
                    continue;
                }
            };
            if len > 8 * 1024 * 1024 {
                i += 3; // panjang mustahil: data rusak, sinkron ulang
                continue;
            }
            if b.len() - i < hl + len {
                break;
            }
            if t == 0xFD || t == 0xFC {
                let mut time = None;
                if t == 0xFD {
                    if b[i + 5] > 0 {
                        self.fps = b[i + 5] as u32;
                    }
                    self.width = b[i + 6] as u32 * 8;
                    self.height = b[i + 7] as u32 * 8;
                    time = NvrTime::from_xm_packed(u32::from_le_bytes([b[i + 8], b[i + 9], b[i + 10], b[i + 11]]));
                }
                out.push(Frame { key: t == 0xFD, time, data: b[i + hl..i + hl + len].to_vec() });
            }
            i += hl + len;
        }
        self.buf.drain(..i);
        out
    }
}

fn find_start(b: &[u8]) -> Option<usize> {
    b.windows(3).position(|w| w == [0, 0, 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iframe(t: u32, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![0, 0, 1, 0xFD, 2, 25, 240, 135];
        v.extend_from_slice(&t.to_le_bytes());
        v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        v.extend_from_slice(payload);
        v
    }

    fn pframe(payload: &[u8]) -> Vec<u8> {
        let mut v = vec![0, 0, 1, 0xFC];
        v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn splits_across_arbitrary_chunks_and_drops_audio() {
        let mut s = iframe(1787559936, &[0, 0, 0, 1, 0x67, 1, 2, 3]);
        s.extend(pframe(&[0, 0, 0, 1, 0x41, 9]));
        s.extend([0, 0, 1, 0xFA, 0, 0, 3, 0, 7, 7, 7]); // audio 3 byte
        s.extend(pframe(&[0, 0, 0, 1, 0x41, 8]));
        let mut d = XmDemux::new();
        let mut frames = Vec::new();
        for c in s.chunks(5) {
            frames.extend(d.feed(c));
        }
        assert_eq!(frames.len(), 3);
        assert!(frames[0].key && !frames[1].key && !frames[2].key);
        assert_eq!(frames[0].time.unwrap().to_string(), "2026-10-06T00:00:00");
        assert_eq!((d.fps, d.width, d.height), (25, 1920, 1080));
        assert_eq!(frames[2].data, vec![0, 0, 0, 1, 0x41, 8]);
    }
}

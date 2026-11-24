//! Header-only MXF reader. All reads (including KLV framing) share a 512 KiB budget.
use crate::{hex, MediaMetadata, Rational};
use anyhow::{bail, ensure, Context, Result};
use std::{
    collections::HashMap,
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    path::Path,
};
#[path = "tags.rs"]
mod tags;
pub const READ_LIMIT: usize = 512 * 1024;
const PRIMER: &str = "060e2b34020501010d01020101050100";
const PROJECT: &str = "a5fb7b25f61594b962fc3717492d42bf";
/// Decode definite BER lengths; reject indefinite, oversized and truncated encodings.
pub fn ber(b: &[u8]) -> Result<(usize, usize)> {
    ensure!(!b.is_empty(), "missing BER length");
    if b[0] < 128 {
        return Ok((b[0] as usize, 1));
    }
    let n = (b[0] & 127) as usize;
    ensure!(n > 0 && n <= 8 && b.len() > n, "invalid BER length");
    let mut value = 0usize;
    for x in &b[1..=n] {
        value = value
            .checked_mul(256)
            .and_then(|v| v.checked_add(*x as usize))
            .context("BER overflow")?;
    }
    Ok((value, n + 1))
}
// Restrict buffering to the declared header extent, so read-ahead cannot touch essence.
struct HeaderReader<R> {
    inner: R,
    end: u64,
    bytes_read: usize,
}
impl<R: Read + Seek> Read for HeaderReader<R> {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.end.saturating_sub(self.inner.stream_position()?);
        let n = b
            .len()
            .min(remaining.min(usize::MAX as u64) as usize)
            .min(READ_LIMIT - self.bytes_read);
        if n == 0 && !b.is_empty() && remaining > 0 {
            return Err(std::io::Error::other("MXF read budget exceeded"));
        }
        let read = self.inner.read(&mut b[..n])?;
        self.bytes_read += read;
        Ok(read)
    }
}
impl<R: Seek> Seek for HeaderReader<R> {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(p)
    }
}
struct Budget<R> {
    r: R,
    used: usize,
}
impl<R: Read> Budget<R> {
    fn take(&mut self, n: usize) -> Result<Vec<u8>> {
        ensure!(
            n <= READ_LIMIT - self.used,
            "MXF header exceeds read budget"
        );
        let mut b = vec![0; n];
        self.r.read_exact(&mut b)?;
        self.used += n;
        Ok(b)
    }
    fn framing(&mut self) -> Result<(Vec<u8>, usize)> {
        let k = self.take(16)?;
        let mut l = self.take(1)?;
        if l[0] & 128 != 0 {
            let n = (l[0] & 127) as usize;
            ensure!(n > 0 && n <= 8, "invalid BER");
            l.extend(self.take(n)?);
        }
        Ok((k, ber(&l)?.0))
    }
}
#[derive(Debug)]
struct Set {
    key: String,
    fields: HashMap<u16, Vec<u8>>,
}
fn text(b: &[u8]) -> String {
    String::from_utf16_lossy(
        &b.as_chunks::<2>()
            .0
            .iter()
            .map(|x| u16::from_be_bytes([x[0], x[1]]))
            .collect::<Vec<_>>(),
    )
    .trim_end_matches('\0')
    .to_string()
}
fn number(b: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(b.try_into().ok()?))
}
fn rate(b: &[u8]) -> Option<Rational> {
    if b.len() != 8 {
        return None;
    }
    let n = i32::from_be_bytes(b[..4].try_into().ok()?);
    let d = i32::from_be_bytes(b[4..].try_into().ok()?);
    (d > 0 && n > 0).then_some(Rational {
        numerator: n,
        denominator: d,
    })
}
fn date(b: &[u8]) -> Option<String> {
    if b.len() != 8 {
        return None;
    }
    Some(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        u16::from_be_bytes([b[0], b[1]]),
        b[2],
        b[3],
        b[4],
        b[5],
        b[6],
        u16::from(b[7]) * 4
    ))
}
fn field(s: &Set, t: u16) -> Option<&[u8]> {
    s.fields.get(&t).map(Vec::as_slice)
}
fn partition(k: &[u8]) -> bool {
    k.len() == 16
        && k[..13] == [6, 14, 43, 52, 2, 5, 1, 1, 13, 1, 2, 1, 1]
        && matches!(k[13], 2..=4)
}
fn essence(k: &[u8]) -> bool {
    k.len() == 16 && k[..4] == [6, 14, 43, 52] && k[4] == 1 && k[8..12] == [13, 1, 3, 1]
}
pub fn read(path: &Path) -> Result<MediaMetadata> {
    parse(File::open(path)?)
}
pub fn parse<R: Read + Seek>(reader: R) -> Result<MediaMetadata> {
    let mut r = Budget { r: reader, used: 0 };
    let (k, n) = r.framing()?;
    ensure!(partition(&k) && k[13] == 2, "missing MXF header partition");
    let p = r.take(n)?;
    ensure!(p.len() >= 64, "short partition");
    let declared = u64::from_be_bytes(p[32..40].try_into()?);
    let header_end =
        r.r.stream_position()?
            .checked_add(declared)
            .context("header extent overflow")?;
    ensure!(declared > 0, "empty or open header metadata extent");
    let mut r = Budget {
        used: r.used,
        r: BufReader::with_capacity(
            16 * 1024,
            HeaderReader {
                inner: r.r,
                end: header_end,
                bytes_read: r.used,
            },
        ),
    };
    let mut primer = HashMap::new();
    let mut sets = Vec::new();
    let mut project = None;
    // Stop before reading any essence value or a body/index/footer partition.
    loop {
        let pos = r.r.stream_position()?;
        let framing = r.framing();
        let (k, n) = match framing {
            Ok(v) => v,
            Err(e) => {
                if r.used < READ_LIMIT && r.r.stream_position()? == pos {
                    break;
                }
                return Err(e);
            }
        };
        if partition(&k) || essence(&k) {
            break;
        }
        if hex(&k) == "060e2b34010101010301021001000000" {
            let pos = r.r.stream_position()?;
            let end = pos.checked_add(n as u64).context("filler overflow")?;
            ensure!(end <= header_end + 4096, "filler beyond header extent");
            r.r.seek(SeekFrom::Start(end))?;
            continue;
        }
        ensure!(n <= READ_LIMIT - r.used, "header KLV exceeds budget");
        let v = r.take(n)?;
        if hex(&k) == PRIMER {
            ensure!(v.len() >= 8, "short primer");
            let count = number(&v[..4]).context("primer count")? as usize;
            ensure!(
                number(&v[4..8]) == Some(18) && count <= (v.len() - 8) / 18,
                "invalid primer batch"
            );
            for x in v[8..8 + count * 18].as_chunks::<18>().0 {
                primer.insert(u16::from_be_bytes([x[0], x[1]]), hex(&x[2..]));
            }
        } else if k[4..6] == [2, 0x53] {
            let mut fields = HashMap::new();
            let mut i = 0;
            while i < v.len() {
                ensure!(v.len() - i >= 4, "truncated local item");
                let tag = u16::from_be_bytes([v[i], v[i + 1]]);
                let len = u16::from_be_bytes([v[i + 2], v[i + 3]]) as usize;
                i += 4;
                ensure!(len <= v.len() - i, "truncated local value");
                let value = &v[i..i + len];
                i += len;
                if let Some(ul) = primer.get(&tag) {
                    if ul == PROJECT {
                        let s = text(value);
                        if !s.is_empty() {
                            project = Some(s);
                        }
                    }
                    if let Some((canonical, _)) = tags::TAGS.iter().find(|(_, u)| u == ul) {
                        fields.insert(*canonical, value.to_vec());
                    }
                }
            }
            sets.push(Set {
                key: hex(&k),
                fields,
            });
        }
        // HeaderByteCount is advisory across Avid versions; KLV boundaries remain authoritative.
        if r.used == READ_LIMIT {
            bail!("MXF header budget exhausted")
        }
    }
    ensure!(!primer.is_empty(), "missing primer");
    let mut m = MediaMetadata {
        project,
        ..Default::default()
    };
    let material = sets.iter().find(|s| s.key.ends_with("013600"));
    if let Some(s) = material {
        m.material_package_uid = field(s, 0x4401).map(hex);
        m.clip_name = field(s, 0x4402).map(text);
        m.created = field(s, 0x4405).and_then(date);
        m.modified = field(s, 0x4404).and_then(date);
    }
    let source = sets.iter().filter(|s| s.key.ends_with("013700")).find(|s| {
        field(s, 0x4701).is_some_and(|id| {
            sets.iter()
                .any(|d| field(d, 0x3c0a) == Some(id) && field(d, 0x3001).is_some())
        })
    });
    if let Some(s) = source {
        m.file_package_uid = field(s, 0x4401).map(hex);
        if let Some(d) = sets.iter().find(|d| field(d, 0x3c0a) == field(s, 0x4701)) {
            m.edit_rate = field(d, 0x3001).and_then(rate);
            m.duration = field(d, 0x3002).and_then(|b| b.try_into().ok().map(i64::from_be_bytes));
            m.width = field(d, 0x3203).and_then(number);
            m.height = field(d, 0x3202).and_then(number);
            m.sample_rate = field(d, 0x3d03).and_then(rate);
            m.bit_depth = field(d, 0x3301)
                .or_else(|| field(d, 0x3d01))
                .and_then(number)
                .or_else(|| {
                    field(d, 0x3401).and_then(|b| {
                        b.as_chunks::<2>()
                            .0
                            .iter()
                            .take_while(|x| x[0] != 0)
                            .map(|x| u32::from(x[1]))
                            .max()
                    })
                });
            let audio = m.sample_rate.is_some();
            m.track_kind = Some(
                if audio {
                    "audio"
                } else if m.width.is_some() {
                    "video"
                } else {
                    "data"
                }
                .into(),
            );
            m.codec_ul = field(d, 0x3201).or_else(|| field(d, 0x3d06)).map(hex);
            m.codec = if audio && d.key.ends_with("014800") {
                Some("PCM".into())
            } else {
                m.codec_ul.as_deref().map(codec_name).map(str::to_owned)
            };
        }
    }
    ensure!(
        m.file_package_uid.is_some(),
        "no supported file source package descriptor"
    );
    // Do not guess captured/imported/render from a filename: provenance needs verified private properties.
    Ok(m)
}
pub fn codec_name(ul: &str) -> &'static str {
    if ul.starts_with("060e2b340401") && ul.get(16..24) == Some("04010202") {
        match ul.get(24..28) {
            Some("7124") => "DNxHR 444",
            Some("7125") => "DNxHR HQX",
            Some("7126") => "DNxHR HQ",
            Some("7127") => "DNxHR SQ",
            Some("7128") => "DNxHR LB",
            Some("0307") => "DNxUncompressed",
            Some("0306") => "ProRes",
            Some("0201") => "DV",
            Some("0131" | "0132") => "H.264",
            Some("0301") => "JPEG 2000",
            Some("0302") => "DNxHD / DNxHR",
            Some("0101" | "0102" | "0103" | "0104") => "MPEG-2",
            Some(v)
                if v.starts_with("71")
                    && u8::from_str_radix(&v[2..], 16).is_ok_and(|p| (1..=0x18).contains(&p)) =>
            {
                "DNxHD"
            }
            Some(v) if v.starts_with("71") => "DNxHD / DNxHR",
            _ => "Unknown",
        }
    } else if ul.starts_with("060e2b340401") && ul.get(16..28) == Some("0e0402010211") {
        "ProRes"
    } else if ul.starts_with("060e2b340401") && ul.get(16..24) == Some("04010201") {
        "Uncompressed"
    } else if ul.contains("04020201") {
        "PCM"
    } else {
        "Unknown"
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[test]
    fn lengths() {
        assert_eq!(ber(&[0x82, 1, 0]).unwrap(), (256, 3));
        for b in [&[0x80][..], &[0x89][..], &[0x82, 1][..]] {
            assert!(ber(b).is_err());
        }
    }
    #[test]
    fn headers() {
        for (b, kind) in [
            (
                include_bytes!("../../../tests/fixtures/audio.header").as_slice(),
                "audio",
            ),
            (
                include_bytes!("../../../tests/fixtures/video.header").as_slice(),
                "video",
            ),
            (
                include_bytes!("../../../tests/fixtures/rgba.header").as_slice(),
                "video",
            ),
        ] {
            let m = parse(Cursor::new(b)).unwrap();
            assert_eq!(m.project.as_deref(), Some("BD Demo Reel"));
            assert_eq!(m.track_kind.as_deref(), Some(kind));
            assert!(m.duration.unwrap() > 0);
            assert!(m.edit_rate.unwrap().denominator > 0);
            if kind == "audio" {
                assert_eq!(m.codec.as_deref(), Some("PCM"));
                assert_eq!(m.bit_depth, Some(16));
            } else if m.codec_ul.as_deref() == Some("060e2b340401010d0401020271270000") {
                assert_eq!(m.codec.as_deref(), Some("DNxHR SQ"));
                assert_eq!(m.bit_depth, Some(8));
            } else {
                assert_eq!(m.codec.as_deref(), Some("DNxUncompressed"));
                assert_eq!(m.bit_depth, Some(16));
            }
        }
    }
}

#[cfg(test)]
mod safety_tests {
    use super::*;
    use std::io::Cursor;
    #[test]
    fn primer_remapping() {
        let mut b = include_bytes!("../../../tests/fixtures/video.header").to_vec();
        let project_ul = [
            0xa5, 0xfb, 0x7b, 0x25, 0xf6, 0x15, 0x94, 0xb9, 0x62, 0xfc, 0x37, 0x17, 0x49, 0x2d,
            0x42, 0xbf,
        ];
        let mut pos = 0;
        let mut primer_seen = false;
        while pos + 17 <= b.len() {
            let (n, l) = ber(&b[pos + 16..]).unwrap();
            let start = pos + 16 + l;
            let end = start + n;
            if hex(&b[pos..pos + 16]) == PRIMER {
                for i in (start + 8..end).step_by(18) {
                    if b[i + 2..i + 18] == project_ul {
                        b[i..i + 2].copy_from_slice(&[0xfe, 0x12]);
                        primer_seen = true;
                    }
                }
            } else if b[pos + 4..pos + 6] == [2, 0x53] {
                let mut i = start;
                while i + 4 <= end {
                    let n = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
                    if b[i..i + 2] == [0xff, 0xfd] {
                        b[i..i + 2].copy_from_slice(&[0xfe, 0x12]);
                    }
                    i += 4 + n;
                }
            }
            pos = end;
        }
        assert!(primer_seen);
        assert_eq!(
            parse(Cursor::new(b)).unwrap().project.as_deref(),
            Some("BD Demo Reel")
        );
    }
    #[test]
    fn corrupt_prefixes_do_not_panic() {
        let b = include_bytes!("../../../tests/fixtures/video.header");
        for n in (0..b.len()).step_by(997) {
            assert!(parse(Cursor::new(&b[..n])).is_err());
        }
    }
    #[test]
    fn essence_value_never_read() {
        struct Guard {
            c: Cursor<Vec<u8>>,
            limit: u64,
            read: usize,
        }
        impl Read for Guard {
            fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
                assert!(self.c.position() <= self.limit);
                assert!(
                    self.c.position() + b.len() as u64 <= self.limit,
                    "attempted essence read"
                );
                let n = self.c.read(b)?;
                self.read += n;
                assert!(self.read <= READ_LIMIT);
                Ok(n)
            }
        }
        impl Seek for Guard {
            fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
                self.c.seek(p)
            }
        }
        let mut b = include_bytes!("../../../tests/fixtures/video.header").to_vec();
        let limit = b.len() as u64;
        let declared = limit - 129;
        b[57..65].copy_from_slice(&declared.to_be_bytes());
        b.extend([0x55; 100]);
        let m = parse(Guard {
            c: Cursor::new(b),
            limit,
            read: 0,
        })
        .unwrap();
        assert_eq!(m.project.as_deref(), Some("BD Demo Reel"));
    }
}

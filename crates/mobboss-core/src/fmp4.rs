//! Bounded ISO BMFF pipe reader. No filesystem paths or media bytes leave memory.
use anyhow::{bail, ensure, Context, Result};
use std::io::Read;

pub const MAX_BOX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Track {
    pub id: u32,
    pub timescale: u32,
    pub video: bool,
    pub audio: bool,
    pub channels: u16,
    pub codec: String,
    default_duration: u32,
}
#[derive(Clone, Debug)]
pub struct Init {
    pub bytes: Vec<u8>,
    pub tracks: Vec<Track>,
    pub mime: String,
}
#[derive(Clone, Debug)]
pub struct Fragment {
    pub bytes: Vec<u8>,
    pub start: f64,
    pub end: f64,
}
pub enum Segment {
    Init(Init),
    Media(Fragment),
}

fn u32_at(b: &[u8], p: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(
        b.get(p..p + 4).context("short MP4 integer")?.try_into()?,
    ))
}
fn u64_at(b: &[u8], p: usize) -> Result<u64> {
    Ok(u64::from_be_bytes(
        b.get(p..p + 8).context("short MP4 integer")?.try_into()?,
    ))
}
fn boxes(mut bytes: &[u8]) -> Result<Vec<(&[u8], &[u8])>> {
    let mut result = Vec::new();
    while !bytes.is_empty() {
        ensure!(bytes.len() >= 8, "truncated MP4 box");
        let n = u32_at(bytes, 0)? as usize;
        let (size, header) = if n == 1 {
            (usize::try_from(u64_at(bytes, 8)?)?, 16)
        } else {
            (n, 8)
        };
        ensure!(
            size >= header && size <= bytes.len(),
            "invalid MP4 box length"
        );
        result.push((&bytes[4..8], &bytes[header..size]));
        bytes = &bytes[size..];
    }
    Ok(result)
}
fn child<'a>(b: &'a [u8], name: &[u8]) -> Result<&'a [u8]> {
    boxes(b)?
        .into_iter()
        .find(|(kind, _)| *kind == name)
        .map(|(_, body)| body)
        .context("missing MP4 child")
}
pub fn parse_init(bytes: Vec<u8>) -> Result<Init> {
    let moov = child(&bytes, b"moov")?;
    let mut tracks = Vec::new();
    for (kind, trak) in boxes(moov)? {
        if kind != b"trak" {
            continue;
        }
        let tkhd = child(trak, b"tkhd")?;
        let id = u32_at(tkhd, if tkhd.first() == Some(&1) { 20 } else { 12 })?;
        let mdia = child(trak, b"mdia")?;
        let mdhd = child(mdia, b"mdhd")?;
        let timescale = u32_at(mdhd, if mdhd.first() == Some(&1) { 20 } else { 12 })?;
        ensure!(timescale > 0, "zero MP4 timescale");
        let handler = child(mdia, b"hdlr")?;
        let video = handler.get(8..12) == Some(b"vide");
        let audio = handler.get(8..12) == Some(b"soun");
        if !video && !audio {
            continue;
        }
        let stsd = child(child(child(mdia, b"minf")?, b"stbl")?, b"stsd")?;
        let (kind, entry) = boxes(stsd.get(8..).context("short sample descriptions")?)?
            .into_iter()
            .next()
            .context("missing MP4 sample description")?;
        let codec = match kind {
            b"avc1" => {
                let avcc = child(entry.get(78..).context("short video description")?, b"avcC")?;
                ensure!(avcc.len() >= 4, "short avcC");
                format!("avc1.{:02X}{:02X}{:02X}", avcc[1], avcc[2], avcc[3])
            }
            b"vp09" => {
                let vpcc = child(entry.get(78..).context("short video description")?, b"vpcC")?;
                ensure!(vpcc.len() >= 7, "short vpcC");
                format!("vp09.{:02}.{:02}.{:02}", vpcc[4], vpcc[5], vpcc[6] >> 4)
            }
            b"mp4a" => "mp4a.40.2".into(), // native AAC is explicitly configured as LC
            b"Opus" => "opus".into(),
            _ => bail!("unsupported streaming MP4 codec"),
        };
        let channels = if audio {
            u16::from_be_bytes(
                entry
                    .get(16..18)
                    .context("short audio description")?
                    .try_into()?,
            )
        } else {
            0
        };
        tracks.push(Track {
            id,
            timescale,
            video,
            audio,
            channels,
            codec,
            default_duration: 0,
        });
    }
    ensure!(!tracks.is_empty(), "MP4 has no playable tracks");
    let mvex = child(moov, b"mvex")?;
    for (kind, trex) in boxes(mvex)? {
        if kind == b"trex" {
            let id = u32_at(trex, 4)?;
            if let Some(t) = tracks.iter_mut().find(|t| t.id == id) {
                t.default_duration = u32_at(trex, 12)?;
            }
        }
    }
    let media = if tracks.iter().any(|t| t.video) {
        "video"
    } else {
        "audio"
    };
    let codecs = tracks
        .iter()
        .map(|t| t.codec.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let mime = format!("{media}/mp4; codecs=\"{codecs}\"");
    Ok(Init {
        bytes,
        tracks,
        mime,
    })
}
fn fragment_range(moof: &[u8], tracks: &[Track]) -> Result<(f64, f64)> {
    let mut ranges = Vec::new();
    for (kind, traf) in boxes(moof)? {
        if kind != b"traf" {
            continue;
        }
        let tfhd = child(traf, b"tfhd")?;
        let flags = u32_at(tfhd, 0)? & 0xffffff;
        let id = u32_at(tfhd, 4)?;
        let Some(track) = tracks.iter().find(|t| t.id == id) else {
            continue;
        };
        let offset = 8 + if flags & 1 != 0 { 8 } else { 0 } + if flags & 2 != 0 { 4 } else { 0 };
        let default_duration = if flags & 8 != 0 {
            u32_at(tfhd, offset)?
        } else {
            track.default_duration
        };
        let tfdt = child(traf, b"tfdt")?;
        let start = if tfdt.first() == Some(&1) {
            u64_at(tfdt, 4)?
        } else {
            u32_at(tfdt, 4)? as u64
        };
        let mut duration = 0u64;
        for (kind, trun) in boxes(traf)? {
            if kind != b"trun" {
                continue;
            }
            let flags = u32_at(trun, 0)? & 0xffffff;
            let count = u32_at(trun, 4)? as usize;
            ensure!(count <= 1_000_000, "excessive MP4 sample count");
            let mut p = 8 + if flags & 1 != 0 { 4 } else { 0 } + if flags & 4 != 0 { 4 } else { 0 };
            for _ in 0..count {
                let d = if flags & 0x100 != 0 {
                    u32_at(trun, p)?
                } else {
                    default_duration
                };
                duration = duration
                    .checked_add(d as u64)
                    .context("MP4 duration overflow")?;
                for bit in [0x100, 0x200, 0x400, 0x800] {
                    if flags & bit != 0 {
                        p += 4;
                    }
                }
                ensure!(p <= trun.len(), "truncated MP4 samples");
            }
        }
        let scale = track.timescale as f64;
        ranges.push((
            track.video,
            start as f64 / scale,
            start
                .checked_add(duration)
                .context("MP4 timestamp overflow")? as f64
                / scale,
        ));
    }
    // Video defines the seek timeline; AAC/Opus priming can start a few ms earlier.
    let video = ranges.iter().any(|r| r.0);
    let selected: Vec<_> = ranges.iter().filter(|r| !video || r.0).collect();
    ensure!(!selected.is_empty(), "fragment has no track timestamps");
    Ok((
        selected.iter().map(|r| r.1).fold(f64::INFINITY, f64::min),
        selected.iter().map(|r| r.2).fold(0.0, f64::max),
    ))
}

pub struct Splitter<R> {
    reader: R,
    init: Vec<u8>,
    tracks: Vec<Track>,
    pending: Vec<u8>,
    range: Option<(f64, f64)>,
}

impl<R: Read> Splitter<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            init: Vec::new(),
            tracks: Vec::new(),
            pending: Vec::new(),
            range: None,
        }
    }
    pub fn next_segment(&mut self) -> Result<Option<Segment>> {
        loop {
            let mut header = [0u8; 8];
            let n = self.reader.read(&mut header[..1])?;
            if n == 0 {
                ensure!(
                    self.pending.is_empty() && self.init.is_empty(),
                    "truncated MP4 stream"
                );
                return Ok(None);
            }
            self.reader.read_exact(&mut header[1..])?;
            let mut data = header.to_vec();
            let n = u32_at(&header, 0)? as usize;
            let size = if n == 1 {
                let mut extended = [0; 8];
                self.reader.read_exact(&mut extended)?;
                data.extend_from_slice(&extended);
                usize::try_from(u64::from_be_bytes(extended))?
            } else {
                n
            };
            ensure!(
                (data.len()..=MAX_BOX_BYTES).contains(&size),
                "invalid or excessive MP4 box length"
            );
            let h = data.len();
            data.resize(size, 0);
            self.reader.read_exact(&mut data[h..])?;
            match &header[4..8] {
                b"ftyp" | b"moov" => {
                    ensure!(self.tracks.is_empty(), "duplicate MP4 initialization");
                    self.init.extend_from_slice(&data);
                    ensure!(
                        self.init.len() <= 1024 * 1024,
                        "excessive MP4 initialization"
                    );
                    if &header[4..8] == b"moov" {
                        let init = parse_init(std::mem::take(&mut self.init))?;
                        self.tracks = init.tracks.clone();
                        return Ok(Some(Segment::Init(init)));
                    }
                }
                b"moof" => {
                    ensure!(
                        !self.tracks.is_empty() && self.pending.is_empty(),
                        "unexpected MP4 fragment"
                    );
                    self.range = Some(fragment_range(&data[h..], &self.tracks)?);
                    self.pending = data;
                }
                b"mdat" => {
                    let (start, end) = self.range.take().context("MP4 media without fragment")?;
                    self.pending.extend_from_slice(&data);
                    return Ok(Some(Segment::Media(Fragment {
                        bytes: std::mem::take(&mut self.pending),
                        start,
                        end,
                    })));
                }
                _ => {} // optional indexes/free boxes are not media segments
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_truncated_unbounded_and_oversized_boxes() {
        for bytes in [
            vec![0, 0, 0],
            b"\0\0\0\0mdat".to_vec(),
            [
                ((MAX_BOX_BYTES + 1) as u32).to_be_bytes().as_slice(),
                b"mdat",
            ]
            .concat(),
            b"\0\0\0\x10mdatshort".to_vec(),
            b"\0\0\0\x08mdat".to_vec(),
        ] {
            assert!(Splitter::new(std::io::Cursor::new(bytes))
                .next_segment()
                .is_err());
        }
    }
    #[test]
    fn extended_box_bounds_and_empty_stream() {
        assert!(Splitter::new(std::io::empty())
            .next_segment()
            .unwrap()
            .is_none());
        let bytes = [
            1u32.to_be_bytes().as_slice(),
            b"free",
            16u64.to_be_bytes().as_slice(),
        ]
        .concat();
        assert!(Splitter::new(std::io::Cursor::new(bytes))
            .next_segment()
            .unwrap()
            .is_none());
        let bad = [
            1u32.to_be_bytes().as_slice(),
            b"free",
            u64::MAX.to_be_bytes().as_slice(),
        ]
        .concat();
        assert!(Splitter::new(std::io::Cursor::new(bad))
            .next_segment()
            .is_err());
    }
}

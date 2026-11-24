//! Small bounded EBML reader for the Linux WebKitGTK MSE compatibility transport.
use crate::fmp4::{Fragment, Init, Segment, MAX_BOX_BYTES};
use anyhow::{ensure, Context, Result};
use std::io::Read;

struct Element {
    id: u32,
    header: Vec<u8>,
    size: Option<usize>,
}
fn element(reader: &mut impl Read) -> Result<Option<Element>> {
    let mut first = [0];
    if reader.read(&mut first)? == 0 {
        return Ok(None);
    }
    let id_len = first[0].leading_zeros() as usize + 1;
    ensure!(id_len <= 4, "invalid EBML ID");
    let mut header = vec![0; id_len];
    header[0] = first[0];
    reader.read_exact(&mut header[1..])?;
    let id = header.iter().fold(0u32, |n, b| (n << 8) | *b as u32);
    reader.read_exact(&mut first)?;
    let size_len = first[0].leading_zeros() as usize + 1;
    ensure!(size_len <= 8, "invalid EBML length");
    let mut encoded = vec![0; size_len];
    encoded[0] = first[0];
    reader.read_exact(&mut encoded[1..])?;
    let mask = 0xffu8.checked_shr(size_len as u32).unwrap_or(0);
    let mut size = u64::from(encoded[0] & mask);
    let unknown = encoded[0] & mask == mask && encoded[1..].iter().all(|b| *b == 255);
    for b in &encoded[1..] {
        size = (size << 8) | u64::from(*b);
    }
    header.extend_from_slice(&encoded);
    Ok(Some(Element {
        id,
        header,
        size: if unknown {
            None
        } else {
            Some(usize::try_from(size)?)
        },
    }))
}
pub struct Splitter<R> {
    reader: R,
    init: Vec<u8>,
    pending: Option<Fragment>,
    initialized: bool,
    mime: String,
}
impl<R: Read> Splitter<R> {
    pub fn new(reader: R, video: bool, opus: bool) -> Self {
        let mime = format!(
            "{}/webm; codecs=\"{}{}\"",
            if video { "video" } else { "audio" },
            if video { "vp9," } else { "" },
            if opus { "opus" } else { "vorbis" }
        );
        Self {
            reader,
            init: Vec::new(),
            pending: None,
            initialized: false,
            mime,
        }
    }
    pub fn next_segment(&mut self) -> Result<Option<Segment>> {
        if let Some(fragment) = self.pending.take() {
            return Ok(Some(Segment::Media(fragment)));
        }
        loop {
            let Some(mut e) = element(&mut self.reader)? else {
                ensure!(self.init.is_empty(), "incomplete WebM initialization");
                return Ok(None);
            };
            if e.id == 0x18538067 {
                // Segment is an unbounded container on a pipe.
                self.init.extend(e.header);
                continue;
            }
            let size = e.size.context("unbounded WebM child")?;
            ensure!(size <= MAX_BOX_BYTES, "excessive WebM element");
            let header = e.header.len();
            e.header.resize(header + size, 0);
            self.reader.read_exact(&mut e.header[header..])?;
            if e.id == 0x1f43b675 {
                let mut body = std::io::Cursor::new(&e.header[header..]);
                let mut timestamp = None;
                while let Some(child) = element(&mut body)? {
                    let size = child.size.context("unbounded WebM cluster child")?;
                    let start = body.position() as usize;
                    ensure!(
                        start
                            .checked_add(size)
                            .is_some_and(|n| n <= body.get_ref().len()),
                        "truncated WebM cluster"
                    );
                    if child.id == 0xe7 {
                        ensure!(size <= 8, "invalid WebM timestamp");
                        timestamp = Some(
                            body.get_ref()[start..start + size]
                                .iter()
                                .fold(0u64, |n, b| (n << 8) | u64::from(*b)),
                        );
                        break;
                    }
                    body.set_position((start + size) as u64);
                }
                let start = timestamp.context("WebM cluster has no timestamp")? as f64 / 1000.0;
                let fragment = Fragment {
                    bytes: e.header,
                    start,
                    end: start + 1.0,
                };
                if !self.initialized {
                    self.initialized = true;
                    self.pending = Some(fragment);
                    return Ok(Some(Segment::Init(Init {
                        bytes: std::mem::take(&mut self.init),
                        tracks: vec![],
                        mime: self.mime.clone(),
                    })));
                }
                return Ok(Some(Segment::Media(fragment)));
            } else if !self.initialized {
                self.init.extend(e.header);
                ensure!(
                    self.init.len() <= 1024 * 1024,
                    "excessive WebM initialization"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_vints_and_truncated_elements() {
        for bytes in [
            vec![0],
            vec![0x81, 0],
            vec![0x81, 0xff],
            vec![0x81, 0x82, 0],
        ] {
            assert!(Splitter::new(std::io::Cursor::new(bytes), true, false)
                .next_segment()
                .is_err());
        }
    }
}

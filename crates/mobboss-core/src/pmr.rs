//! Bounds-checked port of DJFio's MIT-licensed MDV snippets/pmrtool.cpp (2020).
use anyhow::{bail, ensure, Result};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub file_package_uid: String,
    pub filename: String,
    pub project: String,
    pub material_source_package_uid: String,
    pub timestamp: u32,
}
struct Reader<'a> {
    b: &'a [u8],
    p: usize,
    be: bool,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .p
            .checked_add(n)
            .ok_or_else(|| anyhow::anyhow!("PMR overflow"))?;
        ensure!(end <= self.b.len(), "truncated PMR at {}", self.p);
        let v = &self.b[self.p..end];
        self.p = end;
        Ok(v)
    }
    fn number(&mut self, n: usize) -> Result<u32> {
        let be = self.be;
        let b = self.take(n)?;
        Ok(if be {
            b.iter().fold(0, |a, x| (a << 8) | u32::from(*x))
        } else {
            b.iter().rev().fold(0, |a, x| (a << 8) | u32::from(*x))
        })
    }
    fn string(&mut self) -> Result<String> {
        let n = self.number(2)? as usize;
        Ok(String::from_utf8_lossy(self.take(n)?)
            .trim_end_matches('\0')
            .to_string())
    }
}
pub fn parse(b: &[u8]) -> Result<Vec<Entry>> {
    ensure!(b.len() >= 12, "short PMR");
    let be = if b[..4] == [0, 0, 7, 0xa9] {
        true
    } else if b[..4] == [0xa9, 7, 0, 0] {
        false
    } else {
        bail!("bad PMR magic")
    };
    let mut r = Reader { b, p: 4, be };
    let words = r.number(4)?;
    ensure!(words == 2 || words == 8, "unsupported PMR UID length");
    let count = r.number(4)? as usize;
    ensure!(count <= b.len() / 24, "invalid PMR entry count");
    let mut out = Vec::new();
    for _ in 0..count {
        out.push(Entry {
            file_package_uid: crate::hex(r.take(words as usize * 4)?),
            filename: r.string()?,
            project: r.string()?,
            material_source_package_uid: crate::hex(r.take(words as usize * 4)?),
            timestamp: r.number(4)?,
        });
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn corrupt() {
        for n in 0..40 {
            assert!(parse(&vec![0; n]).is_err());
        }
    }
    #[test]
    fn fixture() {
        let e = parse(include_bytes!("../../../tests/fixtures/msmFMID.pmr")).unwrap();
        assert_eq!(e.len(), 344);
        assert!(e.iter().all(|e| e.project == "BD Demo Reel"));
    }
}

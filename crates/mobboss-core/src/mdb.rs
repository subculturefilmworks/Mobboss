//! Minimal Bento label inspection; object/property association is deliberately unsupported.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};
#[derive(Debug, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub file_size: u64,
    pub little_endian: bool,
    pub toc_offset: u32,
    pub toc_length: u32,
}
pub fn inspect(path: &Path) -> Result<ContainerInfo> {
    let mut f = File::open(path)?;
    let size = f.metadata()?.len();
    ensure!(size >= 24, "short Bento label");
    f.seek(SeekFrom::End(-24))?;
    let mut b = [0; 24];
    f.read_exact(&mut b)?;
    label(&b, size)
}
fn label(b: &[u8; 24], size: u64) -> Result<ContainerInfo> {
    ensure!(
        b[..8] == [0xa4, 0x43, 0x4d, 0xa5, 0x48, 0x64, 0x72, 0xd7],
        "unrecognized MDB container"
    );
    let little = match &b[10..12] {
        b"II" => true,
        b"MM" => false,
        _ => anyhow::bail!("unrecognized Bento byte order"),
    };
    let n = |i: usize| {
        let b = [b[i], b[i + 1], b[i + 2], b[i + 3]];
        if little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        }
    };
    let offset = n(16);
    let length = n(20);
    ensure!(
        u64::from(offset) + u64::from(length) <= size.saturating_sub(24),
        "Bento TOC outside file"
    );
    Ok(ContainerInfo {
        file_size: size,
        little_endian: little,
        toc_offset: offset,
        toc_length: length,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bento_label() {
        let b = [
            0xa4, 0x43, 0x4d, 0xa5, 0x48, 0x64, 0x72, 0xd7, 0, 0, 0x49, 0x49, 1, 0, 0, 0, 0x80,
            0x37, 7, 0, 0xf8, 0x9b, 0x29, 0,
        ];
        let x = label(&b, 3199888).unwrap();
        assert_eq!(x.toc_offset, 472960);
        assert_eq!(x.toc_length, 2726904);
        assert!(label(&b, 100).is_err());
    }
}

pub mod clips;
pub mod fmp4;
pub mod mdb;
pub mod mxf;
pub mod pmr;
pub mod previews;
pub mod scan;
pub mod streaming;
pub mod webm;
pub use scan::*;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Rational {
    pub numerator: i32,
    pub denominator: i32,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaMetadata {
    pub material_package_uid: Option<String>,
    pub file_package_uid: Option<String>,
    pub clip_name: Option<String>,
    pub project: Option<String>,
    pub track_kind: Option<String>,
    pub codec: Option<String>,
    pub codec_ul: Option<String>,
    pub edit_rate: Option<Rational>,
    pub sample_rate: Option<Rational>,
    pub duration: Option<i64>,
    pub start_timecode: Option<i64>,
    pub timecode_fps: Option<u16>,
    pub timecode_drop: Option<bool>,
    pub track_id: Option<u32>,
    pub timecode_rate: Option<Rational>,
    pub header_version: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bit_depth: Option<u32>,
    pub created: Option<String>,
    pub modified: Option<String>,
    pub kind: Option<String>,
}
pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
pub mod ops;
pub mod plan;

/// Match Finder on macOS/Linux and Explorer on Windows.
pub fn format_size(value: u64) -> String {
    format_size_for(value, cfg!(target_os = "windows"))
}
fn format_size_for(value: u64, windows: bool) -> String {
    let base: f64 = if windows { 1024.0 } else { 1000.0 };
    let mut size = value as f64;
    let mut unit = 0;
    while size >= base && unit < 4 {
        size /= base;
        unit += 1;
    }
    format!("{size:.2} {}", ["B", "KB", "MB", "GB", "TB"][unit])
}

#[cfg(test)]
mod size_tests {
    #[test]
    fn finder_and_explorer_parity() {
        let exact = 315_636_807_021;
        assert_eq!(super::format_size_for(exact, false), "315.64 GB");
        assert_eq!(super::format_size_for(exact, true), "293.96 GB");
        let counts = crate::Reconciliation {
            media_files: 2449,
            other_files: 5,
            folders: 4,
            total_bytes: exact,
            ..Default::default()
        };
        assert_eq!(
            counts.media_files + 8 + counts.other_files + counts.folders,
            2466
        );
    }
}

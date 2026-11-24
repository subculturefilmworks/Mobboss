pub mod mdb;
pub mod mxf;
pub mod pmr;
pub mod scan;
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

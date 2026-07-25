//! Polygon zone — WKT 파싱과 point-in-polygon (#48, RFC §6.8).
//!
//! N4(GEOS/GDAL 비링크) 하의 순수 Rust 경로. zone 좌표는 raster native CRS 전제(N2).
//! 경계 위 픽셀 중심 판정은 비계약 (even-odd half-open) — docs/sedona-semantics.md.

use core::fmt;

/// 폴리곤 zone. POLYGON 은 1-원소 MULTIPOLYGON 으로 정규화해 보관한다.
pub struct Zone {
    _priv: (), // 구현 슬라이스에서 MultiPolygon 보관으로 대체
}

/// WKT zone 인자 오류 — SQL 계층이 하드 에러로 승격한다 (unknown stat 과 동급).
#[derive(Debug)]
pub enum ZoneError {
    /// WKT 문법 오류
    Parse(String),
    /// POLYGON/MULTIPOLYGON 외 지오메트리 타입
    Unsupported(String),
    /// 비유한 좌표 (NaN/∞)
    NonFinite,
}

impl fmt::Display for ZoneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ZoneError::Parse(m) => write!(f, "invalid WKT zone: {m}"),
            ZoneError::Unsupported(t) => {
                write!(f, "unsupported zone type {t} (POLYGON/MULTIPOLYGON only)")
            }
            ZoneError::NonFinite => write!(f, "zone coordinates must be finite"),
        }
    }
}

impl std::error::Error for ZoneError {}

/// WKT → Zone. POLYGON/MULTIPOLYGON 만 (EMPTY 허용); 그 외 타입/문법 오류 거부.
pub fn parse_zone_wkt(s: &str) -> Result<Zone, ZoneError> {
    Err(ZoneError::Parse(format!("unimplemented: {s}")))
}

impl Zone {
    /// zone 의 [xmin, ymin, xmax, ymax] envelope — EMPTY → None.
    pub fn envelope(&self) -> Option<[f64; 4]> {
        None
    }

    /// (x, y) 포함 여부 — even-odd ray cast (구멍/멀티폴리곤은 패리티로 처리).
    pub fn contains(&self, _x: f64, _y: f64) -> bool {
        false
    }
}

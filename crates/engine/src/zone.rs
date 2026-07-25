//! Polygon zone — WKT 파싱과 point-in-polygon (#48, RFC §6.8).
//!
//! N4(GEOS/GDAL 비링크) 하의 순수 Rust 경로: wkt → geo-types 파싱만 크레이트에
//! 위임하고 PIP 는 even-odd ray cast 자체 구현 (구멍/멀티폴리곤 = 패리티).
//! zone 좌표는 raster native CRS 전제(N2). 경계 위 픽셀 중심 판정은 비계약
//! (half-open) — docs/sedona-semantics.md.

use core::fmt;

use geo_types::{Geometry, LineString, MultiPolygon};

/// 폴리곤 zone. POLYGON 은 1-원소 MULTIPOLYGON 으로 정규화해 보관한다.
pub struct Zone(MultiPolygon<f64>);

/// WKT zone 인자 오류 — SQL 계층이 하드 에러로 승격한다 (unknown stat 과 동급).
#[derive(Debug)]
pub enum ZoneError {
    /// WKT 문법 오류
    Parse(String),
    /// POLYGON/MULTIPOLYGON 외 지오메트리 타입
    Unsupported(&'static str),
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
    let parsed: wkt::Wkt<f64> = s
        .trim()
        .parse()
        .map_err(|e: &str| ZoneError::Parse(e.to_string()))?;
    let geom = Geometry::try_from(parsed).map_err(|e| ZoneError::Parse(format!("{e:?}")))?;
    let mp = match geom {
        Geometry::Polygon(p) => MultiPolygon(vec![p]),
        Geometry::MultiPolygon(mp) => mp,
        Geometry::Point(_) => return Err(ZoneError::Unsupported("POINT")),
        Geometry::LineString(_) => return Err(ZoneError::Unsupported("LINESTRING")),
        Geometry::MultiPoint(_) => return Err(ZoneError::Unsupported("MULTIPOINT")),
        Geometry::MultiLineString(_) => return Err(ZoneError::Unsupported("MULTILINESTRING")),
        Geometry::GeometryCollection(_) => {
            return Err(ZoneError::Unsupported("GEOMETRYCOLLECTION"))
        }
        _ => return Err(ZoneError::Unsupported("geometry")),
    };
    if rings(&mp)
        .flat_map(|r| r.0.iter())
        .any(|c| !c.x.is_finite() || !c.y.is_finite())
    {
        return Err(ZoneError::NonFinite);
    }
    Ok(Zone(mp))
}

/// 모든 링 (exterior + interiors) 순회.
fn rings(mp: &MultiPolygon<f64>) -> impl Iterator<Item = &LineString<f64>> {
    mp.iter()
        .flat_map(|p| core::iter::once(p.exterior()).chain(p.interiors().iter()))
}

impl Zone {
    /// zone 의 [xmin, ymin, xmax, ymax] envelope — EMPTY → None.
    pub fn envelope(&self) -> Option<[f64; 4]> {
        let mut env: Option<[f64; 4]> = None;
        for c in self.0.iter().flat_map(|p| p.exterior().0.iter()) {
            let e = env.get_or_insert([c.x, c.y, c.x, c.y]);
            e[0] = e[0].min(c.x);
            e[1] = e[1].min(c.y);
            e[2] = e[2].max(c.x);
            e[3] = e[3].max(c.y);
        }
        env
    }

    /// (x, y) 포함 여부 — even-odd ray cast (+x 방향 반직선의 교차 패리티).
    /// 구멍은 짝수 번째 교차로, 멀티폴리곤은 링 합산으로 자연 처리된다.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        let mut inside = false;
        for ring in rings(&self.0) {
            let pts = &ring.0;
            if pts.len() < 2 {
                continue;
            }
            let mut j = pts.len() - 1;
            for i in 0..pts.len() {
                let (pi, pj) = (pts[i], pts[j]);
                if (pi.y > y) != (pj.y > y) {
                    let x_cross = pj.x + (y - pj.y) * (pi.x - pj.x) / (pi.y - pj.y);
                    if x < x_cross {
                        inside = !inside;
                    }
                }
                j = i;
            }
        }
        inside
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notch_and_hole_parity() {
        // U-notch + 구멍: notch 안/구멍 안은 밖, 몸통은 안
        let z = parse_zone_wkt(
            "POLYGON ((0 0, 10 0, 10 10, 7 10, 7 4, 3 4, 3 10, 0 10, 0 0), \
             (8 1, 9 1, 9 2, 8 2, 8 1))",
        )
        .expect("valid");
        assert!(z.contains(1.5, 9.0), "왼팔 안");
        assert!(z.contains(5.0, 2.0), "몸통 안");
        assert!(!z.contains(5.0, 7.0), "notch 안 = 밖");
        assert!(!z.contains(8.5, 1.5), "구멍 안 = 밖");
        assert!(!z.contains(-1.0, 5.0), "envelope 밖");
        assert_eq!(z.envelope(), Some([0.0, 0.0, 10.0, 10.0]));
    }

    #[test]
    fn multipolygon_and_empty() {
        let z = parse_zone_wkt(
            "MULTIPOLYGON (((0 0, 1 0, 1 1, 0 1, 0 0)), ((5 5, 6 5, 6 6, 5 6, 5 5)))",
        )
        .expect("valid");
        assert!(z.contains(0.5, 0.5));
        assert!(z.contains(5.5, 5.5));
        assert!(!z.contains(3.0, 3.0), "두 폴리곤 사이");
        let empty = parse_zone_wkt("POLYGON EMPTY").expect("EMPTY 허용");
        assert_eq!(empty.envelope(), None);
        assert!(!empty.contains(0.0, 0.0));
    }
}

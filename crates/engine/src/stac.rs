//! STAC 문서 → (item, asset) 행 (RFC §6.7 재료). 파싱은 serde_json 위임.
//!
//! graceful degradation: collection/datetime/bbox/media_type 결측 → None,
//! href 없는 asset 은 건너뛴다. id 없는 item 과 모르는 문서 type 은 에러.

use serde_json::Value;

use crate::meta::BandStats;

/// read_stac() 한 행 — (item, asset) 조합.
#[derive(Debug, Clone, PartialEq)]
pub struct StacAssetRow {
    pub item_id: String,
    pub collection: Option<String>,
    pub datetime: Option<String>,
    pub asset_key: String,
    pub href: String,
    pub media_type: Option<String>,
    /// [xmin, ymin, xmax, ymax] — 2D(4원소)·3D(6원소, z 제외) 모두 수용.
    pub bbox: Option<[f64; 4]>,
    /// raster:bands 통계 (§6.7 decode 없는 집계 재료) — 확장 부재 시 None.
    pub band_stats: Option<Vec<BandStats>>,
}

#[derive(Debug)]
pub enum StacError {
    Parse(String),
}

impl std::fmt::Display for StacError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StacError::Parse(msg) => write!(f, "invalid STAC document: {msg}"),
        }
    }
}

impl std::error::Error for StacError {}

/// STAC Item("Feature") 또는 ItemCollection("FeatureCollection") 을 행으로 푼다.
pub fn parse_stac(bytes: &[u8]) -> Result<Vec<StacAssetRow>, StacError> {
    let doc: Value = serde_json::from_slice(bytes).map_err(|e| StacError::Parse(e.to_string()))?;
    match doc.get("type").and_then(Value::as_str) {
        Some("Feature") => item_rows(&doc),
        Some("FeatureCollection") => {
            let feats = doc
                .get("features")
                .and_then(Value::as_array)
                .ok_or_else(|| StacError::Parse("FeatureCollection without features".into()))?;
            let mut out = Vec::new();
            for f in feats {
                out.extend(item_rows(f)?);
            }
            Ok(out)
        }
        other => Err(StacError::Parse(format!(
            "unsupported document type {other:?} (expected Feature or FeatureCollection)"
        ))),
    }
}

fn item_rows(item: &Value) -> Result<Vec<StacAssetRow>, StacError> {
    let item_id = item
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| StacError::Parse("item without id".into()))?
        .to_string();
    let collection = item
        .get("collection")
        .and_then(Value::as_str)
        .map(String::from);
    let datetime = item
        .get("properties")
        .and_then(|p| p.get("datetime"))
        .and_then(Value::as_str)
        .map(String::from);
    let bbox = item.get("bbox").and_then(Value::as_array).and_then(|a| {
        let f = |i: usize| a.get(i).and_then(Value::as_f64);
        match a.len() {
            4 => Some([f(0)?, f(1)?, f(2)?, f(3)?]),
            // 3D bbox: [xmin, ymin, zmin, xmax, ymax, zmax]
            6 => Some([f(0)?, f(1)?, f(3)?, f(4)?]),
            _ => None,
        }
    });
    let mut out = Vec::new();
    if let Some(assets) = item.get("assets").and_then(Value::as_object) {
        for (key, asset) in assets {
            // href 없는 asset 은 행이 될 수 없다 — graceful skip
            let Some(href) = asset.get("href").and_then(Value::as_str) else {
                continue;
            };
            let band_stats = asset
                .get("raster:bands")
                .and_then(Value::as_array)
                .map(|bands| {
                    bands
                        .iter()
                        .map(|b| {
                            let st = b.get("statistics");
                            let g = |k: &str| st.and_then(|s| s.get(k)).and_then(Value::as_f64);
                            BandStats {
                                min: g("minimum"),
                                max: g("maximum"),
                                mean: g("mean"),
                                stddev: g("stddev"),
                            }
                        })
                        .collect()
                });
            out.push(StacAssetRow {
                item_id: item_id.clone(),
                collection: collection.clone(),
                datetime: datetime.clone(),
                asset_key: key.clone(),
                href: href.to_string(),
                media_type: asset.get("type").and_then(Value::as_str).map(String::from),
                bbox,
                band_stats,
            });
        }
    }
    Ok(out)
}

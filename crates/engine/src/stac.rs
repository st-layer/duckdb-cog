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

/// STAC API 검색 응답의 rel=next 링크 (#29). method 기본 GET,
/// POST next 는 body(+merge — 원 요청 body 위에 얹기)를 나를 수 있다.
#[derive(Debug, Clone, PartialEq)]
pub struct StacNext {
    pub href: String,
    pub method: String,
    pub body: Option<Value>,
    pub merge: bool,
}

/// STAC API 검색 응답 한 페이지: (item, asset) 행들 + 다음 페이지 링크.
#[derive(Debug)]
pub struct StacPage {
    pub rows: Vec<StacAssetRow>,
    pub next: Option<StacNext>,
}

/// 검색 응답(FeatureCollection) 한 페이지를 푼다 — 단일 Feature 는 검색
/// 응답이 아니므로 에러 (정적 문서는 [`parse_stac`]).
pub fn parse_stac_page(bytes: &[u8]) -> Result<StacPage, StacError> {
    let doc: Value = serde_json::from_slice(bytes).map_err(|e| StacError::Parse(e.to_string()))?;
    if doc.get("type").and_then(Value::as_str) != Some("FeatureCollection") {
        return Err(StacError::Parse(
            "search response must be a FeatureCollection".into(),
        ));
    }
    let feats = doc
        .get("features")
        .and_then(Value::as_array)
        .ok_or_else(|| StacError::Parse("FeatureCollection without features".into()))?;
    let mut rows = Vec::new();
    for f in feats {
        rows.extend(item_rows(f)?);
    }
    let next = doc
        .get("links")
        .and_then(Value::as_array)
        .and_then(|links| {
            links
                .iter()
                .find(|l| l.get("rel").and_then(Value::as_str) == Some("next"))
        })
        .and_then(|l| {
            Some(StacNext {
                href: l.get("href").and_then(Value::as_str)?.to_string(),
                method: l
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or("GET")
                    .to_string(),
                body: l.get("body").cloned(),
                merge: l.get("merge").and_then(Value::as_bool).unwrap_or(false),
            })
        });
    Ok(StacPage { rows, next })
}

/// POST /search body 조립 — 설정된 필드만 키로 넣는다 (STAC API Item Search).
pub fn build_search_body(
    collections: Option<&[String]>,
    bbox: Option<[f64; 4]>,
    datetime: Option<&str>,
    limit: Option<u32>,
) -> Value {
    let mut body = serde_json::Map::new();
    if let Some(c) = collections {
        body.insert("collections".into(), serde_json::json!(c));
    }
    if let Some(b) = bbox {
        body.insert("bbox".into(), serde_json::json!(b));
    }
    if let Some(d) = datetime {
        body.insert("datetime".into(), serde_json::json!(d));
    }
    if let Some(l) = limit {
        body.insert("limit".into(), serde_json::json!(l));
    }
    Value::Object(body)
}

/// next 링크를 다음 요청 (href, method, body) 로 푼다.
/// merge=true 면 원 body 위에 next body 를 얹는다 (겹치는 키는 next 승리).
pub fn apply_next(original: &Value, next: &StacNext) -> (String, String, Option<Value>) {
    let body = match (&next.body, next.merge) {
        (Some(nb), true) => {
            let mut merged = original.as_object().cloned().unwrap_or_default();
            if let Some(nb) = nb.as_object() {
                for (k, v) in nb {
                    merged.insert(k.clone(), v.clone());
                }
            }
            Some(Value::Object(merged))
        }
        (Some(nb), false) => Some(nb.clone()),
        (None, _) => None,
    };
    (next.href.clone(), next.method.clone(), body)
}

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

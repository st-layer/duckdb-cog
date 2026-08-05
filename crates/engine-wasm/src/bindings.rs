//! JS 경계 — bytes 입력(메모리 경로) 바인딩. fetch/URL 경로는 다음 슬라이스.
//!
//! 반환 규약은 engine 이 이미 보장하는 G11 을 그대로 통과시킨다:
//! 1-based 밴드, 범위 밖 밴드/빈 교차 → count 0·나머지 null. 입력 파싱
//! 실패(WKT/stat)와 IO/디코드 실패는 Promise reject(Error).

use engine::{open_cog, parse_zone_wkt, MemorySource, ZonalStat};
use js_sys::{Array, Object, Promise, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

/// 패닉 메시지를 브라우저 console 로 — 모듈 로드 시 1회.
#[wasm_bindgen(start)]
fn start() {
    console_error_panic_hook::set_once();
}

fn err(e: impl std::fmt::Display) -> JsValue {
    js_sys::Error::new(&e.to_string()).into()
}

/// Reflect::set — 수신자가 방금 만든 Object 라 실패하지 않는다.
fn set(obj: &Object, key: &str, val: JsValue) {
    let _ = Reflect::set(obj, &JsValue::from_str(key), &val);
}

fn opt_f64(v: Option<f64>) -> JsValue {
    v.map(JsValue::from_f64).unwrap_or(JsValue::NULL)
}

/// `cogMetaFromBytes(bytes)` → Promise<{width, height, numBands, srid,
/// geotransform, nodata, levels, bandStats}>.
///
/// geotransform 은 GDAL 순서 (scaleX, skewY, skewX, scaleY, upperLeftX,
/// upperLeftY — RFC §6.8, `georeference_gdal` 과 동일), georef 부재 시 null.
/// width/height 도 레벨 부재 시 null (graceful degradation, §6.7).
#[wasm_bindgen(js_name = cogMetaFromBytes)]
pub fn cog_meta_from_bytes(bytes: Uint8Array) -> Promise {
    let data = bytes.to_vec();
    future_to_promise(async move {
        let meta = engine::read_cog_meta(MemorySource::new(data))
            .await
            .map_err(err)?;

        let o = Object::new();
        set(&o, "width", opt_f64(meta.width().map(f64::from)));
        set(&o, "height", opt_f64(meta.height().map(f64::from)));
        set(&o, "numBands", f64::from(meta.num_bands).into());
        set(&o, "srid", f64::from(meta.srid()).into());
        set(&o, "nodata", opt_f64(meta.nodata));

        match &meta.georef {
            Some(g) => {
                let (sx, sy) = g.scale_gdal();
                let (kx, ky) = g.skew();
                let gt = [sx, ky, kx, sy, g.origin_x, g.origin_y]
                    .iter()
                    .map(|v| JsValue::from_f64(*v))
                    .collect::<Array>();
                set(&o, "geotransform", gt.into());
            }
            None => set(&o, "geotransform", JsValue::NULL),
        }

        let levels = Array::new();
        for l in &meta.levels {
            let lo = Object::new();
            set(&lo, "width", f64::from(l.image_width).into());
            set(&lo, "height", f64::from(l.image_height).into());
            set(&lo, "tileWidth", f64::from(l.tile_width).into());
            set(&lo, "tileHeight", f64::from(l.tile_height).into());
            levels.push(&lo);
        }
        set(&o, "levels", levels.into());

        match &meta.band_stats {
            Some(bands) => {
                let arr = Array::new();
                for b in bands {
                    let bo = Object::new();
                    set(&bo, "min", opt_f64(b.min));
                    set(&bo, "max", opt_f64(b.max));
                    set(&bo, "mean", opt_f64(b.mean));
                    set(&bo, "stddev", opt_f64(b.stddev));
                    arr.push(&bo);
                }
                set(&o, "bandStats", arr.into());
            }
            None => set(&o, "bandStats", JsValue::NULL),
        }

        Ok(o.into())
    })
}

/// `zonalStatsFromBytes(bytes, zoneWkt, band, stat)` → Promise<number|null>.
///
/// stat ∈ {count, sum, mean, min, max} (대소문자 무관) — 매핑은
/// `engine::ZonalStat`/`ZonalStats::value` 가 단일 소스 (네이티브와 동일).
#[wasm_bindgen(js_name = zonalStatsFromBytes)]
pub fn zonal_stats_from_bytes(
    bytes: Uint8Array,
    zone_wkt: String,
    band: u32,
    stat: String,
) -> Promise {
    let data = bytes.to_vec();
    future_to_promise(async move {
        let stat: ZonalStat = stat.parse().map_err(err)?;
        let zone = parse_zone_wkt(&zone_wkt).map_err(err)?;
        let (meta, reader) = open_cog(MemorySource::new(data)).await.map_err(err)?;
        let z = reader
            .zonal_stats_polygon(&meta, &zone, band)
            .await
            .map_err(err)?;
        Ok(opt_f64(z.value(stat)))
    })
}

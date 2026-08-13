//! JS 경계 — bytes 입력(메모리 경로)과 URL 입력(fetch 경로) 바인딩.
//!
//! 반환 규약은 engine 이 이미 보장하는 G11 을 그대로 통과시킨다:
//! 1-based 밴드, 범위 밖 밴드/빈 교차 → count 0·나머지 null. 입력 파싱
//! 실패(WKT/stat)와 IO/디코드 실패는 Promise reject(Error).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use engine::futures::future::join_all;
use engine::{open_cog, parse_zone_wkt, ByteSource, CogMeta, MemorySource, ZonalStat, Zone};
use js_sys::{Array, Object, Promise, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

use crate::registry;

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

/// CogMeta → JS 객체 {width, height, numBands, srid, geotransform, nodata,
/// levels, bandStats} — bytes/URL 두 경로 공용.
///
/// geotransform 은 GDAL 순서 (scaleX, skewY, skewX, scaleY, upperLeftX,
/// upperLeftY — RFC §6.8, `georeference_gdal` 과 동일), georef 부재 시 null.
/// width/height 도 레벨 부재 시 null (graceful degradation, §6.7).
fn meta_to_js(meta: &CogMeta) -> JsValue {
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

    o.into()
}

/// zone WKT + stat 이름 파싱 — 모든 zonal 진입점 공용 (실패 → reject).
fn parse_inputs(zone_wkt: &str, stat: &str) -> Result<(Zone, ZonalStat), JsValue> {
    let stat: ZonalStat = stat.parse().map_err(err)?;
    let zone = parse_zone_wkt(zone_wkt).map_err(err)?;
    Ok((zone, stat))
}

/// zonal 공용 코어 (bytes 경로) — 규약(G11)·에러 매핑은 URL 경로와 동일.
async fn zonal_value<S: ByteSource>(
    source: S,
    zone_wkt: &str,
    band: u32,
    stat: &str,
) -> Result<JsValue, JsValue> {
    let (zone, stat) = parse_inputs(zone_wkt, stat)?;
    let (meta, reader) = open_cog(source).await.map_err(err)?;
    let z = reader
        .zonal_stats_polygon(&meta, &zone, band)
        .await
        .map_err(err)?;
    Ok(opt_f64(z.value(stat)))
}

/// 열린 리더 위 zonal 하나 (URL 스칼라·배치 공용). `max_pixels`(#71) 예산이
/// 있으면 오버뷰 레벨 근사 — 네이티브와 같은 select_level/value_scaled 경로
/// (count/sum 은 level-0 환산 추정, 0 = exact).
async fn zonal_on(
    opened: &registry::Opened,
    zone: &Zone,
    band: u32,
    stat: ZonalStat,
    max_pixels: u64,
) -> Result<JsValue, JsValue> {
    let level = zone
        .envelope()
        .map_or(0, |e| engine::select_level(&opened.0, e, max_pixels));
    let z = opened
        .1
        .zonal_stats_polygon_at(&opened.0, zone, band, level)
        .await
        .map_err(err)?;
    Ok(opt_f64(
        z.value_scaled(stat, engine::level_scale(&opened.0, level)),
    ))
}

/// `cogMetaFromBytes(bytes)` → Promise<meta 객체> ([`meta_to_js`] 참조).
#[wasm_bindgen(js_name = cogMetaFromBytes)]
pub fn cog_meta_from_bytes(bytes: Uint8Array) -> Promise {
    let data = bytes.to_vec();
    future_to_promise(async move {
        let meta = engine::read_cog_meta(MemorySource::new(data))
            .await
            .map_err(err)?;
        Ok(meta_to_js(&meta))
    })
}

/// `cogMeta(url)` → Promise<meta 객체> — 원격 COG (fetch/HTTP Range,
/// 리더 레지스트리 경유 — 같은 URL 재호출은 웜).
#[wasm_bindgen(js_name = cogMeta)]
pub fn cog_meta(url: String) -> Promise {
    future_to_promise(async move {
        let opened = registry::open_cached(&url).await.map_err(err)?;
        Ok(meta_to_js(&opened.0))
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
    future_to_promise(
        async move { zonal_value(MemorySource::new(data), &zone_wkt, band, &stat).await },
    )
}

/// `zonalStats(url, zoneWkt, band, stat)` → Promise<number|null> — 원격 COG.
/// 규약은 [`zonal_stats_from_bytes`] 와 동일 (전송 계층만 다르다). 리더는
/// 레지스트리로 재사용 — 같은 씬 반복 호출(폴리곤 수정)이 타일 캐시 웜 경로.
/// `maxPixels`(#71, 선택): 픽셀 예산으로 오버뷰 fast mode — 생략/0 = exact.
#[wasm_bindgen(js_name = zonalStats)]
pub fn zonal_stats(
    url: String,
    zone_wkt: String,
    band: u32,
    stat: String,
    max_pixels: Option<u32>,
) -> Promise {
    future_to_promise(async move {
        let (zone, stat) = parse_inputs(&zone_wkt, &stat)?;
        let opened = registry::open_cached(&url).await.map_err(err)?;
        zonal_on(
            &opened,
            &zone,
            band,
            stat,
            u64::from(max_pixels.unwrap_or(0)),
        )
        .await
    })
}

/// `zonalStatsBatch(urls, zoneWkt, band, stat)` → Promise<Array<number|null>>
/// — 시계열 배치 (1 zone × N scenes). 씬별 리더는 레지스트리로 상각·재사용,
/// 유일 URL 만 open 하고 zonal 은 동시 실행, 결과는 입력 순서.
///
/// **IO 실패는 전체 reject** (해당 URL 포함) — null 은 G11 의미(빈 교차/범위
/// 밖 밴드/nodata)로만 쓴다. 무음 데이터 손실 금지.
#[wasm_bindgen(js_name = zonalStatsBatch)]
pub fn zonal_stats_batch(
    urls: Array,
    zone_wkt: String,
    band: u32,
    stat: String,
    max_pixels: Option<u32>,
) -> Promise {
    future_to_promise(async move {
        let mp = u64::from(max_pixels.unwrap_or(0));
        let (zone, stat) = parse_inputs(&zone_wkt, &stat)?;
        let urls: Vec<String> = urls
            .iter()
            .map(|u| u.as_string().ok_or_else(|| err("urls must be strings")))
            .collect::<Result<_, _>>()?;

        // 유일 URL 만 open (같은 URL 동시 미스의 중복 open 방지) — 네트워크는
        // join_all 로 자연 병렬
        let mut uniq: Vec<&str> = Vec::new();
        let mut seen = HashSet::new();
        for u in &urls {
            if seen.insert(u.as_str()) {
                uniq.push(u);
            }
        }
        let mut by_url: HashMap<&str, Rc<registry::Opened>> = HashMap::new();
        for (u, res) in uniq
            .iter()
            .zip(join_all(uniq.iter().map(|u| registry::open_cached(u))).await)
        {
            by_url.insert(u, res.map_err(err)?);
        }

        let results = join_all(urls.iter().map(|u| {
            let opened = Rc::clone(&by_url[u.as_str()]);
            let zone = &zone;
            async move { zonal_on(&opened, zone, band, stat, mp).await }
        }))
        .await;
        let out = Array::new();
        for r in results {
            out.push(&r?);
        }
        Ok(out.into())
    })
}

/// `zonalStatsBatchZones(url, zoneWkts, band, stat)` → Promise<Array<number|null>>
/// — zone축 배치 (N zones × 1 scene): 네이티브 #62 와 같은 타일 union 1회
/// fetch 경로 (`zonal_stats_polygon_batch`). WKT 파싱 실패 → reject (네이티브
/// LIST 오버로드와 동일), 결과는 입력 순서.
#[wasm_bindgen(js_name = zonalStatsBatchZones)]
pub fn zonal_stats_batch_zones(
    url: String,
    zone_wkts: Array,
    band: u32,
    stat: String,
    max_pixels: Option<u32>,
) -> Promise {
    future_to_promise(async move {
        let stat: ZonalStat = stat.parse().map_err(err)?;
        let zones: Vec<Zone> = zone_wkts
            .iter()
            .map(|w| {
                w.as_string()
                    .ok_or_else(|| err("zoneWkts must be strings"))
                    .and_then(|s| parse_zone_wkt(&s).map_err(err))
            })
            .collect::<Result<_, _>>()?;
        let opened = registry::open_cached(&url).await.map_err(err)?;
        // #71: zone union envelope 기준 단일 레벨 (네이티브 배치와 동일 규약)
        let env = zones.iter().filter_map(|z| z.envelope()).reduce(|a, b| {
            [
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[2].max(b[2]),
                a[3].max(b[3]),
            ]
        });
        let level = env.map_or(0, |e| {
            engine::select_level(&opened.0, e, u64::from(max_pixels.unwrap_or(0)))
        });
        let scale = engine::level_scale(&opened.0, level);
        let zs = opened
            .1
            .zonal_stats_polygon_batch_at(&opened.0, &zones, band, level)
            .await
            .map_err(err)?;
        let out = Array::new();
        for z in &zs {
            out.push(&opt_f64(z.value_scaled(stat, scale)));
        }
        Ok(out.into())
    })
}

/// band_window* 결과 → JS 객체 (#76). `None`(범위 밖 밴드) → null,
/// `Some(빈)`(비교차/빈 폴리곤) → {values: 빈, width: 0, height: 0, bbox: null},
/// 그 외 → {values: Float64Array(NaN = 값 부재), width, height,
/// bbox: [minx, miny, maxx, maxy] (래스터 CRS, 배치용)}.
///
/// 창 좌표는 `engine::envelope_window`(픽셀 중심 규약의 단일 소스)로 얻고,
/// bbox 는 확정된 정수 rect 의 선형 변환 — 반올림 규약을 재계산하지 않는다.
fn window_result(
    meta: &CogMeta,
    env: Option<[f64; 4]>,
    win: Option<Vec<Option<f64>>>,
) -> Result<JsValue, JsValue> {
    let Some(win) = win else {
        return Ok(JsValue::NULL); // 범위 밖 밴드 — 네이티브 NULL 행과 동형
    };
    let o = Object::new();
    let values = js_sys::Float64Array::new_with_length(win.len() as u32);
    for (i, v) in win.iter().enumerate() {
        values.set_index(i as u32, v.unwrap_or(f64::NAN));
    }
    let rect = env
        .and_then(|e| engine::envelope_window(meta, e))
        .filter(|_| !win.is_empty());
    match (rect, &meta.georef) {
        (Some((c0, c1, r0, r1)), Some(g)) => {
            set(&o, "width", ((c1 - c0 + 1) as f64).into());
            set(&o, "height", ((r1 - r0 + 1) as f64).into());
            let bbox = [
                g.origin_x + c0 as f64 * g.pixel_x,
                g.origin_y - (r1 + 1) as f64 * g.pixel_y,
                g.origin_x + (c1 + 1) as f64 * g.pixel_x,
                g.origin_y - r0 as f64 * g.pixel_y,
            ]
            .iter()
            .map(|v| JsValue::from_f64(*v))
            .collect::<Array>();
            set(&o, "bbox", bbox.into());
        }
        _ => {
            set(&o, "width", 0f64.into());
            set(&o, "height", 0f64.into());
            set(&o, "bbox", JsValue::NULL);
        }
    }
    set(&o, "values", values.into());
    Ok(o.into())
}

/// `bandWindowPolygon(url, zoneWkt, band)` → Promise — AOI 픽셀 창 (#76).
/// 픽셀 중심 규약을 zonal 과 공유하므로 **렌더된 래스터 = 통계가 본 픽셀
/// 집합**. NaN 은 nodata·폴리곤 밖 공통의 "값 부재" (구분 없음, 엔진 계약).
#[wasm_bindgen(js_name = bandWindowPolygon)]
pub fn band_window_polygon(url: String, zone_wkt: String, band: u32) -> Promise {
    future_to_promise(async move {
        let zone = parse_zone_wkt(&zone_wkt).map_err(err)?;
        let opened = registry::open_cached(&url).await.map_err(err)?;
        let win = opened
            .1
            .band_window_polygon(&opened.0, &zone, band)
            .await
            .map_err(err)?;
        window_result(&opened.0, zone.envelope(), win)
    })
}

/// `bandWindow(url, bbox, band)` → Promise — bbox([minx, miny, maxx, maxy])
/// 창 (#76). bbox 필수: 전체 씬 읽기는 브라우저 footgun — 필요하면 cogMeta
/// 의 geotransform 으로 씬 bbox 를 구성해 넘긴다.
#[wasm_bindgen(js_name = bandWindow)]
pub fn band_window(url: String, bbox: Array, band: u32) -> Promise {
    future_to_promise(async move {
        let b: Vec<f64> = bbox
            .iter()
            .map(|v| v.as_f64().ok_or_else(|| err("bbox must be 4 numbers")))
            .collect::<Result<_, _>>()?;
        let b: [f64; 4] = b
            .try_into()
            .map_err(|_| err("bbox must be [minx, miny, maxx, maxy]"))?;
        let opened = registry::open_cached(&url).await.map_err(err)?;
        let win = opened
            .1
            .band_window(&opened.0, Some(b), band)
            .await
            .map_err(err)?;
        window_result(&opened.0, Some(b), win)
    })
}

/// `configureTileCache(mb)` — 타일 캐시 상한 재설정 (0 = 비활성, 기본 64MB).
/// 캐시 교체와 함께 리더 레지스트리도 비운다 (콜드 리셋).
#[wasm_bindgen(js_name = configureTileCache)]
pub fn configure_tile_cache(mb: u32) {
    registry::configure_tile_cache(mb);
}

/// `tileCacheStats()` → {hits, misses, evictions, bytes, maxBytes} —
/// 네이티브 `cog_cache_stats()`(#63) 의 브라우저 대응.
#[wasm_bindgen(js_name = tileCacheStats)]
pub fn tile_cache_stats() -> JsValue {
    let s = registry::tile_cache_stats();
    let o = Object::new();
    set(&o, "hits", (s.hits as f64).into());
    set(&o, "misses", (s.misses as f64).into());
    set(&o, "evictions", (s.evictions as f64).into());
    set(&o, "bytes", (s.bytes as f64).into());
    set(&o, "maxBytes", (s.max_bytes as f64).into());
    o.into()
}

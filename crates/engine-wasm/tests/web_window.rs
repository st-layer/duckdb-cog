//! AOI 픽셀 창 바인딩 판정 (#76): 렌더링용 래스터가 **zonal 통계와 정확히
//! 같은 픽셀 집합**인지 — engine 의 픽셀 중심 규약 공유 계약을 브라우저에서
//! 검증한다. 골든은 전부 기존 상수(zonal_batch.rs) + 창 기하 손계산.
#![cfg(target_arch = "wasm32")]

use engine_wasm::{band_window, band_window_polygon};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

/// justfile wasm-test 레시피와의 포트 계약.
const BASE: &str = "http://127.0.0.1:18925";

fn url(name: &str) -> String {
    format!("{BASE}/{name}")
}

/// zonal_batch.rs 오라클 P3 삼각형 (count 7267 / sum 239110784).
const P3: &str = "POLYGON ((300203.7 3999803.7, 301403.7 3999803.7, 300203.7 3998593.1, \
     300203.7 3999803.7))";

fn get(o: &JsValue, k: &str) -> JsValue {
    js_sys::Reflect::get(o, &JsValue::from_str(k)).unwrap()
}

fn values_of(o: &JsValue) -> Vec<f64> {
    get(o, "values")
        .dyn_into::<js_sys::Float64Array>()
        .expect("values 는 Float64Array")
        .to_vec()
}

#[wasm_bindgen_test]
async fn polygon_window_is_the_zonal_pixel_set() {
    let w = JsFuture::from(band_window_polygon(
        url("basic_512x512_u16.tif"),
        P3.to_string(),
        1,
    ))
    .await
    .expect("resolve");

    // 창 기하 손계산 (origin (300000, 4000000), 10m): cols 20..=139, rows 20..=140
    assert_eq!(get(&w, "width").as_f64(), Some(120.0));
    assert_eq!(get(&w, "height").as_f64(), Some(121.0));
    let bbox: Vec<f64> = js_sys::Array::from(&get(&w, "bbox"))
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    assert_eq!(bbox, [300_200.0, 3_998_590.0, 301_400.0, 3_999_800.0]);

    // "차트의 통계와 화면의 래스터는 같은 픽셀 집합" — zonal 골든과 직접 대조
    let v = values_of(&w);
    assert_eq!(v.len(), 120 * 121);
    let finite: Vec<f64> = v.iter().copied().filter(|x| !x.is_nan()).collect();
    assert_eq!(finite.len(), 7_267, "비-NaN 개수 = zonal count");
    assert_eq!(finite.iter().sum::<f64>(), 239_110_784.0, "합 = zonal sum");
    assert_eq!(v.len() - finite.len(), 7_253, "삼각형 마스킹 (~50%)");
}

#[wasm_bindgen_test]
async fn bbox_window_matches_rectangle_polygon() {
    // 같은 영역: bbox vs 그 직사각 WKT — 픽셀 중심이 경계에 안 걸리는 좌표로
    let bbox = js_sys::Array::of4(
        &300_503.7.into(),
        &3_999_003.7.into(),
        &300_903.7.into(),
        &3_999_403.7.into(),
    );
    let rect = "POLYGON ((300503.7 3999003.7, 300903.7 3999003.7, 300903.7 3999403.7, \
         300503.7 3999403.7, 300503.7 3999003.7))";

    let a = JsFuture::from(band_window(url("basic_512x512_u16.tif"), bbox, 1))
        .await
        .expect("resolve");
    let b = JsFuture::from(band_window_polygon(
        url("basic_512x512_u16.tif"),
        rect.to_string(),
        1,
    ))
    .await
    .expect("resolve");

    assert_eq!(get(&a, "width").as_f64(), Some(40.0));
    assert_eq!(get(&a, "height").as_f64(), Some(40.0));
    let (va, vb) = (values_of(&a), values_of(&b));
    assert_eq!(va.len(), 1600);
    assert_eq!(va, vb, "직사각 폴리곤 = bbox (마스크 없음, 전 원소 동일)");
    assert!(va.iter().all(|x| !x.is_nan()), "내부 완전 포함 — NaN 없음");
}

#[wasm_bindgen_test]
async fn out_of_range_band_resolves_null() {
    let w = JsFuture::from(band_window_polygon(
        url("basic_512x512_u16.tif"),
        P3.to_string(),
        99,
    ))
    .await
    .expect("resolve");
    assert!(w.is_null(), "범위 밖 밴드 → null (네이티브 NULL 행과 동형)");
}

#[wasm_bindgen_test]
async fn non_intersecting_polygon_resolves_empty_window() {
    // P3 는 stats_64x64 씬(origin 950000/4000000, 640m 폭)과 비교차
    let w = JsFuture::from(band_window_polygon(
        url("stats_64x64_u16.tif"),
        P3.to_string(),
        1,
    ))
    .await
    .expect("resolve");
    assert_eq!(get(&w, "width").as_f64(), Some(0.0));
    assert_eq!(get(&w, "height").as_f64(), Some(0.0));
    assert!(get(&w, "bbox").is_null());
    assert_eq!(values_of(&w).len(), 0);
}

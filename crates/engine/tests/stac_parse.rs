//! read_stac 의 엔진 재료 계약: STAC JSON 파싱 + 전체 문서 fetch.

use engine::{fetch_all, parse_stac, MemorySource};

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    engine::futures::executor::block_on(f)
}

const ITEM: &str = r#"{
  "type": "Feature", "id": "solo", "collection": "c1",
  "bbox": [1.0, 2.0, 3.0, 4.0],
  "properties": { "datetime": "2026-01-01T00:00:00Z" },
  "assets": { "cog": { "href": "a.tif", "type": "image/tiff" } }
}"#;

#[test]
fn parses_single_item() {
    let rows = parse_stac(ITEM.as_bytes()).expect("valid STAC");
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.item_id, "solo");
    assert_eq!(r.collection.as_deref(), Some("c1"));
    assert_eq!(r.datetime.as_deref(), Some("2026-01-01T00:00:00Z"));
    assert_eq!((r.asset_key.as_str(), r.href.as_str()), ("cog", "a.tif"));
    assert_eq!(r.media_type.as_deref(), Some("image/tiff"));
    assert_eq!(r.bbox, Some([1.0, 2.0, 3.0, 4.0]));
}

#[test]
fn parses_item_collection_with_graceful_degradation() {
    let doc = format!(
        r#"{{"type": "FeatureCollection", "features": [{ITEM},
            {{"type": "Feature", "id": "bare", "properties": {{}},
              "assets": {{"data": {{"href": "b.tif"}}}}}}]}}"#
    );
    let rows = parse_stac(doc.as_bytes()).expect("valid STAC");
    assert_eq!(rows.len(), 2);
    let bare = rows.iter().find(|r| r.item_id == "bare").expect("bare");
    assert_eq!(bare.collection, None);
    assert_eq!(bare.datetime, None);
    assert_eq!(bare.bbox, None);
    assert_eq!(bare.media_type, None);
}

#[test]
fn rejects_invalid_documents() {
    assert!(parse_stac(b"{ not json").is_err());
    assert!(
        parse_stac(br#"{"type": "Unknown"}"#).is_err(),
        "모르는 type"
    );
    // asset 에 href 가 없으면 그 asset 은 건너뛴다 (graceful) — item 자체는 유효
    let rows = parse_stac(
        br#"{"type": "Feature", "id": "x", "properties": {},
             "assets": {"broken": {}, "ok": {"href": "h.tif"}}}"#,
    )
    .expect("valid");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].asset_key, "ok");
}

#[test]
fn fetch_all_reads_whole_document_progressively() {
    // 초기 청크(64KiB)보다 큰 문서 — 점진 배증으로 전체 획득 (EOF 클램프 계약 활용)
    let big = vec![b'x'; 1_500_000];
    let got = block_on(fetch_all(&MemorySource::new(big.clone()))).expect("io ok");
    assert_eq!(got.len(), big.len());
    let small = vec![b'y'; 10];
    let got = block_on(fetch_all(&MemorySource::new(small.clone()))).expect("io ok");
    assert_eq!(got.as_ref(), small.as_slice());
}

#[test]
fn three_dimensional_bbox_drops_z() {
    // 6원소 bbox = [xmin, ymin, zmin, xmax, ymax, zmax] → z 제외 인덱스 (0,1,3,4)
    let rows = parse_stac(
        br#"{"type": "Feature", "id": "z", "bbox": [10.0, 20.0, -5.0, 30.0, 40.0, 99.0],
             "properties": {}, "assets": {"a": {"href": "h.tif"}}}"#,
    )
    .expect("valid");
    assert_eq!(rows[0].bbox, Some([10.0, 20.0, 30.0, 40.0]));
    // 그 외 길이(5 등)는 graceful None
    let rows = parse_stac(
        br#"{"type": "Feature", "id": "w", "bbox": [1.0, 2.0, 3.0, 4.0, 5.0],
             "properties": {}, "assets": {"a": {"href": "h.tif"}}}"#,
    )
    .expect("valid");
    assert_eq!(rows[0].bbox, None);
}

/// raster:bands 통계 매핑 (§6.7 "decode 없는 집계" 재료).
#[test]
fn parses_raster_bands_statistics() {
    let doc = std::fs::read("../../test/data/stac/with_stats.json").expect("fixture");
    let rows = parse_stac(&doc).expect("valid STAC");
    let get = |k: &str| rows.iter().find(|r| r.asset_key == k).expect(k);

    let red = get("red");
    let bands = red.band_stats.as_ref().expect("raster:bands 있음");
    assert_eq!(bands.len(), 1);
    assert_eq!(
        (bands[0].min, bands[0].max, bands[0].mean, bands[0].stddev),
        (Some(1.0), Some(65535.0), Some(32768.5), Some(18918.9))
    );

    let rgb = get("rgb");
    let bands = rgb.band_stats.as_ref().expect("raster:bands 있음");
    assert_eq!(bands.len(), 3);
    assert_eq!(bands[0].stddev, None, "부분 결측 graceful");
    assert_eq!(bands[1].stddev, Some(73.2));
    assert_eq!(
        (bands[2].min, bands[2].mean),
        (None, None),
        "빈 밴드 객체도 자리 보존"
    );

    // 확장 부재 → None (빈 리스트와 구분)
    assert!(get("nostats").band_stats.is_none());
}

// ---- STAC API 검색 재료 (#29): 페이지 파싱 + 검색 body 조립 + next 팔로우 ----

use engine::{apply_next, build_search_body, parse_stac_page};

/// FeatureCollection + rel=next(POST, body, merge) → rows 와 next 를 함께 푼다.
#[test]
fn page_with_post_next_link() {
    let doc = format!(
        r#"{{"type": "FeatureCollection", "features": [{ITEM}],
            "links": [
              {{"rel": "self", "href": "https://api/search"}},
              {{"rel": "next", "href": "https://api/search",
                "method": "POST", "merge": true,
                "body": {{"token": "next:abc"}}}}
            ]}}"#
    );
    let page = parse_stac_page(doc.as_bytes()).expect("valid page");
    assert_eq!(page.rows.len(), 1);
    let next = page.next.expect("next link");
    assert_eq!(next.href, "https://api/search");
    assert_eq!(next.method, "POST");
    assert!(next.merge);
    assert_eq!(
        next.body.as_ref().and_then(|b| b.get("token")).and_then(|v| v.as_str()),
        Some("next:abc")
    );
}

/// method 생략 시 GET 이 기본 (STAC API 링크 규약), body/merge 없음.
#[test]
fn page_with_get_next_link_defaults() {
    let doc = format!(
        r#"{{"type": "FeatureCollection", "features": [{ITEM}],
            "links": [{{"rel": "next", "href": "https://api/search?page=2"}}]}}"#
    );
    let next = parse_stac_page(doc.as_bytes()).expect("valid").next.expect("next");
    assert_eq!(next.method, "GET");
    assert_eq!(next.body, None);
    assert!(!next.merge);
}

/// next 없는 마지막 페이지·links 자체가 없는 페이지 → next = None.
#[test]
fn page_without_next() {
    let with_links = format!(
        r#"{{"type": "FeatureCollection", "features": [{ITEM}],
            "links": [{{"rel": "self", "href": "https://api/search"}}]}}"#
    );
    assert!(parse_stac_page(with_links.as_bytes()).expect("valid").next.is_none());
    let no_links = format!(r#"{{"type": "FeatureCollection", "features": [{ITEM}]}}"#);
    assert!(parse_stac_page(no_links.as_bytes()).expect("valid").next.is_none());
}

/// 단일 Feature 는 페이지가 아니다 — 에러 (read_stac 과 달리 검색 응답 계약).
#[test]
fn page_rejects_bare_item() {
    assert!(parse_stac_page(ITEM.as_bytes()).is_err());
}

/// 설정된 필드만 body 에 들어간다 — None 필드는 키 자체가 없어야 한다.
#[test]
fn search_body_includes_only_set_fields() {
    let body = build_search_body(None, None, None, None);
    assert_eq!(body, engine::serde_json::json!({}));

    let cols = vec!["sentinel-2-l2a".to_string()];
    let body = build_search_body(
        Some(&cols),
        Some([126.0, 37.0, 127.0, 38.0]),
        Some("2026-07-01/2026-07-12"),
        Some(50),
    );
    assert_eq!(
        body,
        engine::serde_json::json!({
            "collections": ["sentinel-2-l2a"],
            "bbox": [126.0, 37.0, 127.0, 38.0],
            "datetime": "2026-07-01/2026-07-12",
            "limit": 50
        })
    );
}

/// merge=true: 원 body 위에 next body 를 얹는다 (겹치는 키는 next 가 이긴다).
/// merge=false: next body 만 쓴다. body 없는 GET next: body = None.
#[test]
fn apply_next_merge_semantics() {
    let original = engine::serde_json::json!({"collections": ["c"], "limit": 10});
    let next = engine::StacNext {
        href: "https://api/search".into(),
        method: "POST".into(),
        body: Some(engine::serde_json::json!({"token": "t2", "limit": 20})),
        merge: true,
    };
    let (href, method, body) = apply_next(&original, &next);
    assert_eq!((href.as_str(), method.as_str()), ("https://api/search", "POST"));
    assert_eq!(
        body.expect("merged body"),
        engine::serde_json::json!({"collections": ["c"], "limit": 20, "token": "t2"})
    );

    let replace = engine::StacNext { merge: false, ..next };
    let (_, _, body) = apply_next(&original, &replace);
    assert_eq!(body.expect("replaced"), engine::serde_json::json!({"token": "t2", "limit": 20}));

    let get = engine::StacNext {
        href: "https://api/search?page=2".into(),
        method: "GET".into(),
        body: None,
        merge: false,
    };
    let (_, method, body) = apply_next(&original, &get);
    assert_eq!(method, "GET");
    assert_eq!(body, None);
}

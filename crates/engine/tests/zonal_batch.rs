//! 배치 zonal 계약 (이슈 #60): N개 zone 을 타일 union 1회 fetch 로 집계한다.
//! 필드 리포트 4차 — 캐시가 전송 병목을 없앤 뒤 지배 비용이 된 "호출당
//! 고정비"를 상각하는 API. 값은 스칼라 경로와 비트 동일해야 하고(오라클
//! 수치는 pixel_value.rs 준거), fetch 상각은 캐시 유무와 무관하게 성립한다.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use engine::{open_cog, parse_zone_wkt, ByteSource, MemorySource, SourceError};

#[derive(Debug)]
struct CountingSource {
    inner: MemorySource,
    fetches: Arc<AtomicUsize>,
}

impl ByteSource for CountingSource {
    fn fetch(
        &self,
        range: std::ops::Range<u64>,
    ) -> engine::futures::future::BoxFuture<'_, Result<engine::bytes::Bytes, SourceError>> {
        self.fetches.fetch_add(1, Ordering::Relaxed);
        self.inner.fetch(range)
    }
}

fn fixture_bytes() -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/data/generated/basic_512x512_u16.tif");
    std::fs::read(&path)
        .unwrap_or_else(|_| panic!("픽스처 없음: {} — `just fixtures` 로 생성", path.display()))
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    engine::futures::executor::block_on(f)
}

/// pixel_value.rs 오라클 폴리곤 3본 (P1 오목+구멍 / P2 MULTIPOLYGON / P3 삼각형).
const P1: &str = "POLYGON ((301203.7 3995803.7, 304003.7 3995803.7, 304003.7 3998803.7, \
     303203.7 3998803.7, 303203.7 3996803.7, 302203.7 3996803.7, \
     302203.7 3998803.7, 301203.7 3998803.7, 301203.7 3995803.7), \
     (303403.7 3996003.7, 303803.7 3996003.7, 303803.7 3996403.7, \
     303403.7 3996403.7, 303403.7 3996003.7))";
const P2: &str = "MULTIPOLYGON (((301003.7 3999003.7, 301503.7 3999003.7, 301503.7 3999503.7, \
     301003.7 3999503.7, 301003.7 3999003.7)), \
     ((303003.7 3999003.7, 303503.7 3999003.7, 303503.7 3999503.7, \
     303003.7 3999503.7, 303003.7 3999003.7)))";
const P3: &str = "POLYGON ((300203.7 3999803.7, 301403.7 3999803.7, 300203.7 3998593.1, \
     300203.7 3999803.7))";

#[test]
fn batch_matches_scalar_results_in_input_order() {
    let (meta, reader) = block_on(open_cog(MemorySource::new(fixture_bytes()))).expect("valid COG");
    let zones: Vec<_> = [P1, P2, P3]
        .iter()
        .map(|w| parse_zone_wkt(w).expect("valid WKT"))
        .collect();

    let batch = block_on(reader.zonal_stats_polygon_batch(&meta, &zones, 1)).expect("io ok");
    assert_eq!(batch.len(), 3);
    for (b, z) in zones.iter().enumerate() {
        let scalar = block_on(reader.zonal_stats_polygon(&meta, z, 1)).expect("io ok");
        assert_eq!(
            (batch[b].count, batch[b].sum, batch[b].min, batch[b].max),
            (scalar.count, scalar.sum, scalar.min, scalar.max),
            "zone {b}: 배치 결과는 스칼라 경로와 비트 동일 (입력 순서 정렬)"
        );
    }
    // 오라클 준거 재확인 (pixel_value.rs 와 같은 수치 — 준거 이동 방지)
    assert_eq!((batch[0].count, batch[0].sum), (62_400, 2_043_359_680.0));
    assert_eq!((batch[1].count, batch[1].sum), (5_000, 165_209_622.0));
    assert_eq!((batch[2].count, batch[2].sum), (7_267, 239_110_784.0));
}

#[test]
fn batch_fetches_the_tile_union_once_even_detached() {
    // 캐시 미부착 리더에서도 성립하는 상각 계약 — 배치 자체가 union 1회 fetch.
    let fetches = Arc::new(AtomicUsize::new(0));
    let source = CountingSource {
        inner: MemorySource::new(fixture_bytes()),
        fetches: Arc::clone(&fetches),
    };
    let (meta, reader) = block_on(open_cog(source)).expect("valid COG");
    let meta_fetches = fetches.load(Ordering::Relaxed);

    // 타일 (0,0) 안의 소형 zone 20개 (tile_cache.rs 필지 격자와 동일 배치의 WKT 판)
    let zones: Vec<_> = (0..20u32)
        .map(|i| {
            let dx = f64::from(i % 5) * 300.0;
            let dy = f64::from(i / 5) * 300.0;
            let (x0, y0) = (300100.0 + dx, 3997600.0 + dy);
            let (x1, y1) = (x0 + 100.0, y0 + 100.0);
            parse_zone_wkt(&format!(
                "POLYGON (({x0} {y0}, {x1} {y0}, {x1} {y1}, {x0} {y1}, {x0} {y0}))"
            ))
            .expect("valid WKT")
        })
        .collect();

    let batch = block_on(reader.zonal_stats_polygon_batch(&meta, &zones, 1)).expect("io ok");
    assert_eq!(batch.len(), 20);
    assert!(batch.iter().all(|z| z.count == 100), "10×10px zone 들");
    assert_eq!(
        fetches.load(Ordering::Relaxed) - meta_fetches,
        1,
        "한 타일을 공유하는 20개 zone = 타일 fetch 1회 (호출당 고정비 상각의 근거)"
    );
}

#[test]
fn batch_empty_and_out_of_range_zones_yield_empty_slots() {
    let (meta, reader) = block_on(open_cog(MemorySource::new(fixture_bytes()))).expect("valid COG");
    // [래스터 밖, 유효, 래스터 밖] — 자리는 보존되고 빈 슬롯은 EMPTY 집계
    let zones = vec![
        parse_zone_wkt(
            "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))", // extent 밖
        )
        .expect("valid WKT"),
        parse_zone_wkt(P3).expect("valid WKT"),
        parse_zone_wkt(
            "POLYGON ((900000 900000, 900010 900000, 900010 900010, 900000 900010, 900000 900000))",
        )
        .expect("valid WKT"),
    ];
    let batch = block_on(reader.zonal_stats_polygon_batch(&meta, &zones, 1)).expect("io ok");
    assert_eq!(batch.len(), 3);
    assert_eq!((batch[0].count, batch[0].min), (0, None), "밖 = 빈 집계");
    assert_eq!(batch[1].count, 7_267, "가운데 유효 zone 은 제자리에서 집계");
    assert_eq!((batch[2].count, batch[2].min), (0, None));
}

#[test]
fn batch_out_of_range_band_is_all_empty_and_zero_fetch() {
    let fetches = Arc::new(AtomicUsize::new(0));
    let source = CountingSource {
        inner: MemorySource::new(fixture_bytes()),
        fetches: Arc::clone(&fetches),
    };
    let (meta, reader) = block_on(open_cog(source)).expect("valid COG");
    let meta_fetches = fetches.load(Ordering::Relaxed);
    let zones = vec![parse_zone_wkt(P3).expect("valid WKT")];
    let batch = block_on(reader.zonal_stats_polygon_batch(&meta, &zones, 9)).expect("io ok");
    assert_eq!(batch.len(), 1);
    assert_eq!(
        batch[0].count, 0,
        "범위 밖 밴드 = 빈 집계 (스칼라 규약 동일)"
    );
    assert_eq!(
        fetches.load(Ordering::Relaxed),
        meta_fetches,
        "빈 결과에 픽셀 IO 없음"
    );
}

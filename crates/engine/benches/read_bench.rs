//! T7 벤치마크 (RFC §6.9): 성능 회귀 감시 재료.
//!
//! - cold_first_pixel: open → 첫 픽셀 (RFC 의 "콜드 첫-타일 지연" — PixelQuery
//!   80ms 기준선의 인메모리 하한 관측용)
//! - warm_zonal_100x100: 열린 리더에서 100×100 영역 집계 처리량
//! - metadata_listing: read_cog_meta + 전 타일 열거 (read_cog bind 경로)
//!
//! CI 기준선 추적(회귀 게이트)은 후속 — 지금은 `just bench-smoke` 가
//! "벤치가 항상 실행 가능하다"만 판정한다.

use criterion::{criterion_group, criterion_main, Criterion};
use engine::{enumerate_tiles, open_cog, parse_zone_wkt, read_cog_meta, MemorySource};

/// #48 벤치 폴리곤: 4타일에 걸치는 오목(U-notch)+구멍 — 테스트 계약의 P1 과 동일.
const P1_WKT: &str = "POLYGON ((301203.7 3995803.7, 304003.7 3995803.7, 304003.7 3998803.7, \
     303203.7 3998803.7, 303203.7 3996803.7, 302203.7 3996803.7, \
     302203.7 3998803.7, 301203.7 3998803.7, 301203.7 3995803.7), \
     (303403.7 3996003.7, 303803.7 3996003.7, 303803.7 3996403.7, \
     303403.7 3996403.7, 303403.7 3996003.7))";

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/data/generated")
        .join(name);
    std::fs::read(&path)
        .unwrap_or_else(|_| panic!("픽스처 없음: {} — `just fixtures`", path.display()))
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    engine::futures::executor::block_on(f)
}

fn benches(c: &mut Criterion) {
    let basic = fixture("basic_512x512_u16.tif");

    c.bench_function("cold_first_pixel", |b| {
        b.iter(|| {
            let (meta, reader) =
                block_on(open_cog(MemorySource::new(basic.clone()))).expect("open");
            block_on(reader.read_pixel(&meta, 300005.0, 3999995.0, 1))
                .expect("io")
                .expect("value")
        })
    });

    let (meta, reader) = block_on(open_cog(MemorySource::new(basic.clone()))).expect("open");
    c.bench_function("warm_zonal_100x100", |b| {
        b.iter(|| {
            block_on(reader.zonal_stats(&meta, [300000.0, 3999000.0, 301000.0, 4000000.0], 1))
                .expect("io")
        })
    });

    // #48 polygon zonal — bbox 대비 PIP 오버헤드.
    // (1) 동일 100×100 윈도를 사각 폴리곤으로: 같은 픽셀 집합, 추가 비용 = 픽셀당 PIP
    let rect100 = parse_zone_wkt(
        "POLYGON ((300000 3999000, 301000 3999000, 301000 4000000, \
         300000 4000000, 300000 3999000))",
    )
    .expect("valid WKT");
    c.bench_function("warm_zonal_polygon_rect_100x100", |b| {
        b.iter(|| block_on(reader.zonal_stats_polygon(&meta, &rect100, 1)).expect("io"))
    });

    // (2) 실전 규모: P1 폴리곤(62.4k px, 10변+구멍) vs 같은 envelope bbox(84k px, 4타일)
    let p1 = parse_zone_wkt(P1_WKT).expect("valid WKT");
    c.bench_function("warm_zonal_bbox_p1_envelope", |b| {
        b.iter(|| {
            block_on(reader.zonal_stats(&meta, [301203.7, 3995803.7, 304003.7, 3998803.7], 1))
                .expect("io")
        })
    });
    c.bench_function("warm_zonal_polygon_p1", |b| {
        b.iter(|| block_on(reader.zonal_stats_polygon(&meta, &p1, 1)).expect("io"))
    });

    // (3) WKT 파싱 비용 (SQL 계층은 청크-로컬 dedupe 로 상쇄하지만 하한 관측용)
    c.bench_function("parse_zone_wkt_p1", |b| {
        b.iter(|| parse_zone_wkt(P1_WKT).expect("valid"))
    });

    c.bench_function("metadata_listing", |b| {
        b.iter(|| {
            let meta = block_on(read_cog_meta(MemorySource::new(basic.clone()))).expect("meta");
            enumerate_tiles(&meta).count()
        })
    });
}

criterion_group!(benches_group, benches);
criterion_main!(benches_group);

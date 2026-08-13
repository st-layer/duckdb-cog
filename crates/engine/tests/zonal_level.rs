//! 오버뷰 zonal fast mode 계약 (#71): 레벨 선택 · level-0 환산 스케일 ·
//! nearest 부분집합 불변식 · exact 경로 무회귀.
//!
//! basic 픽스처 = 2레벨 (512² 본체 + 256² 오버뷰, nearest — fetch_contract.rs
//! 가 레벨 수를 고정). fast 골든은 리터럴이 아니라 **exact 대비 불변식**으로
//! 판정한다 (근사치의 계약: 이슈 #71 "tolerance-band parity").

use engine::{
    level_scale, open_cog, parse_zone_wkt, select_level, MemorySource, ZonalStat, ZonalStats,
};

fn fixture_bytes(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/data/generated")
        .join(name);
    std::fs::read(&path)
        .unwrap_or_else(|_| panic!("픽스처 없음: {} — `just fixtures` 로 생성", path.display()))
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    engine::futures::executor::block_on(f)
}

/// zonal_batch.rs 오라클 P3 (exact 골든: count 7267, sum 239110784).
const P3: &str = "POLYGON ((300203.7 3999803.7, 301403.7 3999803.7, 300203.7 3998593.1, \
     300203.7 3999803.7))";
/// P3 envelope — 창 손계산: level 0 = 120×121 = 14520 px.
const P3_ENV: [f64; 4] = [300203.7, 3998593.1, 301403.7, 3999803.7];

#[test]
fn select_level_walks_fine_to_coarse_within_budget() {
    let (meta, _) = block_on(open_cog(MemorySource::new(fixture_bytes(
        "basic_512x512_u16.tif",
    ))))
    .expect("valid COG");

    assert_eq!(select_level(&meta, P3_ENV, 0), 0, "0 = exact (기본 불변)");
    assert_eq!(
        select_level(&meta, P3_ENV, 1_000_000),
        0,
        "예산이 level-0 창(14520)보다 크면 exact"
    );
    assert_eq!(
        select_level(&meta, P3_ENV, 5_000),
        1,
        "level-1 창(~60×61)이 예산에 들어오는 첫 레벨"
    );
    assert_eq!(
        select_level(&meta, P3_ENV, 100),
        1,
        "어느 레벨도 예산을 못 맞추면 최상위 오버뷰"
    );
}

#[test]
fn level_scale_is_the_pixel_area_ratio() {
    let (meta, _) = block_on(open_cog(MemorySource::new(fixture_bytes(
        "basic_512x512_u16.tif",
    ))))
    .expect("valid COG");
    assert_eq!(level_scale(&meta, 0), 1.0);
    assert_eq!(level_scale(&meta, 1), 4.0, "512²/256² = 4");
}

#[test]
fn at_level_zero_is_the_exact_path() {
    let (meta, reader) = block_on(open_cog(MemorySource::new(fixture_bytes(
        "basic_512x512_u16.tif",
    ))))
    .expect("valid COG");
    let zone = parse_zone_wkt(P3).expect("valid WKT");
    let exact = block_on(reader.zonal_stats_polygon(&meta, &zone, 1)).expect("io ok");
    let at0 = block_on(reader.zonal_stats_polygon_at(&meta, &zone, 1, 0)).expect("io ok");
    assert_eq!(exact, at0, "level 0 위임 = 기존 경로와 동일 (무회귀)");
    assert_eq!(
        (exact.count, exact.sum),
        (7_267, 239_110_784.0),
        "exact 골든"
    );
}

#[test]
fn overview_zonal_holds_tolerance_and_subset_invariants() {
    let (meta, reader) = block_on(open_cog(MemorySource::new(fixture_bytes(
        "basic_512x512_u16.tif",
    ))))
    .expect("valid COG");
    let zone = parse_zone_wkt(P3).expect("valid WKT");
    let exact = block_on(reader.zonal_stats_polygon(&meta, &zone, 1)).expect("io ok");
    let fast = block_on(reader.zonal_stats_polygon_at(&meta, &zone, 1, 1)).expect("io ok");

    // nearest 리샘플 = 원본 픽셀의 부분집합 → min 은 오르거나 유지, max 는
    // 내리거나 유지 (감쇠 방향 불변식)
    assert!(fast.min.unwrap() >= exact.min.unwrap(), "min 감쇠 방향");
    assert!(fast.max.unwrap() <= exact.max.unwrap(), "max 감쇠 방향");

    // mean 은 트렌드 정밀도 — 상대오차 5% 이내
    let (me, mf) = (exact.mean().unwrap(), fast.mean().unwrap());
    assert!((mf - me).abs() / me < 0.05, "mean 드리프트: {me} vs {mf}");

    // 환산 count/sum 은 level-0 추정치 — 상대오차 5% 이내
    let scale = level_scale(&meta, 1);
    let scaled_count = fast.value_scaled(ZonalStat::Count, scale).unwrap();
    assert!(
        (scaled_count - 7_267.0).abs() / 7_267.0 < 0.05,
        "환산 count: {scaled_count}"
    );
    let scaled_sum = fast.value_scaled(ZonalStat::Sum, scale).unwrap();
    assert!(
        (scaled_sum - 239_110_784.0).abs() / 239_110_784.0 < 0.05,
        "환산 sum: {scaled_sum}"
    );
}

#[test]
fn value_scaled_applies_only_to_count_and_sum() {
    let z = ZonalStats {
        count: 100,
        sum: 1000.0,
        min: Some(2.0),
        max: Some(50.0),
    };
    assert_eq!(z.value_scaled(ZonalStat::Count, 4.0), Some(400.0));
    assert_eq!(z.value_scaled(ZonalStat::Sum, 4.0), Some(4000.0));
    assert_eq!(
        z.value_scaled(ZonalStat::Mean, 4.0),
        Some(10.0),
        "mean 불변"
    );
    assert_eq!(z.value_scaled(ZonalStat::Min, 4.0), Some(2.0), "min 원시");
    assert_eq!(z.value_scaled(ZonalStat::Max, 4.0), Some(50.0), "max 원시");
    // scale 1.0 = 기존 value() 와 동치
    assert_eq!(
        z.value_scaled(ZonalStat::Count, 1.0),
        z.value(ZonalStat::Count)
    );
}

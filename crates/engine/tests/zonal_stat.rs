//! ZonalStat 이름 매핑 계약 (G11) — 네이티브(RS_ZonalStats)와 wasm 사이드카가
//! 공유하는 단일 소스 (#66). count/나머지 비대칭: 빈 집계에서 count → 0,
//! 나머지 → None (Sedona 의미론).

use std::str::FromStr;

use engine::{ZonalStat, ZonalStats};

#[test]
fn stat_names_parse_case_insensitively() {
    assert_eq!(ZonalStat::from_str("count").unwrap(), ZonalStat::Count);
    assert_eq!(ZonalStat::from_str("SUM").unwrap(), ZonalStat::Sum);
    assert_eq!(ZonalStat::from_str("Mean").unwrap(), ZonalStat::Mean);
    assert_eq!(ZonalStat::from_str("min").unwrap(), ZonalStat::Min);
    assert_eq!(ZonalStat::from_str("MAX").unwrap(), ZonalStat::Max);
}

#[test]
fn unknown_stat_reports_supported_list() {
    // 소문자화 후 보고 — 기존 RS_ZonalStats 에러 메시지 계약과 동일해야 한다
    let err = ZonalStat::from_str("median").unwrap_err();
    assert_eq!(err, "unknown stat 'median' (count/sum/mean/min/max)");
    let err = ZonalStat::from_str("MEDIAN").unwrap_err();
    assert_eq!(err, "unknown stat 'median' (count/sum/mean/min/max)");
}

#[test]
fn value_extracts_with_count_asymmetry() {
    let z = ZonalStats {
        count: 4,
        sum: 10.0,
        min: Some(1.0),
        max: Some(4.0),
    };
    assert_eq!(z.value(ZonalStat::Count), Some(4.0));
    assert_eq!(z.value(ZonalStat::Sum), Some(10.0));
    assert_eq!(z.value(ZonalStat::Mean), Some(2.5));
    assert_eq!(z.value(ZonalStat::Min), Some(1.0));
    assert_eq!(z.value(ZonalStat::Max), Some(4.0));

    let empty = ZonalStats {
        count: 0,
        sum: 0.0,
        min: None,
        max: None,
    };
    assert_eq!(
        empty.value(ZonalStat::Count),
        Some(0.0),
        "빈 집계 count 는 0"
    );
    assert_eq!(empty.value(ZonalStat::Sum), None);
    assert_eq!(empty.value(ZonalStat::Mean), None);
    assert_eq!(empty.value(ZonalStat::Min), None);
    assert_eq!(empty.value(ZonalStat::Max), None);
}

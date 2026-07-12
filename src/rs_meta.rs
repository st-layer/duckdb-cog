// RS_* 메타데이터 접근자 (RFC §6.8 Phase 1, G11) — Sedona RS_ 카탈로그 준거.
//
// SQL 배선만 담당한다 — 값 의미론(1-based 밴드, SRID 0, GDAL 순서, NULL 규약)은
// 전부 `engine::CogMeta` 접근자에 있다. 의미론 스냅샷: docs/sedona-semantics.md.
// (lib.rs 가 include! 로 포함하므로 //! 내부 독 주석은 못 쓴다.)

use std::collections::HashMap;
use std::error::Error;
use std::rc::Rc;

use duckdb::core::{DataChunkHandle, FlatVector, Inserter, LogicalTypeHandle, LogicalTypeId};
use duckdb::ffi::duckdb_string_t;
use duckdb::types::DuckString;
use duckdb::vscalar::{ScalarFunctionSignature, VScalar};
use duckdb::vtab::arrow::WritableVector;
use duckdb::Connection;

// crate:: 가 아니라 super:: — wasm 우회 빌드(example)에선 lib.rs 가 크레이트
// 루트가 아니므로 부모 모듈 상대 참조만 양쪽에서 성립한다.
use super::open_cog_cached;

/// 경로 하나의 IFD 메타데이터 읽기 — read_cog bind 와 동일 경로 (픽셀 미접촉).
/// 원격은 전역 캐시 경유 (#26); meta 는 작은 구조체라 clone 으로 소유권을 뗀다.
fn read_meta(fn_name: &str, path: &str) -> Result<engine::CogMeta, Box<dyn Error>> {
    let cog = open_cog_cached(path).map_err(|e| format!("{fn_name}: {e}"))?;
    Ok(cog.0.clone())
}

/// 컬럼 0(VARCHAR path)을 행별 CogMeta 로 변환. NULL 경로 → None.
/// 청크 내 같은 경로는 한 번만 읽는다 (원격은 전역 캐시도 경유 — #26).
fn meta_per_row(
    fn_name: &str,
    input: &mut DataChunkHandle,
) -> Result<Vec<Option<Rc<engine::CogMeta>>>, Box<dyn Error>> {
    let n = input.len();
    let paths = input.flat_vector(0);
    // SAFETY: 컬럼 0 은 signatures() 에서 VARCHAR 로 선언 — duckdb_string_t 표현.
    let raw = unsafe { paths.as_slice_with_len::<duckdb_string_t>(n) };
    let mut cache: HashMap<String, Rc<engine::CogMeta>> = HashMap::new();
    let mut rows = Vec::with_capacity(n);
    for (i, s) in raw.iter().enumerate() {
        if paths.row_is_null(i as u64) {
            rows.push(None);
            continue;
        }
        let path = DuckString::new(&mut { *s }).as_str().into_owned();
        let meta = match cache.get(&path) {
            Some(m) => Rc::clone(m),
            None => {
                let m = Rc::new(read_meta(fn_name, &path)?);
                cache.insert(path, Rc::clone(&m));
                m
            }
        };
        rows.push(Some(meta));
    }
    Ok(rows)
}

/// 값/NULL 행을 고정폭 타입 벡터에 쓴다.
fn write_values<T: Copy>(vector: &mut FlatVector, rows: &[Option<T>]) {
    // SAFETY: T 는 각 함수의 signatures() 반환 타입(INTEGER=i32, DOUBLE=f64)과 일치.
    let slice = unsafe { vector.as_mut_slice::<T>() };
    for (i, r) in rows.iter().enumerate() {
        if let Some(v) = r {
            slice[i] = *v;
        }
    }
    for (i, r) in rows.iter().enumerate() {
        if r.is_none() {
            vector.set_null(i);
        }
    }
}

/// CogMeta 에서 값 하나를 뽑는 함수 포인터 (행 단위, NULL = None).
type Extract<T> = fn(&engine::CogMeta) -> Option<T>;

/// 단일 path 인자 접근자의 등록 상태: SQL 이름 + CogMeta 추출 함수.
#[derive(Clone)]
struct MetaFn<T: 'static> {
    name: &'static str,
    extract: Extract<T>,
}

/// path → INTEGER 접근자 (RS_Width/Height/NumBands/SRID).
struct RsMetaInt;

impl VScalar for RsMetaInt {
    type State = MetaFn<i32>;

    fn invoke(
        state: &Self::State,
        input: &mut DataChunkHandle,
        output: &mut dyn WritableVector,
    ) -> Result<(), Box<dyn Error>> {
        let rows: Vec<Option<i32>> = meta_per_row(state.name, input)?
            .iter()
            .map(|m| m.as_deref().and_then(state.extract))
            .collect();
        write_values(&mut output.flat_vector(), &rows);
        Ok(())
    }

    fn signatures() -> Vec<ScalarFunctionSignature> {
        vec![ScalarFunctionSignature::exact(
            vec![LogicalTypeId::Varchar.into()],
            LogicalTypeId::Integer.into(),
        )]
    }
}

/// path → DOUBLE 접근자 (RS_ScaleX/Y, RS_SkewX/Y, RS_UpperLeftX/Y).
struct RsMetaDouble;

impl VScalar for RsMetaDouble {
    type State = MetaFn<f64>;

    fn invoke(
        state: &Self::State,
        input: &mut DataChunkHandle,
        output: &mut dyn WritableVector,
    ) -> Result<(), Box<dyn Error>> {
        let rows: Vec<Option<f64>> = meta_per_row(state.name, input)?
            .iter()
            .map(|m| m.as_deref().and_then(state.extract))
            .collect();
        write_values(&mut output.flat_vector(), &rows);
        Ok(())
    }

    fn signatures() -> Vec<ScalarFunctionSignature> {
        vec![ScalarFunctionSignature::exact(
            vec![LogicalTypeId::Varchar.into()],
            LogicalTypeId::Double.into(),
        )]
    }
}

/// path → VARCHAR 접근자 (RS_GeoReference).
struct RsMetaText;

impl VScalar for RsMetaText {
    type State = MetaFn<String>;

    fn invoke(
        state: &Self::State,
        input: &mut DataChunkHandle,
        output: &mut dyn WritableVector,
    ) -> Result<(), Box<dyn Error>> {
        let metas = meta_per_row(state.name, input)?;
        let mut vector = output.flat_vector();
        for (i, m) in metas.iter().enumerate() {
            match m.as_deref().and_then(state.extract) {
                Some(s) => vector.insert(i, s.as_str()),
                None => vector.set_null(i),
            }
        }
        Ok(())
    }

    fn signatures() -> Vec<ScalarFunctionSignature> {
        vec![ScalarFunctionSignature::exact(
            vec![LogicalTypeId::Varchar.into()],
            LogicalTypeId::Varchar.into(),
        )]
    }
}

/// `RS_BandNoDataValue(path[, band])` — 밴드 1-based, 기본 1.
/// 범위 밖 밴드·nodata 부재·NULL 인자 → NULL (에러 아님, RFC §6.8 규약).
struct RsBandNoDataValue;

impl VScalar for RsBandNoDataValue {
    type State = ();

    fn invoke(
        _: &Self::State,
        input: &mut DataChunkHandle,
        output: &mut dyn WritableVector,
    ) -> Result<(), Box<dyn Error>> {
        let metas = meta_per_row("RS_BandNoDataValue", input)?;
        let n = input.len();
        // 2-인자 오버로드에서만 밴드 컬럼이 존재. NULL 밴드 → None.
        let bands: Vec<Option<i32>> = if input.num_columns() > 1 {
            let v = input.flat_vector(1);
            // SAFETY: 컬럼 1 은 INTEGER 로 선언 — i32 표현.
            let raw = unsafe { v.as_slice_with_len::<i32>(n) };
            (0..n)
                .map(|i| (!v.row_is_null(i as u64)).then(|| raw[i]))
                .collect()
        } else {
            vec![Some(1); n]
        };
        let rows: Vec<Option<f64>> = metas
            .iter()
            .zip(&bands)
            .map(|(m, band)| {
                let meta = m.as_deref()?;
                let band = u32::try_from((*band)?).ok()?;
                meta.band_nodata(band)
            })
            .collect();
        write_values(&mut output.flat_vector(), &rows);
        Ok(())
    }

    fn signatures() -> Vec<ScalarFunctionSignature> {
        vec![
            ScalarFunctionSignature::exact(
                vec![LogicalTypeId::Varchar.into()],
                LogicalTypeId::Double.into(),
            ),
            ScalarFunctionSignature::exact(
                vec![LogicalTypeId::Varchar.into(), LogicalTypeId::Integer.into()],
                LogicalTypeId::Double.into(),
            ),
        ]
    }
}

/// `RS_MetaData(path)` — 개별 접근자와 동일 정보의 STRUCT 묶음.
/// Sedona 1.5 는 DOUBLE 배열을 반환하지만 우리는 DuckDB 관례의 named STRUCT
/// (문서화된 이탈 — docs/sedona-semantics.md).
struct RsMetaData;

/// RS_MetaData 반환 STRUCT 의 필드 (이름, 타입) — 선언과 쓰기 코드의 단일 준거.
const METADATA_FIELDS: [(&str, LogicalTypeId); 10] = [
    ("upperleftx", LogicalTypeId::Double),
    ("upperlefty", LogicalTypeId::Double),
    ("width", LogicalTypeId::Integer),
    ("height", LogicalTypeId::Integer),
    ("scalex", LogicalTypeId::Double),
    ("scaley", LogicalTypeId::Double),
    ("skewx", LogicalTypeId::Double),
    ("skewy", LogicalTypeId::Double),
    ("srid", LogicalTypeId::Integer),
    ("numbands", LogicalTypeId::Integer),
];

impl VScalar for RsMetaData {
    type State = ();

    fn invoke(
        _: &Self::State,
        input: &mut DataChunkHandle,
        output: &mut dyn WritableVector,
    ) -> Result<(), Box<dyn Error>> {
        let metas = meta_per_row("RS_MetaData", input)?;
        let n = metas.len();
        let mut sv = output.struct_vector();
        let f64_fields: [(usize, Extract<f64>); 6] = [
            (0, |m| m.georef.as_ref().map(|g| g.origin_x)),
            (1, |m| m.georef.as_ref().map(|g| g.origin_y)),
            (4, |m| m.georef.as_ref().map(|g| g.scale_gdal().0)),
            (5, |m| m.georef.as_ref().map(|g| g.scale_gdal().1)),
            (6, |m| m.georef.as_ref().map(|g| g.skew().0)),
            (7, |m| m.georef.as_ref().map(|g| g.skew().1)),
        ];
        for (ci, extract) in f64_fields {
            let rows: Vec<Option<f64>> = metas
                .iter()
                .map(|m| m.as_deref().and_then(extract))
                .collect();
            write_values(&mut sv.child(ci, n), &rows);
        }
        let i32_fields: [(usize, Extract<i32>); 4] = [
            (2, |m| m.width().and_then(|v| i32::try_from(v).ok())),
            (3, |m| m.height().and_then(|v| i32::try_from(v).ok())),
            (8, |m| i32::try_from(m.srid()).ok()),
            (9, |m| i32::try_from(m.num_bands).ok()),
        ];
        for (ci, extract) in i32_fields {
            let rows: Vec<Option<i32>> = metas
                .iter()
                .map(|m| m.as_deref().and_then(extract))
                .collect();
            write_values(&mut sv.child(ci, n), &rows);
        }
        for (i, m) in metas.iter().enumerate() {
            if m.is_none() {
                sv.set_null(i);
            }
        }
        Ok(())
    }

    fn signatures() -> Vec<ScalarFunctionSignature> {
        let fields: Vec<(&str, LogicalTypeHandle)> = METADATA_FIELDS
            .iter()
            .map(|(name, ty)| (*name, LogicalTypeHandle::from(*ty)))
            .collect();
        vec![ScalarFunctionSignature::exact(
            vec![LogicalTypeId::Varchar.into()],
            LogicalTypeHandle::struct_type(&fields),
        )]
    }
}

/// `RS_Value(path, x, y[, band])` — level 0 월드 좌표 픽셀값 (RFC §6.8 Phase 2).
/// extent 밖·범위 밖 밴드·nodata·NULL 인자 → NULL. 보간 없음 (floor 격자, N2).
struct RsValue;

impl VScalar for RsValue {
    type State = ();

    fn invoke(
        _: &Self::State,
        input: &mut DataChunkHandle,
        output: &mut dyn WritableVector,
    ) -> Result<(), Box<dyn Error>> {
        let n = input.len();
        let paths = input.flat_vector(0);
        // SAFETY: 컬럼 타입은 signatures() 선언(VARCHAR, DOUBLE, DOUBLE[, INTEGER])과 일치.
        let raw_paths = unsafe { paths.as_slice_with_len::<duckdb_string_t>(n) };
        let xv = input.flat_vector(1);
        let xs = unsafe { xv.as_slice_with_len::<f64>(n) };
        let yv = input.flat_vector(2);
        let ys = unsafe { yv.as_slice_with_len::<f64>(n) };
        let bands: Vec<Option<i32>> = if input.num_columns() > 3 {
            let bv = input.flat_vector(3);
            let raw = unsafe { bv.as_slice_with_len::<i32>(n) };
            (0..n)
                .map(|i| (!bv.row_is_null(i as u64)).then(|| raw[i]))
                .collect()
        } else {
            vec![Some(1); n]
        };

        // 청크 내 경로 dedupe — 파일당 open(메타 IFD 읽기) 1회, 픽셀은 행마다.
        // 청크-로컬 dedupe (전역 캐시 락 왕복 절약) — 항목은 전역 캐시와 공유 (#26)
        let mut cache: HashMap<String, std::sync::Arc<engine::SharedCog>> = HashMap::new();
        let mut rows: Vec<Option<f64>> = Vec::with_capacity(n);
        for i in 0..n {
            let band = bands[i].and_then(|b| u32::try_from(b).ok());
            if paths.row_is_null(i as u64)
                || xv.row_is_null(i as u64)
                || yv.row_is_null(i as u64)
                || bands[i].is_none()
            {
                rows.push(None);
                continue;
            }
            let path = DuckString::new(&mut { raw_paths[i] }).as_str().into_owned();
            let opened = match cache.get(&path) {
                Some(o) => std::sync::Arc::clone(o),
                None => {
                    let o = open_cog_cached(&path).map_err(|e| format!("RS_Value: {e}"))?;
                    cache.insert(path.clone(), std::sync::Arc::clone(&o));
                    o
                }
            };
            let (meta, reader) = (&opened.0, &opened.1);
            let value = match band {
                // 음수 밴드는 u32 변환 실패 → 범위 밖과 동일하게 NULL
                None => None,
                Some(b) => engine::futures::executor::block_on(
                    reader.read_pixel(meta, xs[i], ys[i], b),
                )
                .map_err(|e| format!("RS_Value: '{path}': {e}"))?,
            };
            rows.push(value);
        }
        write_values(&mut output.flat_vector(), &rows);
        Ok(())
    }

    fn signatures() -> Vec<ScalarFunctionSignature> {
        let (v, d, i) = (
            || LogicalTypeHandle::from(LogicalTypeId::Varchar),
            || LogicalTypeHandle::from(LogicalTypeId::Double),
            || LogicalTypeHandle::from(LogicalTypeId::Integer),
        );
        vec![
            ScalarFunctionSignature::exact(vec![v(), d(), d()], d()),
            ScalarFunctionSignature::exact(vec![v(), d(), d(), i()], d()),
        ]
    }
}

/// `RS_NormalizedDifference(path, x, y, band1, band2)` — (v2-v1)/(v2+v1) 포인트 값.
/// Sedona 는 raster 를 반환하지만 우리는 reader (N3) — 포인트형 이탈 (문서화).
/// 결측(extent 밖·nodata·범위 밖 밴드)·합 0·NULL 인자 → NULL.
struct RsNormalizedDifference;

impl VScalar for RsNormalizedDifference {
    type State = ();

    fn invoke(
        _: &Self::State,
        input: &mut DataChunkHandle,
        output: &mut dyn WritableVector,
    ) -> Result<(), Box<dyn Error>> {
        let n = input.len();
        let paths = input.flat_vector(0);
        // SAFETY: 컬럼 타입은 signatures() 선언(V,D,D,I,I)과 일치.
        let raw_paths = unsafe { paths.as_slice_with_len::<duckdb_string_t>(n) };
        let xv = input.flat_vector(1);
        let xs = unsafe { xv.as_slice_with_len::<f64>(n) };
        let yv = input.flat_vector(2);
        let ys = unsafe { yv.as_slice_with_len::<f64>(n) };
        let b1v = input.flat_vector(3);
        let b1s = unsafe { b1v.as_slice_with_len::<i32>(n) };
        let b2v = input.flat_vector(4);
        let b2s = unsafe { b2v.as_slice_with_len::<i32>(n) };

        // 청크-로컬 dedupe (전역 캐시 락 왕복 절약) — 항목은 전역 캐시와 공유 (#26)
        let mut cache: HashMap<String, std::sync::Arc<engine::SharedCog>> = HashMap::new();
        let mut rows: Vec<Option<f64>> = Vec::with_capacity(n);
        for i in 0..n {
            if paths.row_is_null(i as u64)
                || xv.row_is_null(i as u64)
                || yv.row_is_null(i as u64)
                || b1v.row_is_null(i as u64)
                || b2v.row_is_null(i as u64)
            {
                rows.push(None);
                continue;
            }
            let path = DuckString::new(&mut { raw_paths[i] }).as_str().into_owned();
            let opened = match cache.get(&path) {
                Some(o) => std::sync::Arc::clone(o),
                None => {
                    let o = open_cog_cached(&path).map_err(|e| format!("RS_NormalizedDifference: {e}"))?;
                    cache.insert(path.clone(), std::sync::Arc::clone(&o));
                    o
                }
            };
            let (meta, reader) = (&opened.0, &opened.1);
            // 음수/0 밴드는 u32 변환 실패 → 범위 밖과 동일하게 NULL
            let read = |b: i32| -> Result<Option<f64>, String> {
                match u32::try_from(b).ok() {
                    None => Ok(None),
                    Some(b) => {
                        engine::futures::executor::block_on(reader.read_pixel(meta, xs[i], ys[i], b))
                            .map_err(|e| format!("RS_NormalizedDifference: '{path}': {e}"))
                    }
                }
            };
            let (v1, v2) = (read(b1s[i])?, read(b2s[i])?);
            rows.push(engine::normalized_difference(v1, v2));
        }
        write_values(&mut output.flat_vector(), &rows);
        Ok(())
    }

    fn signatures() -> Vec<ScalarFunctionSignature> {
        let d = || LogicalTypeHandle::from(LogicalTypeId::Double);
        let i = || LogicalTypeHandle::from(LogicalTypeId::Integer);
        vec![ScalarFunctionSignature::exact(
            vec![LogicalTypeId::Varchar.into(), d(), d(), i(), i()],
            d(),
        )]
    }
}

/// `RS_Values(path, xs DOUBLE[], ys DOUBLE[][, band])` — 배치 픽셀 (위치 보존).
/// Sedona 는 Point geometry 배열 인자 — 우리는 좌표 배열 쌍 (geometry 타입 부재,
/// 문서화된 이탈). 리스트 인자 NULL → 결과 NULL, 원소 NULL/extent 밖/nodata →
/// 그 원소만 NULL, xs·ys 길이 불일치 → 에러.
struct RsValues;

impl VScalar for RsValues {
    type State = ();

    fn invoke(
        _: &Self::State,
        input: &mut DataChunkHandle,
        output: &mut dyn WritableVector,
    ) -> Result<(), Box<dyn Error>> {
        let n = input.len();
        let paths = input.flat_vector(0);
        // SAFETY: 컬럼 0 VARCHAR, 1/2 LIST(DOUBLE), 3 INTEGER — signatures() 선언과 일치.
        let raw_paths = unsafe { paths.as_slice_with_len::<duckdb_string_t>(n) };
        let xnull = input.flat_vector(1);
        let ynull = input.flat_vector(2);
        let xl = input.list_vector(1);
        let yl = input.list_vector(2);
        let bands: Vec<Option<i32>> = if input.num_columns() > 3 {
            let bv = input.flat_vector(3);
            let raw = unsafe { bv.as_slice_with_len::<i32>(n) };
            (0..n)
                .map(|i| (!bv.row_is_null(i as u64)).then(|| raw[i]))
                .collect()
        } else {
            vec![Some(1); n]
        };

        // 자식 슬라이스는 최대 필요 길이까지 읽는다
        let max_end = |lv: &duckdb::core::ListVector, null_v: &FlatVector| -> usize {
            (0..n)
                .filter(|i| !null_v.row_is_null(*i as u64))
                .map(|i| {
                    let (o, l) = lv.get_entry(i);
                    o + l
                })
                .max()
                .unwrap_or(0)
        };
        let (xe, ye) = (max_end(&xl, &xnull), max_end(&yl, &ynull));
        let xchild = xl.child(xe);
        let ychild = yl.child(ye);
        let xs = unsafe { xchild.as_slice_with_len::<f64>(xe) };
        let ys = unsafe { ychild.as_slice_with_len::<f64>(ye) };

        // 청크-로컬 dedupe (전역 캐시 락 왕복 절약) — 항목은 전역 캐시와 공유 (#26)
        let mut cache: HashMap<String, std::sync::Arc<engine::SharedCog>> = HashMap::new();
        // 행별 결과: None = 리스트 자체 NULL, Some(vec) = 원소별 값
        let mut rows: Vec<Option<Vec<Option<f64>>>> = Vec::with_capacity(n);
        for i in 0..n {
            let band = bands[i].and_then(|b| u32::try_from(b).ok());
            if paths.row_is_null(i as u64)
                || xnull.row_is_null(i as u64)
                || ynull.row_is_null(i as u64)
                || bands[i].is_none()
            {
                rows.push(None);
                continue;
            }
            let (xo, xn) = xl.get_entry(i);
            let (yo, yn) = yl.get_entry(i);
            if xn != yn {
                return Err(format!(
                    "RS_Values: xs/ys length mismatch ({xn} vs {yn})"
                )
                .into());
            }
            let path = DuckString::new(&mut { raw_paths[i] }).as_str().into_owned();
            let opened = match cache.get(&path) {
                Some(o) => std::sync::Arc::clone(o),
                None => {
                    let o = open_cog_cached(&path).map_err(|e| format!("RS_Values: {e}"))?;
                    cache.insert(path.clone(), std::sync::Arc::clone(&o));
                    o
                }
            };
            let (meta, reader) = (&opened.0, &opened.1);
            // 원소 NULL 은 배치에서 제외하고 자리만 보존
            let mut result = vec![None; xn];
            let mut points = Vec::with_capacity(xn);
            let mut idx = Vec::with_capacity(xn);
            for k in 0..xn {
                if xchild.row_is_null((xo + k) as u64) || ychild.row_is_null((yo + k) as u64) {
                    continue;
                }
                points.push((xs[xo + k], ys[yo + k]));
                idx.push(k);
            }
            let values = match band {
                None => vec![None; points.len()],
                Some(b) => {
                    engine::futures::executor::block_on(reader.read_pixels(meta, &points, b))
                        .map_err(|e| format!("RS_Values: '{path}': {e}"))?
                }
            };
            for (k, v) in idx.into_iter().zip(values) {
                result[k] = v;
            }
            rows.push(Some(result));
        }

        // LIST(DOUBLE) 출력: 자식 평탄화 + entry + 행 NULL (값/NULL 2패스 — 차용 규칙)
        let mut out = output.list_vector();
        let mut offsets = Vec::with_capacity(n);
        let mut total = 0usize;
        for row in &rows {
            offsets.push(total);
            total += row.as_ref().map_or(0, Vec::len);
        }
        let mut child = out.child(total);
        {
            // SAFETY: 자식은 DOUBLE — f64 표현. total 로 reserve 됨.
            let slice = unsafe { child.as_mut_slice::<f64>() };
            for (i, row) in rows.iter().enumerate() {
                if let Some(vals) = row {
                    for (k, v) in vals.iter().enumerate() {
                        if let Some(x) = v {
                            slice[offsets[i] + k] = *x;
                        }
                    }
                }
            }
        }
        for (i, row) in rows.iter().enumerate() {
            match row {
                None => {
                    out.set_entry(i, offsets[i], 0);
                    out.set_null(i);
                }
                Some(vals) => {
                    out.set_entry(i, offsets[i], vals.len());
                    for (k, v) in vals.iter().enumerate() {
                        if v.is_none() {
                            child.set_null(offsets[i] + k);
                        }
                    }
                }
            }
        }
        out.set_len(total);
        Ok(())
    }

    fn signatures() -> Vec<ScalarFunctionSignature> {
        let list_d = || LogicalTypeHandle::list(&LogicalTypeHandle::from(LogicalTypeId::Double));
        vec![
            ScalarFunctionSignature::exact(
                vec![LogicalTypeId::Varchar.into(), list_d(), list_d()],
                list_d(),
            ),
            ScalarFunctionSignature::exact(
                vec![
                    LogicalTypeId::Varchar.into(),
                    list_d(),
                    list_d(),
                    LogicalTypeId::Integer.into(),
                ],
                list_d(),
            ),
        ]
    }
}

/// `RS_BandAsArray(path, band[, bbox DOUBLE[]])` — 밴드를 row-major DOUBLE[] 로.
/// bbox 없으면 전체 level 0 밴드(georef 불요), 있으면 픽셀 중심 포함 윈도.
/// nodata → NULL 원소. 범위 밖 밴드·NULL 인자 → NULL (빈 배열과 구분).
struct RsBandAsArray;

impl VScalar for RsBandAsArray {
    type State = ();

    fn invoke(
        _: &Self::State,
        input: &mut DataChunkHandle,
        output: &mut dyn WritableVector,
    ) -> Result<(), Box<dyn Error>> {
        let n = input.len();
        let paths = input.flat_vector(0);
        // SAFETY: 컬럼 타입은 signatures() 선언(V, I[, LIST(DOUBLE)])과 일치.
        let raw_paths = unsafe { paths.as_slice_with_len::<duckdb_string_t>(n) };
        let bandv = input.flat_vector(1);
        let bands = unsafe { bandv.as_slice_with_len::<i32>(n) };
        // bbox 자식은 루프 밖에서 한 번만 차용 (RS_ZonalStats 패턴 — 행당 FFI 방지)
        let has_bbox = input.num_columns() > 2;
        let bnull = has_bbox.then(|| input.flat_vector(2));
        let blist = has_bbox.then(|| input.list_vector(2));
        let bmax = match (&bnull, &blist) {
            (Some(bn_v), Some(bl_v)) => (0..n)
                .filter(|i| !bn_v.row_is_null(*i as u64))
                .map(|i| {
                    let (o, l) = bl_v.get_entry(i);
                    o + l
                })
                .max()
                .unwrap_or(0),
            _ => 0,
        };
        let bchild = blist.as_ref().map(|bl| bl.child(bmax));
        // SAFETY: 자식은 DOUBLE — f64 표현. bmax 는 비 NULL 행들의 최대 (offset+len).
        let bvals: &[f64] = match &bchild {
            Some(c) => unsafe { c.as_slice_with_len::<f64>(bmax) },
            None => &[],
        };

        // 청크-로컬 dedupe (전역 캐시 락 왕복 절약) — 항목은 전역 캐시와 공유 (#26)
        let mut cache: HashMap<String, std::sync::Arc<engine::SharedCog>> = HashMap::new();
        let mut rows: Vec<Option<Vec<Option<f64>>>> = Vec::with_capacity(n);
        for i in 0..n {
            if paths.row_is_null(i as u64) || bandv.row_is_null(i as u64) {
                rows.push(None);
                continue;
            }
            let bbox = match (&bnull, &blist, &bchild) {
                (Some(bn_v), Some(bl_v), Some(bc)) => {
                    if bn_v.row_is_null(i as u64) {
                        rows.push(None);
                        continue;
                    }
                    let (bo, bn) = bl_v.get_entry(i);
                    if bn != 4 {
                        return Err(format!(
                            "RS_BandAsArray: bbox must have exactly 4 elements, got {bn}"
                        )
                        .into());
                    }
                    if (0..4).any(|k| bc.row_is_null((bo + k) as u64)) {
                        rows.push(None);
                        continue;
                    }
                    Some([bvals[bo], bvals[bo + 1], bvals[bo + 2], bvals[bo + 3]])
                }
                _ => None,
            };
            let path = DuckString::new(&mut { raw_paths[i] }).as_str().into_owned();
            let opened = match cache.get(&path) {
                Some(o) => std::sync::Arc::clone(o),
                None => {
                    let o = open_cog_cached(&path).map_err(|e| format!("RS_BandAsArray: {e}"))?;
                    cache.insert(path.clone(), std::sync::Arc::clone(&o));
                    o
                }
            };
            let (meta, reader) = (&opened.0, &opened.1);
            let band = u32::try_from(bands[i]).unwrap_or(0); // 음수 → 범위 밖 → NULL
            let win = engine::futures::executor::block_on(reader.band_window(meta, bbox, band))
                .map_err(|e| format!("RS_BandAsArray: '{path}': {e}"))?;
            rows.push(win);
        }

        // LIST(DOUBLE) 출력 — RsValues 와 동일 2패스
        let mut out = output.list_vector();
        let mut offsets = Vec::with_capacity(n);
        let mut total = 0usize;
        for row in &rows {
            offsets.push(total);
            total += row.as_ref().map_or(0, Vec::len);
        }
        let mut child = out.child(total);
        {
            // SAFETY: 자식은 DOUBLE — f64 표현. total 로 reserve 됨.
            let slice = unsafe { child.as_mut_slice::<f64>() };
            for (i, row) in rows.iter().enumerate() {
                if let Some(vals) = row {
                    for (k, v) in vals.iter().enumerate() {
                        if let Some(x) = v {
                            slice[offsets[i] + k] = *x;
                        }
                    }
                }
            }
        }
        for (i, row) in rows.iter().enumerate() {
            match row {
                None => {
                    out.set_entry(i, offsets[i], 0);
                    out.set_null(i);
                }
                Some(vals) => {
                    out.set_entry(i, offsets[i], vals.len());
                    for (k, v) in vals.iter().enumerate() {
                        if v.is_none() {
                            child.set_null(offsets[i] + k);
                        }
                    }
                }
            }
        }
        out.set_len(total);
        Ok(())
    }

    fn signatures() -> Vec<ScalarFunctionSignature> {
        let list_d = || LogicalTypeHandle::list(&LogicalTypeHandle::from(LogicalTypeId::Double));
        vec![
            ScalarFunctionSignature::exact(
                vec![LogicalTypeId::Varchar.into(), LogicalTypeId::Integer.into()],
                list_d(),
            ),
            ScalarFunctionSignature::exact(
                vec![
                    LogicalTypeId::Varchar.into(),
                    LogicalTypeId::Integer.into(),
                    list_d(),
                ],
                list_d(),
            ),
        ]
    }
}

/// `RS_BandStats(path[, band])` — GDAL_METADATA 의 STATISTICS_* (§6.7).
/// decode 없는 카탈로그 통계 — 태그 부재·범위 밖 밴드·NULL 인자 → NULL
/// (fallback 은 RS_ZonalStats 의 decode 경로).
struct RsBandStats;

impl VScalar for RsBandStats {
    type State = ();

    fn invoke(
        _: &Self::State,
        input: &mut DataChunkHandle,
        output: &mut dyn WritableVector,
    ) -> Result<(), Box<dyn Error>> {
        let n = input.len();
        let metas = meta_per_row("RS_BandStats", input)?;
        let bands: Vec<Option<i32>> = if input.num_columns() > 1 {
            let v = input.flat_vector(1);
            // SAFETY: 컬럼 1 은 INTEGER 로 선언 — i32 표현.
            let raw = unsafe { v.as_slice_with_len::<i32>(n) };
            (0..n)
                .map(|i| (!v.row_is_null(i as u64)).then(|| raw[i]))
                .collect()
        } else {
            vec![Some(1); n]
        };
        let rows: Vec<Option<engine::BandStats>> = metas
            .iter()
            .zip(&bands)
            .map(|(m, band)| {
                let meta = m.as_deref()?;
                let idx = usize::try_from((*band)? - 1).ok()?;
                meta.band_stats.as_ref()?.get(idx).cloned()
            })
            .collect();

        let mut sv = output.struct_vector();
        type Pick = fn(&engine::BandStats) -> Option<f64>;
        let fields: [(usize, Pick); 4] = [
            (0, |b| b.min),
            (1, |b| b.max),
            (2, |b| b.mean),
            (3, |b| b.stddev),
        ];
        for (ci, pick) in fields {
            let vals: Vec<Option<f64>> = rows
                .iter()
                .map(|r| r.as_ref().and_then(pick))
                .collect();
            write_values(&mut sv.child(ci, n), &vals);
        }
        for (i, r) in rows.iter().enumerate() {
            if r.is_none() {
                sv.set_null(i);
            }
        }
        Ok(())
    }

    fn signatures() -> Vec<ScalarFunctionSignature> {
        let d = || LogicalTypeHandle::from(LogicalTypeId::Double);
        let ret = || {
            LogicalTypeHandle::struct_type(&[
                ("min", d()),
                ("max", d()),
                ("mean", d()),
                ("stddev", d()),
            ])
        };
        vec![
            ScalarFunctionSignature::exact(vec![LogicalTypeId::Varchar.into()], ret()),
            ScalarFunctionSignature::exact(
                vec![LogicalTypeId::Varchar.into(), LogicalTypeId::Integer.into()],
                ret(),
            ),
        ]
    }
}

/// `RS_ZonalStats(path, bbox DOUBLE[], band, stat)` — bbox 영역 집계 (RFC §6.8).
/// zone 은 geometry 가 아니라 bbox (GEOS 비링크 N4 하의 적응, 문서화 이탈).
/// stat ∈ {count, sum, mean, min, max} (대소문자 무관). 유효 픽셀 없으면
/// count → 0, 나머지 → NULL. NULL 인자 → NULL.
struct RsZonalStats;

impl VScalar for RsZonalStats {
    type State = ();

    fn invoke(
        _: &Self::State,
        input: &mut DataChunkHandle,
        output: &mut dyn WritableVector,
    ) -> Result<(), Box<dyn Error>> {
        let n = input.len();
        let paths = input.flat_vector(0);
        // SAFETY: 컬럼 타입은 signatures() 선언(V, LIST(DOUBLE), I, V)과 일치.
        let raw_paths = unsafe { paths.as_slice_with_len::<duckdb_string_t>(n) };
        let bnull = input.flat_vector(1);
        let bl = input.list_vector(1);
        let bmax = (0..n)
            .filter(|i| !bnull.row_is_null(*i as u64))
            .map(|i| {
                let (o, l) = bl.get_entry(i);
                o + l
            })
            .max()
            .unwrap_or(0);
        let bchild = bl.child(bmax);
        let bvals = unsafe { bchild.as_slice_with_len::<f64>(bmax) };
        let bandv = input.flat_vector(2);
        let bands = unsafe { bandv.as_slice_with_len::<i32>(n) };
        let statv = input.flat_vector(3);
        let stats = unsafe { statv.as_slice_with_len::<duckdb_string_t>(n) };

        // 청크-로컬 dedupe (전역 캐시 락 왕복 절약) — 항목은 전역 캐시와 공유 (#26)
        let mut cache: HashMap<String, std::sync::Arc<engine::SharedCog>> = HashMap::new();
        let mut rows: Vec<Option<f64>> = Vec::with_capacity(n);
        for i in 0..n {
            if paths.row_is_null(i as u64)
                || bnull.row_is_null(i as u64)
                || bandv.row_is_null(i as u64)
                || statv.row_is_null(i as u64)
            {
                rows.push(None);
                continue;
            }
            let stat = DuckString::new(&mut { stats[i] }).as_str().to_lowercase();
            if !matches!(stat.as_str(), "count" | "sum" | "mean" | "min" | "max") {
                return Err(format!(
                    "RS_ZonalStats: unknown stat '{stat}' (count/sum/mean/min/max)"
                )
                .into());
            }
            let (bo, bn) = bl.get_entry(i);
            if bn != 4 {
                return Err(format!(
                    "RS_ZonalStats: bbox must have exactly 4 elements [xmin, ymin, xmax, ymax], got {bn}"
                )
                .into());
            }
            // 원소 NULL 은 NaN 으로 흘러 engine 검증(비유한)이 에러로 승격하기 전에
            // 행 NULL 로 처리 (다른 함수들의 NULL 인자 규약과 정합)
            if (0..4).any(|k| bchild.row_is_null((bo + k) as u64)) {
                rows.push(None);
                continue;
            }
            let bbox = [bvals[bo], bvals[bo + 1], bvals[bo + 2], bvals[bo + 3]];
            let path = DuckString::new(&mut { raw_paths[i] }).as_str().into_owned();
            let opened = match cache.get(&path) {
                Some(o) => std::sync::Arc::clone(o),
                None => {
                    let o = open_cog_cached(&path).map_err(|e| format!("RS_ZonalStats: {e}"))?;
                    cache.insert(path.clone(), std::sync::Arc::clone(&o));
                    o
                }
            };
            let (meta, reader) = (&opened.0, &opened.1);
            let band = u32::try_from(bands[i]).unwrap_or(0); // 음수 → 범위 밖 → 빈 집계
            let z = engine::futures::executor::block_on(reader.zonal_stats(meta, bbox, band))
                .map_err(|e| format!("RS_ZonalStats: '{path}': {e}"))?;
            rows.push(match stat.as_str() {
                "count" => Some(z.count as f64),
                "sum" => (z.count > 0).then_some(z.sum),
                "mean" => z.mean(),
                "min" => z.min,
                "max" => z.max,
                _ => unreachable!(),
            });
        }
        write_values(&mut output.flat_vector(), &rows);
        Ok(())
    }

    fn signatures() -> Vec<ScalarFunctionSignature> {
        vec![ScalarFunctionSignature::exact(
            vec![
                LogicalTypeId::Varchar.into(),
                LogicalTypeHandle::list(&LogicalTypeHandle::from(LogicalTypeId::Double)),
                LogicalTypeId::Integer.into(),
                LogicalTypeId::Varchar.into(),
            ],
            LogicalTypeId::Double.into(),
        )]
    }
}

/// `RS_WorldToRasterCoord(path, x, y)` — 월드 좌표 → 1-based 그리드 STRUCT(col, row).
/// 순수 변환 (경계 검사 없음, Sedona 준거). NULL 인자 → NULL, georef 없음 → 에러.
struct RsWorldToRasterCoord;

impl VScalar for RsWorldToRasterCoord {
    type State = ();

    fn invoke(
        _: &Self::State,
        input: &mut DataChunkHandle,
        output: &mut dyn WritableVector,
    ) -> Result<(), Box<dyn Error>> {
        let metas = meta_per_row("RS_WorldToRasterCoord", input)?;
        let n = input.len();
        let xv = input.flat_vector(1);
        // SAFETY: 컬럼 1/2 는 DOUBLE 로 선언 — f64 표현.
        let xs = unsafe { xv.as_slice_with_len::<f64>(n) };
        let yv = input.flat_vector(2);
        let ys = unsafe { yv.as_slice_with_len::<f64>(n) };
        let mut rows: Vec<Option<(i64, i64)>> = Vec::with_capacity(n);
        for (i, m) in metas.iter().enumerate() {
            if xv.row_is_null(i as u64) || yv.row_is_null(i as u64) {
                rows.push(None);
                continue;
            }
            match m.as_deref() {
                None => rows.push(None),
                Some(meta) => {
                    let g = meta.georef.as_ref().ok_or_else(|| {
                        "RS_WorldToRasterCoord: coordinate lookup requires a georeferenced COG"
                            .to_string()
                    })?;
                    rows.push(Some(g.world_to_raster(xs[i], ys[i])));
                }
            }
        }
        // i32 초과 좌표는 부분-NULL 대신 **struct 전체 NULL** (계약 단순화)
        let rows: Vec<Option<(i32, i32)>> = rows
            .iter()
            .map(|r| {
                r.and_then(|(c, w)| Some((i32::try_from(c).ok()?, i32::try_from(w).ok()?)))
            })
            .collect();
        let mut sv = output.struct_vector();
        for ci in 0..2usize {
            let vals: Vec<Option<i32>> = rows
                .iter()
                .map(|r| r.map(|cr| if ci == 0 { cr.0 } else { cr.1 }))
                .collect();
            write_values(&mut sv.child(ci, n), &vals);
        }
        for (i, r) in rows.iter().enumerate() {
            if r.is_none() {
                sv.set_null(i);
            }
        }
        Ok(())
    }

    fn signatures() -> Vec<ScalarFunctionSignature> {
        let d = || LogicalTypeHandle::from(LogicalTypeId::Double);
        vec![ScalarFunctionSignature::exact(
            vec![LogicalTypeId::Varchar.into(), d(), d()],
            LogicalTypeHandle::struct_type(&[
                ("col", LogicalTypeHandle::from(LogicalTypeId::Integer)),
                ("row", LogicalTypeHandle::from(LogicalTypeId::Integer)),
            ]),
        )]
    }
}

/// `RS_RasterToWorldCoord(path, col, row)` — 1-based 픽셀 좌상단 코너의 월드 좌표
/// STRUCT(x, y). NULL 인자 → NULL, georef 없음 → 에러.
struct RsRasterToWorldCoord;

impl VScalar for RsRasterToWorldCoord {
    type State = ();

    fn invoke(
        _: &Self::State,
        input: &mut DataChunkHandle,
        output: &mut dyn WritableVector,
    ) -> Result<(), Box<dyn Error>> {
        let metas = meta_per_row("RS_RasterToWorldCoord", input)?;
        let n = input.len();
        let cv = input.flat_vector(1);
        // SAFETY: 컬럼 1/2 는 INTEGER 로 선언 — i32 표현.
        let cols = unsafe { cv.as_slice_with_len::<i32>(n) };
        let rv = input.flat_vector(2);
        let rws = unsafe { rv.as_slice_with_len::<i32>(n) };
        let mut rows: Vec<Option<(f64, f64)>> = Vec::with_capacity(n);
        for (i, m) in metas.iter().enumerate() {
            if cv.row_is_null(i as u64) || rv.row_is_null(i as u64) {
                rows.push(None);
                continue;
            }
            match m.as_deref() {
                None => rows.push(None),
                Some(meta) => {
                    let g = meta.georef.as_ref().ok_or_else(|| {
                        "RS_RasterToWorldCoord: coordinate lookup requires a georeferenced COG"
                            .to_string()
                    })?;
                    rows.push(Some(
                        g.raster_to_world(i64::from(cols[i]), i64::from(rws[i])),
                    ));
                }
            }
        }
        let mut sv = output.struct_vector();
        for ci in 0..2usize {
            let vals: Vec<Option<f64>> = rows
                .iter()
                .map(|r| r.map(|xy| if ci == 0 { xy.0 } else { xy.1 }))
                .collect();
            write_values(&mut sv.child(ci, n), &vals);
        }
        for (i, r) in rows.iter().enumerate() {
            if r.is_none() {
                sv.set_null(i);
            }
        }
        Ok(())
    }

    fn signatures() -> Vec<ScalarFunctionSignature> {
        let i = || LogicalTypeHandle::from(LogicalTypeId::Integer);
        vec![ScalarFunctionSignature::exact(
            vec![LogicalTypeId::Varchar.into(), i(), i()],
            LogicalTypeHandle::struct_type(&[
                ("x", LogicalTypeHandle::from(LogicalTypeId::Double)),
                ("y", LogicalTypeHandle::from(LogicalTypeId::Double)),
            ]),
        )]
    }
}

/// RS_* 전 함수 등록 — extension_entrypoint 에서 호출.
pub(crate) fn register(con: &Connection) -> duckdb::Result<()> {
    let int_fns: [MetaFn<i32>; 4] = [
        MetaFn {
            name: "RS_Width",
            extract: |m| m.width().and_then(|v| i32::try_from(v).ok()),
        },
        MetaFn {
            name: "RS_Height",
            extract: |m| m.height().and_then(|v| i32::try_from(v).ok()),
        },
        MetaFn {
            name: "RS_NumBands",
            extract: |m| i32::try_from(m.num_bands).ok(),
        },
        MetaFn {
            name: "RS_SRID",
            extract: |m| i32::try_from(m.srid()).ok(),
        },
    ];
    for f in int_fns {
        con.register_scalar_function_with_state::<RsMetaInt>(f.name, &f)?;
    }
    let double_fns: [MetaFn<f64>; 6] = [
        MetaFn {
            name: "RS_ScaleX",
            extract: |m| m.georef.as_ref().map(|g| g.scale_gdal().0),
        },
        MetaFn {
            name: "RS_ScaleY",
            extract: |m| m.georef.as_ref().map(|g| g.scale_gdal().1),
        },
        MetaFn {
            name: "RS_SkewX",
            extract: |m| m.georef.as_ref().map(|g| g.skew().0),
        },
        MetaFn {
            name: "RS_SkewY",
            extract: |m| m.georef.as_ref().map(|g| g.skew().1),
        },
        MetaFn {
            name: "RS_UpperLeftX",
            extract: |m| m.georef.as_ref().map(|g| g.origin_x),
        },
        MetaFn {
            name: "RS_UpperLeftY",
            extract: |m| m.georef.as_ref().map(|g| g.origin_y),
        },
    ];
    for f in double_fns {
        con.register_scalar_function_with_state::<RsMetaDouble>(f.name, &f)?;
    }
    con.register_scalar_function_with_state::<RsMetaText>(
        "RS_GeoReference",
        &MetaFn {
            name: "RS_GeoReference",
            extract: |m| m.georeference_gdal(),
        },
    )?;
    con.register_scalar_function::<RsBandNoDataValue>("RS_BandNoDataValue")?;
    con.register_scalar_function::<RsMetaData>("RS_MetaData")?;
    con.register_scalar_function::<RsValue>("RS_Value")?;
    con.register_scalar_function::<RsValues>("RS_Values")?;
    con.register_scalar_function::<RsNormalizedDifference>("RS_NormalizedDifference")?;
    con.register_scalar_function::<RsZonalStats>("RS_ZonalStats")?;
    con.register_scalar_function::<RsBandAsArray>("RS_BandAsArray")?;
    con.register_scalar_function::<RsBandStats>("RS_BandStats")?;
    con.register_scalar_function::<RsWorldToRasterCoord>("RS_WorldToRasterCoord")?;
    con.register_scalar_function::<RsRasterToWorldCoord>("RS_RasterToWorldCoord")?;
    Ok(())
}

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
use super::FileSource;

/// 경로 하나의 IFD 메타데이터 읽기 — read_cog bind 와 동일 경로 (픽셀 미접촉).
fn read_meta(fn_name: &str, path: &str) -> Result<engine::CogMeta, Box<dyn Error>> {
    let source =
        FileSource::open(path).map_err(|e| format!("{fn_name}: cannot open '{path}': {e}"))?;
    Ok(
        engine::futures::executor::block_on(engine::read_cog_meta(source))
            .map_err(|e| format!("{fn_name}: '{path}': {e}"))?,
    )
}

/// 컬럼 0(VARCHAR path)을 행별 CogMeta 로 변환. NULL 경로 → None.
/// 청크 내 같은 경로는 한 번만 읽는다 (전역 캐시는 두지 않음 — Phase 1 단순성).
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
    Ok(())
}

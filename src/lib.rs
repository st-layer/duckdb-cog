//! duckdb-cog: GDAL-free COG reader extension for DuckDB.
//! 설계 준거: docs/RFC-001-rev3.md. 구조 준거: duckdb/extension-template-rs.
//!
//! 부트스트랩 단계: `cog_version()` table function 하나만 등록해
//! "빌드 → LOAD → 쿼리" 전체 경로가 살아있음을 증명한다.
//! Phase 0에서 `read_cog()` 가 이 자리에 들어온다.

use duckdb::{
    core::{DataChunkHandle, Inserter, LogicalTypeHandle, LogicalTypeId},
    duckdb_entrypoint_c_api,
    vtab::{BindInfo, InitInfo, TableFunctionInfo, VTab},
    Connection, Result,
};
use std::{
    error::Error,
    ffi::CString,
    sync::atomic::{AtomicBool, Ordering},
};

#[repr(C)]
struct VersionBindData;

#[repr(C)]
struct VersionInitData {
    done: AtomicBool,
}

/// `SELECT * FROM cog_version();` → 'duckdb-cog 0.0.1'
struct VersionVTab;

impl VTab for VersionVTab {
    type InitData = VersionInitData;
    type BindData = VersionBindData;

    fn bind(bind: &BindInfo) -> Result<Self::BindData, Box<dyn Error>> {
        bind.add_result_column("version", LogicalTypeHandle::from(LogicalTypeId::Varchar));
        Ok(VersionBindData)
    }

    fn init(_: &InitInfo) -> Result<Self::InitData, Box<dyn Error>> {
        Ok(VersionInitData {
            done: AtomicBool::new(false),
        })
    }

    fn func(
        func: &TableFunctionInfo<Self>,
        output: &mut DataChunkHandle,
    ) -> Result<(), Box<dyn Error>> {
        let init_data = func.get_init_data();
        if init_data.done.swap(true, Ordering::Relaxed) {
            output.set_len(0);
        } else {
            // engine 크레이트 배선 확인: 타일 키 왕복이 성립해야 버전을 내놓는다
            let key = engine::pack_tile_key(0, 1, 2).ok_or("engine wiring broken")?;
            if engine::unpack_tile_key(key) != (0, 1, 2) {
                return Err("engine tile key roundtrip failed".into());
            }
            let vector = output.flat_vector(0);
            let result = CString::new(format!("duckdb-cog {}", env!("CARGO_PKG_VERSION")))?;
            vector.insert(0, result);
            output.set_len(1);
        }
        Ok(())
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        None
    }
}

#[repr(C)]
struct ReadCogBindData {
    /// 다음 슬라이스에서 async-tiff reader 경계로 전달될 COG 경로.
    /// 스켈레톤 단계에서는 보관만 한다.
    #[allow(dead_code)]
    path: String,
}

#[repr(C)]
struct ReadCogInitData {
    done: AtomicBool,
}

/// `SELECT * FROM read_cog(path);` — 타일 테이블 스켈레톤 (RFC §6.4 부분집합).
///
/// 실제 COG는 아직 읽지 않는다. 고정 더미 타일 1행을 반환해
/// 스키마 계약과 engine 배선(pack_tile_key)만 세운다.
struct ReadCogVTab;

impl VTab for ReadCogVTab {
    type InitData = ReadCogInitData;
    type BindData = ReadCogBindData;

    fn bind(bind: &BindInfo) -> Result<Self::BindData, Box<dyn Error>> {
        bind.add_result_column("id", LogicalTypeHandle::from(LogicalTypeId::Bigint));
        bind.add_result_column("level", LogicalTypeHandle::from(LogicalTypeId::Integer));
        bind.add_result_column("tile_x", LogicalTypeHandle::from(LogicalTypeId::Integer));
        bind.add_result_column("tile_y", LogicalTypeHandle::from(LogicalTypeId::Integer));
        bind.add_result_column("cols", LogicalTypeHandle::from(LogicalTypeId::Integer));
        bind.add_result_column("rows", LogicalTypeHandle::from(LogicalTypeId::Integer));
        Ok(ReadCogBindData {
            path: bind.get_parameter(0).to_string(),
        })
    }

    fn init(_: &InitInfo) -> Result<Self::InitData, Box<dyn Error>> {
        Ok(ReadCogInitData {
            done: AtomicBool::new(false),
        })
    }

    fn func(
        func: &TableFunctionInfo<Self>,
        output: &mut DataChunkHandle,
    ) -> Result<(), Box<dyn Error>> {
        let init_data = func.get_init_data();
        if init_data.done.swap(true, Ordering::Relaxed) {
            output.set_len(0);
            return Ok(());
        }
        // 더미 타일 (level=0, tile_x=1, tile_y=2) — id 를 engine 키 packing 으로 생성
        const LEVEL: u8 = 0;
        const TILE_X: u32 = 1;
        const TILE_Y: u32 = 2;
        let id = engine::pack_tile_key(LEVEL, TILE_X, TILE_Y).ok_or("tile key out of range")?;
        // SAFETY: 각 벡터는 bind 에서 선언한 컬럼 타입(BIGINT=i64, INTEGER=i32)과 일치하고,
        // 인덱스 0 은 DuckDB 표준 벡터 용량 안이다.
        unsafe {
            output.flat_vector(0).as_mut_slice::<i64>()[0] = id as i64;
            for (col, value) in [
                (1, LEVEL as i32),
                (2, TILE_X as i32),
                (3, TILE_Y as i32),
                (4, 256), // cols
                (5, 256), // rows
            ] {
                output.flat_vector(col).as_mut_slice::<i32>()[0] = value;
            }
        }
        output.set_len(1);
        Ok(())
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        Some(vec![LogicalTypeHandle::from(LogicalTypeId::Varchar)])
    }
}

#[duckdb_entrypoint_c_api]
pub unsafe fn extension_entrypoint(con: Connection) -> Result<(), Box<dyn Error>> {
    con.register_table_function::<VersionVTab>("cog_version")?;
    con.register_table_function::<ReadCogVTab>("read_cog")?;
    Ok(())
}

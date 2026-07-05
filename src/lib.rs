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

#[duckdb_entrypoint_c_api]
pub unsafe fn extension_entrypoint(con: Connection) -> Result<(), Box<dyn Error>> {
    con.register_table_function::<VersionVTab>("cog_version")?;
    Ok(())
}

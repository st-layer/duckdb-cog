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

/// 로컬 파일 [`engine::ByteSource`] — seek+read_exact 기반 (이식성 우선).
///
/// Phase 0 스파이크 경로: 원격(object store/http) 소스는 다음 슬라이스에서
/// §6.5 결정과 함께 들어온다.
#[cfg(not(target_os = "emscripten"))]
#[derive(Debug)]
struct FileSource {
    path: String,
    file: std::sync::Mutex<std::fs::File>,
    len: u64,
}

#[cfg(not(target_os = "emscripten"))]
impl FileSource {
    fn open(path: &str) -> std::io::Result<Self> {
        let file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Self {
            path: path.to_string(),
            file: std::sync::Mutex::new(file),
            len,
        })
    }
}

#[cfg(not(target_os = "emscripten"))]
impl engine::ByteSource for FileSource {
    fn fetch(
        &self,
        range: std::ops::Range<u64>,
    ) -> engine::futures::future::BoxFuture<
        '_,
        std::result::Result<engine::bytes::Bytes, engine::SourceError>,
    > {
        use std::io::{Read, Seek, SeekFrom};
        Box::pin(async move {
            let err = |msg: String| engine::SourceError(format!("{}: {msg}", self.path));
            if range.start > range.end || range.end > self.len {
                return Err(err(format!(
                    "range {}..{} out of bounds (file len {})",
                    range.start, range.end, self.len
                )));
            }
            let mut buf = vec![0u8; (range.end - range.start) as usize];
            let mut file = self.file.lock().map_err(|e| err(e.to_string()))?;
            file.seek(SeekFrom::Start(range.start))
                .and_then(|_| file.read_exact(&mut buf))
                .map_err(|e| err(e.to_string()))?;
            Ok(engine::bytes::Bytes::from(buf))
        })
    }
}

#[cfg(not(target_os = "emscripten"))]
#[repr(C)]
struct ReadCogBindData {
    /// bind 시점에 메타데이터만 읽어 확정된 전체 타일 목록 (픽셀 미접촉).
    tiles: Vec<engine::TileRow>,
}

#[cfg(not(target_os = "emscripten"))]
#[repr(C)]
struct ReadCogInitData {
    cursor: std::sync::atomic::AtomicUsize,
}

/// `SELECT * FROM read_cog(path);` — COG 타일 그리드 나열 (RFC §6.4 부분집합).
///
/// bind 에서 IFD 메타데이터만 읽어(레벨 = 본체 + 오버뷰) 타일 행을 만든다.
/// cols/rows 는 TIFF 물리 타일 크기다 (엣지 클리핑 아님).
#[cfg(not(target_os = "emscripten"))]
struct ReadCogVTab;

#[cfg(not(target_os = "emscripten"))]
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

        let path = bind.get_parameter(0).to_string();
        let source =
            FileSource::open(&path).map_err(|e| format!("read_cog: cannot open '{path}': {e}"))?;
        // 로컬 파일 fetch 는 항상 즉시 완료되는 future 라 전용 런타임 없이 block_on 으로 충분.
        let meta = engine::futures::executor::block_on(engine::read_cog_meta(source))
            .map_err(|e| format!("read_cog: '{path}': {e}"))?;
        Ok(ReadCogBindData {
            tiles: engine::enumerate_tiles(&meta).collect(),
        })
    }

    fn init(_: &InitInfo) -> Result<Self::InitData, Box<dyn Error>> {
        Ok(ReadCogInitData {
            cursor: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    fn func(
        func: &TableFunctionInfo<Self>,
        output: &mut DataChunkHandle,
    ) -> Result<(), Box<dyn Error>> {
        /// DuckDB 표준 벡터 용량 (STANDARD_VECTOR_SIZE)
        const CHUNK: usize = 2048;
        let tiles = &func.get_bind_data().tiles;
        let start = func
            .get_init_data()
            .cursor
            .fetch_add(CHUNK, Ordering::Relaxed);
        if start >= tiles.len() {
            output.set_len(0);
            return Ok(());
        }
        let batch = &tiles[start..(start + CHUNK).min(tiles.len())];
        // SAFETY: 각 벡터는 bind 에서 선언한 컬럼 타입(BIGINT=i64, INTEGER=i32)과 일치하고,
        // batch 길이는 CHUNK(표준 벡터 용량) 이하다.
        unsafe {
            let mut ids = output.flat_vector(0);
            let ids = ids.as_mut_slice::<i64>();
            for (i, t) in batch.iter().enumerate() {
                ids[i] = t.id as i64;
            }
            for (col, get) in [
                (
                    1usize,
                    (|t: &engine::TileRow| t.level as i32) as fn(&engine::TileRow) -> i32,
                ),
                (2, |t| t.tile_x as i32),
                (3, |t| t.tile_y as i32),
                (4, |t| t.cols as i32),
                (5, |t| t.rows as i32),
            ] {
                let mut v = output.flat_vector(col);
                let v = v.as_mut_slice::<i32>();
                for (i, t) in batch.iter().enumerate() {
                    v[i] = get(t);
                }
            }
        }
        output.set_len(batch.len());
        Ok(())
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        Some(vec![LogicalTypeHandle::from(LogicalTypeId::Varchar)])
    }
}

#[duckdb_entrypoint_c_api]
pub unsafe fn extension_entrypoint(con: Connection) -> Result<(), Box<dyn Error>> {
    con.register_table_function::<VersionVTab>("cog_version")?;
    #[cfg(not(target_os = "emscripten"))]
    con.register_table_function::<ReadCogVTab>("read_cog")?;
    Ok(())
}

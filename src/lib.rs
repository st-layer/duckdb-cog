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

/// RS_* 메타데이터 접근자 스칼라 함수 (RFC §6.8 Phase 1).
/// include!: wasm 우회 빌드(example)가 lib.rs 를 비루트 모듈로 포함하면 `mod x;` 의
/// 파일 해석 경로가 갈라진다 — include! 는 이 파일 기준 상대경로라 양쪽에서 동작.
#[cfg(not(target_os = "emscripten"))]
mod rs_meta {
    include!("rs_meta.rs");
}

/// §6.5(b) I/O 경로 실측 하네스 (이슈 #30, experimental — 결정 후 제거 예정).
#[cfg(not(target_os = "emscripten"))]
mod io_bench {
    include!("io_bench.rs");
}

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
/// 원격 경로는 [`ObjectStoreSource`], 스킴 분기는 [`open_source`].
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
            if range.start > range.end || range.start >= self.len {
                return Err(err(format!(
                    "range {}..{} out of bounds (file len {})",
                    range.start, range.end, self.len
                )));
            }
            // EOF 클램프 (ByteSource 계약): end 만 넘으면 가용 분 반환
            let end = range.end.min(self.len);
            let mut buf = vec![0u8; (end - range.start) as usize];
            let mut file = self.file.lock().map_err(|e| err(e.to_string()))?;
            file.seek(SeekFrom::Start(range.start))
                .and_then(|_| file.read_exact(&mut buf))
                .map_err(|e| err(e.to_string()))?;
            Ok(engine::bytes::Bytes::from(buf))
        })
    }
}

/// 원격 객체 저장소 [`engine::ByteSource`] — object_store 직행 (RFC §6.5 (a)).
///
/// 스킴(http/https/s3/file …)은 object_store `parse_url` 이 해석하고, s3 등의
/// 자격증명은 환경변수로 공급된다 (object_store 관례). IO future 는 익스텐션
/// 수명의 tokio 런타임에 스폰되므로 호출측 executor 는 JoinHandle 만 기다린다.
#[cfg(not(target_os = "emscripten"))]
#[derive(Debug)]
struct ObjectStoreSource {
    url: String,
    store: std::sync::Arc<dyn object_store::ObjectStore>,
    location: object_store::path::Path,
}

/// 익스텐션 수명의 tokio 런타임 — 의도적으로 leak (unload 시 drop 하면
/// blocking 컨텍스트 panic; DuckDB 는 사실상 프로세스 종료까지 unload 안 함).
/// worker 1개 = 동시 원격 쿼리의 처리량 상한 — 병목 실측 후 조정 (worklog 참조).
#[cfg(not(target_os = "emscripten"))]
fn tokio_runtime() -> &'static tokio::runtime::Runtime {
    static RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("duckdb-cog: tokio runtime init failed")
    })
}

#[cfg(not(target_os = "emscripten"))]
impl ObjectStoreSource {
    fn open(url_str: &str) -> std::result::Result<Self, String> {
        let err = |e: &dyn std::fmt::Display| format!("cannot open '{url_str}': {e}");
        let url = url::Url::parse(url_str).map_err(|e| err(&e))?;
        // 전체 env 전달은 object_store 의 문서화된 관례 (AWS_* 자격증명 등) —
        // 인식 안 되는 키는 조용히 무시된다. http 엔드포인트의 s3(minio 등)는
        // AWS_ALLOW_HTTP=true 를 env 로 줘야 한다.
        let mut opts: Vec<(String, String)> = std::env::vars().collect();
        if url.scheme() == "http" {
            // object_store 는 기본 HTTPS-only ("URL scheme is not allowed") —
            // 평문 http 는 명시적 opt-in (로컬 테스트 서버·사내 http 스토리지).
            opts.push(("allow_http".into(), "true".into()));
        }
        let (store, location) = object_store::parse_url_opts(&url, opts).map_err(|e| err(&e))?;
        Ok(Self {
            url: url_str.to_string(),
            store: store.into(),
            location,
        })
    }
}

#[cfg(not(target_os = "emscripten"))]
impl engine::ByteSource for ObjectStoreSource {
    fn fetch(
        &self,
        range: std::ops::Range<u64>,
    ) -> engine::futures::future::BoxFuture<
        '_,
        std::result::Result<engine::bytes::Bytes, engine::SourceError>,
    > {
        use object_store::ObjectStoreExt as _;
        let store = std::sync::Arc::clone(&self.store);
        let location = self.location.clone();
        let url = self.url.clone();
        let task = tokio_runtime().spawn(async move { store.get_range(&location, range).await });
        Box::pin(async move {
            let err = |e: String| engine::SourceError(format!("{url}: {e}"));
            task.await
                .map_err(|e| err(e.to_string()))?
                .map_err(|e| err(e.to_string()))
        })
    }
}

/// 경로 스킴에 따라 로컬/원격 소스를 연다 — read_cog · RS_ 공용 진입점.
/// 에러 문자열에는 함수명 접두어가 없다 (호출측이 붙인다).
/// `file://` 도 object_store(LocalFileSystem) 로 간다 — 절대경로 요구 등
/// 의미론이 FileSource(스킴 없는 경로)와 다름에 유의.
#[cfg(not(target_os = "emscripten"))]
fn open_source(path: &str) -> std::result::Result<Box<dyn engine::ByteSource>, String> {
    if path.contains("://") {
        Ok(Box::new(ObjectStoreSource::open(path)?))
    } else {
        let source = FileSource::open(path).map_err(|e| format!("cannot open '{path}': {e}"))?;
        Ok(Box::new(source))
    }
}

#[cfg(not(target_os = "emscripten"))]
#[repr(C)]
struct ReadCogBindData {
    /// bind 시점에 메타데이터만 읽어 확정된 전체 타일 목록 (픽셀 미접촉).
    tiles: Vec<engine::TileRow>,
    /// 파일 단위 CRS ("EPSG:32652" 꼴) — 행마다 복제 노출, 부재 시 NULL.
    crs: Option<std::ffi::CString>,
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
        let double = || LogicalTypeHandle::from(LogicalTypeId::Double);
        bind.add_result_column(
            "bbox",
            LogicalTypeHandle::struct_type(&[
                ("xmin", double()),
                ("ymin", double()),
                ("xmax", double()),
                ("ymax", double()),
            ]),
        );
        bind.add_result_column("crs", LogicalTypeHandle::from(LogicalTypeId::Varchar));

        let path = bind.get_parameter(0).to_string();
        // bbox := [xmin, ymin, xmax, ymax] — 유효성·교차 의미론은 engine 에 (§6.6)
        let filter = match bind.get_named_parameter("bbox") {
            None => None,
            Some(v) if v.is_null() => None,
            Some(v) => {
                let items = v
                    .to_list()
                    .ok_or("read_cog: bbox must be a list of 4 doubles")?;
                if items.len() != 4 {
                    return Err(format!(
                        "read_cog: bbox must have exactly 4 elements [xmin, ymin, xmax, ymax], got {}",
                        items.len()
                    )
                    .into());
                }
                // to_double 은 타입 검사 없이 실패 시 NaN — named_parameters() 의
                // LIST(DOUBLE) 선언이 캐스팅을 보장하고, NULL 원소의 NaN 은
                // engine 의 is_finite 검증이 거른다 (선언 타입 바꾸면 같이 볼 것).
                let f: Vec<f64> = items.iter().map(|x| x.to_double()).collect();
                Some([f[0], f[1], f[2], f[3]])
            }
        };
        let source = open_source(&path).map_err(|e| format!("read_cog: {e}"))?;
        // 원격 IO 는 소스 내부에서 tokio 런타임에 스폰되므로, 여기 block_on 은
        // JoinHandle(로컬은 즉시 완료 future)만 폴링한다 — 전용 executor 불필요.
        let meta = engine::futures::executor::block_on(engine::read_cog_meta(source))
            .map_err(|e| format!("read_cog: '{path}': {e}"))?;
        Ok(ReadCogBindData {
            tiles: engine::enumerate_tiles_filtered(&meta, filter)
                .map_err(|e| format!("read_cog: '{path}': {e}"))?,
            crs: meta.crs().map(std::ffi::CString::new).transpose()?,
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
            // bbox STRUCT: 자식(xmin,ymin,xmax,ymax) 채우고, georef 부재 행은 struct NULL
            let mut bbox = output.struct_vector(6);
            for ci in 0..4 {
                let mut child = bbox.child(ci, batch.len());
                let child = child.as_mut_slice::<f64>();
                for (i, t) in batch.iter().enumerate() {
                    if let Some(b) = t.bbox {
                        child[i] = b[ci];
                    }
                }
            }
            for (i, t) in batch.iter().enumerate() {
                if t.bbox.is_none() {
                    bbox.set_null(i);
                }
            }
        }
        let mut crs_vec = output.flat_vector(7);
        match &func.get_bind_data().crs {
            Some(crs) => {
                for i in 0..batch.len() {
                    crs_vec.insert(i, crs.clone());
                }
            }
            None => {
                for i in 0..batch.len() {
                    crs_vec.set_null(i);
                }
            }
        }
        output.set_len(batch.len());
        Ok(())
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        Some(vec![LogicalTypeHandle::from(LogicalTypeId::Varchar)])
    }

    fn named_parameters() -> Option<Vec<(String, LogicalTypeHandle)>> {
        Some(vec![(
            "bbox".to_string(),
            LogicalTypeHandle::list(&LogicalTypeHandle::from(LogicalTypeId::Double)),
        )])
    }
}

#[cfg(not(target_os = "emscripten"))]
#[repr(C)]
struct ReadStacBindData {
    /// bind 시점에 문서 전체를 읽어 확정된 (item, asset) 행들.
    rows: Vec<engine::StacAssetRow>,
}

#[cfg(not(target_os = "emscripten"))]
#[repr(C)]
struct ReadStacInitData {
    cursor: std::sync::atomic::AtomicUsize,
}

/// `SELECT * FROM read_stac(url);` — STAC Item/ItemCollection 을 (item, asset)
/// 행으로 (RFC §6.7). 파싱·행 모델은 engine (graceful degradation 포함).
#[cfg(not(target_os = "emscripten"))]
struct ReadStacVTab;

#[cfg(not(target_os = "emscripten"))]
impl VTab for ReadStacVTab {
    type InitData = ReadStacInitData;
    type BindData = ReadStacBindData;

    fn bind(bind: &BindInfo) -> Result<Self::BindData, Box<dyn Error>> {
        for col in [
            "item_id",
            "collection",
            "datetime",
            "asset_key",
            "href",
            "media_type",
        ] {
            bind.add_result_column(col, LogicalTypeHandle::from(LogicalTypeId::Varchar));
        }
        let double = || LogicalTypeHandle::from(LogicalTypeId::Double);
        bind.add_result_column(
            "bbox",
            LogicalTypeHandle::struct_type(&[
                ("xmin", double()),
                ("ymin", double()),
                ("xmax", double()),
                ("ymax", double()),
            ]),
        );
        // raster:bands 통계 (§6.7) — 확장 부재 시 NULL
        bind.add_result_column(
            "band_stats",
            LogicalTypeHandle::list(&LogicalTypeHandle::struct_type(&[
                ("min", double()),
                ("max", double()),
                ("mean", double()),
                ("stddev", double()),
            ])),
        );
        let url = bind.get_parameter(0).to_string();
        let source = open_source(&url).map_err(|e| format!("read_stac: {e}"))?;
        let bytes = engine::futures::executor::block_on(engine::fetch_all(&source))
            .map_err(|e| format!("read_stac: '{url}': {e}"))?;
        let rows = engine::parse_stac(&bytes).map_err(|e| format!("read_stac: '{url}': {e}"))?;
        Ok(ReadStacBindData { rows })
    }

    fn init(_: &InitInfo) -> Result<Self::InitData, Box<dyn Error>> {
        Ok(ReadStacInitData {
            cursor: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    fn func(
        func: &TableFunctionInfo<Self>,
        output: &mut DataChunkHandle,
    ) -> Result<(), Box<dyn Error>> {
        const CHUNK: usize = 2048;
        let rows = &func.get_bind_data().rows;
        let start = func
            .get_init_data()
            .cursor
            .fetch_add(CHUNK, Ordering::Relaxed);
        if start >= rows.len() {
            output.set_len(0);
            return Ok(());
        }
        let batch = &rows[start..(start + CHUNK).min(rows.len())];
        // VARCHAR 6컬럼: 값 insert / 결측 set_null
        for (col, get) in [
            (
                0usize,
                (|r: &engine::StacAssetRow| Some(r.item_id.as_str()))
                    as fn(&engine::StacAssetRow) -> Option<&str>,
            ),
            (1, |r| r.collection.as_deref()),
            (2, |r| r.datetime.as_deref()),
            (3, |r| Some(r.asset_key.as_str())),
            (4, |r| Some(r.href.as_str())),
            (5, |r| r.media_type.as_deref()),
        ] {
            let mut v = output.flat_vector(col);
            for (i, row) in batch.iter().enumerate() {
                match get(row) {
                    Some(s) => v.insert(i, s),
                    None => v.set_null(i),
                }
            }
        }
        // bbox STRUCT — read_cog 와 동일 패턴
        // SAFETY: 자식 4개는 DOUBLE 로 선언 — f64 표현, batch 길이는 CHUNK 이하.
        unsafe {
            let mut bbox = output.struct_vector(6);
            for ci in 0..4 {
                let mut child = bbox.child(ci, batch.len());
                let child = child.as_mut_slice::<f64>();
                for (i, row) in batch.iter().enumerate() {
                    if let Some(b) = row.bbox {
                        child[i] = b[ci];
                    }
                }
            }
            for (i, row) in batch.iter().enumerate() {
                if row.bbox.is_none() {
                    bbox.set_null(i);
                }
            }
        }
        // band_stats LIST(STRUCT(...)): 자식 struct 를 평탄화해 채운다
        {
            let mut lv = output.list_vector(7);
            let mut offsets = Vec::with_capacity(batch.len());
            let mut total = 0usize;
            for row in batch {
                offsets.push(total);
                total += row.band_stats.as_ref().map_or(0, Vec::len);
            }
            let sv = lv.struct_child(total);
            type Pick = fn(&engine::BandStats) -> Option<f64>;
            let fields: [(usize, Pick); 4] = [
                (0, |b| b.min),
                (1, |b| b.max),
                (2, |b| b.mean),
                (3, |b| b.stddev),
            ];
            for (ci, pick) in fields {
                let mut child = sv.child(ci, total);
                // SAFETY: 자식 필드는 DOUBLE — f64 표현, total 로 용량 확보됨.
                {
                    let slice = unsafe { child.as_mut_slice::<f64>() };
                    for (i, row) in batch.iter().enumerate() {
                        if let Some(bands) = &row.band_stats {
                            for (k, b) in bands.iter().enumerate() {
                                if let Some(v) = pick(b) {
                                    slice[offsets[i] + k] = v;
                                }
                            }
                        }
                    }
                }
                for (i, row) in batch.iter().enumerate() {
                    if let Some(bands) = &row.band_stats {
                        for (k, b) in bands.iter().enumerate() {
                            if pick(b).is_none() {
                                child.set_null(offsets[i] + k);
                            }
                        }
                    }
                }
            }
            for (i, row) in batch.iter().enumerate() {
                match &row.band_stats {
                    None => {
                        lv.set_entry(i, offsets[i], 0);
                        lv.set_null(i);
                    }
                    Some(bands) => lv.set_entry(i, offsets[i], bands.len()),
                }
            }
            lv.set_len(total);
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
    {
        con.register_table_function::<ReadCogVTab>("read_cog")?;
        con.register_table_function::<ReadStacVTab>("read_stac")?;
        rs_meta::register(&con)?;
        io_bench::register(&con)?;
    }
    Ok(())
}

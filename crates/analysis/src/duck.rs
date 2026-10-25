//! Small binding to the pinned DuckDB 2.0.0 public C API (duckdb.h).
//! Handles/results/appenders have one Rust owner and are destroyed before their
//! connection. DuckDB is a bundled native library, not a server or subprocess.
use serde_json::Value;
use std::ffi::{c_char, c_void, CStr, CString};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

type Handle = *mut c_void;
#[repr(C)]
#[derive(Default)]
struct RawResult {
    columns: u64,
    rows: u64,
    changed: u64,
    column_data: Handle,
    error: *mut c_char,
    internal: Handle,
}
#[repr(C)]
struct DuckString {
    data: *mut c_char,
    size: u64,
}

#[link(name = "duckdb")]
extern "C" {
    fn duckdb_create_config(out: *mut Handle) -> u32;
    fn duckdb_set_config(config: Handle, name: *const c_char, value: *const c_char) -> u32;
    fn duckdb_destroy_config(config: *mut Handle);
    fn duckdb_open_ext(
        path: *const c_char,
        out: *mut Handle,
        config: Handle,
        error: *mut *mut c_char,
    ) -> u32;
    fn duckdb_close(db: *mut Handle);
    fn duckdb_connect(db: Handle, out: *mut Handle) -> u32;
    fn duckdb_disconnect(connection: *mut Handle);
    fn duckdb_query(connection: Handle, query: *const c_char, result: *mut RawResult) -> u32;
    fn duckdb_destroy_result(result: *mut RawResult);
    fn duckdb_result_error(result: *mut RawResult) -> *const c_char;
    fn duckdb_row_count(result: *mut RawResult) -> u64;
    fn duckdb_value_string(result: *mut RawResult, col: u64, row: u64) -> DuckString;
    fn duckdb_free(pointer: *mut c_void);
    fn duckdb_interrupt(connection: Handle);
    fn duckdb_library_version() -> *const c_char;
    fn duckdb_extract_statements(connection: Handle, sql: *const c_char, out: *mut Handle) -> u64;
    fn duckdb_extract_statements_error(extracted: Handle) -> *const c_char;
    fn duckdb_destroy_extracted(extracted: *mut Handle);
    fn duckdb_prepare_extracted_statement(
        connection: Handle,
        extracted: Handle,
        index: u64,
        out: *mut Handle,
    ) -> u32;
    fn duckdb_prepare_error(prepared: Handle) -> *const c_char;
    fn duckdb_prepared_statement_type(prepared: Handle) -> u32;
    fn duckdb_destroy_prepare(prepared: *mut Handle);
    fn duckdb_appender_create(
        connection: Handle,
        schema: *const c_char,
        table: *const c_char,
        out: *mut Handle,
    ) -> u32;
    fn duckdb_appender_error(appender: Handle) -> *const c_char;
    fn duckdb_appender_close(appender: Handle) -> u32;
    fn duckdb_appender_destroy(appender: *mut Handle) -> u32;
    fn duckdb_appender_end_row(appender: Handle) -> u32;
    fn duckdb_append_null(appender: Handle) -> u32;
    fn duckdb_append_uint64(appender: Handle, value: u64) -> u32;
    fn duckdb_append_int64(appender: Handle, value: i64) -> u32;
    fn duckdb_append_bool(appender: Handle, value: bool) -> u32;
    fn duckdb_append_varchar_length(appender: Handle, value: *const c_char, length: u64) -> u32;
    fn duckdb_append_blob(appender: Handle, value: *const c_void, length: u64) -> u32;
}

fn text(pointer: *const c_char) -> String {
    if pointer.is_null() {
        return "DuckDB operation failed".into();
    }
    // DuckDB owns a valid terminated error/name string until its handle is closed.
    unsafe { CStr::from_ptr(pointer) }
        .to_string_lossy()
        .chars()
        .take(2000)
        .collect()
}
pub(crate) fn invalid(s: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, s)
}
pub(crate) fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
fn cstring(s: &str) -> io::Result<CString> {
    CString::new(s).map_err(|_| invalid("NUL in SQL"))
}

pub(crate) struct Db {
    database: Handle,
    connection: Handle,
}
impl Db {
    pub fn new() -> io::Result<Self> {
        Self::with_limit("128MiB")
    }
    pub fn analysis() -> io::Result<Self> {
        Self::with_limit("256MiB")
    }
    pub fn comparison() -> io::Result<Self> {
        Self::with_limit("512MiB")
    }
    fn with_limit(memory: &str) -> io::Result<Self> {
        unsafe {
            let mut config = std::ptr::null_mut();
            if duckdb_create_config(&mut config) != 0 {
                return Err(io::Error::other("DuckDB config failed"));
            }
            for (key, value) in [
                ("threads", "2"),
                ("memory_limit", memory),
                ("autoinstall_known_extensions", "false"),
                ("autoload_known_extensions", "false"),
                ("temp_directory", ""),
            ] {
                let key = cstring(key)?;
                let value = cstring(value)?;
                if duckdb_set_config(config, key.as_ptr(), value.as_ptr()) != 0 {
                    duckdb_destroy_config(&mut config);
                    return Err(io::Error::other("unsupported DuckDB configuration"));
                }
            }
            let mut database = std::ptr::null_mut();
            let mut error = std::ptr::null_mut();
            let code = duckdb_open_ext(std::ptr::null(), &mut database, config, &mut error);
            duckdb_destroy_config(&mut config);
            if code != 0 {
                let detail = text(error);
                duckdb_free(error.cast());
                return Err(io::Error::other(detail));
            }
            let mut connection = std::ptr::null_mut();
            if duckdb_connect(database, &mut connection) != 0 {
                duckdb_close(&mut database);
                return Err(io::Error::other("DuckDB connection failed"));
            }
            Ok(Self {
                database,
                connection,
            })
        }
    }
    pub fn version() -> String {
        unsafe { text(duckdb_library_version()) }
    }
    pub fn exec(&self, sql: &str) -> io::Result<()> {
        self.execute(sql, None).map(|_| ())
    }
    pub fn query(&self, sql: &str, cancel: Option<&AtomicBool>) -> io::Result<Rows> {
        // DuckDB serializes a row with its native JSON implementation. We only
        // fetch one VARCHAR column, avoiding deprecated heterogeneous cell APIs.
        self.execute(
            &format!("SELECT to_json(macscan_row) FROM (\n{sql}\n) AS macscan_row"),
            cancel,
        )
    }
    fn execute(&self, sql: &str, cancel: Option<&AtomicBool>) -> io::Result<Rows> {
        let sql = cstring(sql)?;
        let mut raw = RawResult::default();
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "query cancelled",
            ));
        }
        let code = if let Some(cancel) = cancel {
            let finished = AtomicBool::new(false);
            let connection = self.connection as usize;
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    while !finished.load(Ordering::Acquire) {
                        if cancel.load(Ordering::Relaxed) {
                            unsafe { duckdb_interrupt(connection as Handle) };
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                });
                let code = unsafe { duckdb_query(self.connection, sql.as_ptr(), &mut raw) };
                finished.store(true, Ordering::Release);
                code
            })
        } else {
            unsafe { duckdb_query(self.connection, sql.as_ptr(), &mut raw) }
        };
        let rows = Rows(raw);
        if code != 0 {
            if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "query cancelled",
                ));
            }
            return Err(io::Error::other(unsafe {
                text(duckdb_result_error(&rows.0 as *const _ as *mut _))
            }));
        }
        Ok(rows)
    }
    pub fn validate_select(&self, sql: &str) -> io::Result<()> {
        if sql.len() > 16 * 1024 {
            return Err(invalid("SQL exceeds 16 KiB"));
        }
        let sql = cstring(sql)?;
        unsafe {
            let mut extracted = std::ptr::null_mut();
            let count = duckdb_extract_statements(self.connection, sql.as_ptr(), &mut extracted);
            if count != 1 {
                let detail = if count == 0 {
                    text(duckdb_extract_statements_error(extracted))
                } else {
                    "one SELECT statement is required".into()
                };
                duckdb_destroy_extracted(&mut extracted);
                return Err(invalid(&detail));
            }
            let mut prepared = std::ptr::null_mut();
            let code =
                duckdb_prepare_extracted_statement(self.connection, extracted, 0, &mut prepared);
            let kind = if code == 0 {
                duckdb_prepared_statement_type(prepared)
            } else {
                0
            };
            let error = if code != 0 {
                Some(text(duckdb_prepare_error(prepared)))
            } else {
                None
            };
            duckdb_destroy_prepare(&mut prepared);
            duckdb_destroy_extracted(&mut extracted);
            if let Some(error) = error {
                return Err(invalid(&error));
            }
            if kind != 1 {
                return Err(invalid(
                    "query accepts SELECT/WITH returning rows, not write statements",
                ));
            }
        }
        Ok(())
    }
    pub fn appender(&self, table: &str) -> io::Result<Appender<'_>> {
        let table = cstring(table)?;
        let mut raw = std::ptr::null_mut();
        if unsafe {
            duckdb_appender_create(self.connection, std::ptr::null(), table.as_ptr(), &mut raw)
        } != 0
        {
            let detail = unsafe { text(duckdb_appender_error(raw)) };
            unsafe { duckdb_appender_destroy(&mut raw) };
            return Err(io::Error::other(detail));
        }
        Ok(Appender { raw, _db: self })
    }
}
impl Drop for Db {
    fn drop(&mut self) {
        unsafe {
            duckdb_disconnect(&mut self.connection);
            duckdb_close(&mut self.database);
        }
    }
}

pub(crate) struct Rows(RawResult);
impl Rows {
    pub fn len(&mut self) -> usize {
        unsafe { duckdb_row_count(&mut self.0) as usize }
    }
    pub fn row(&mut self, index: usize) -> io::Result<Value> {
        if index >= self.len() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "entry/result row not found",
            ));
        }
        let value = unsafe { duckdb_value_string(&mut self.0, 0, index as u64) };
        if value.data.is_null() {
            return Err(io::Error::other("DuckDB row conversion failed"));
        }
        if value.size > 8 * 1024 {
            unsafe { duckdb_free(value.data.cast()) };
            return Err(invalid(
                "one record exceeds the agent response budget; select fewer details",
            ));
        }
        let parsed = serde_json::from_slice::<Value>(unsafe {
            std::slice::from_raw_parts(value.data.cast::<u8>(), value.size as usize)
        });
        unsafe { duckdb_free(value.data.cast()) };
        let mut record = parsed.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fn exact_numbers(value: &mut Value) {
            match value {
                Value::Number(n) => *value = Value::String(n.to_string()),
                Value::Array(a) => a.iter_mut().for_each(exact_numbers),
                Value::Object(m) => m.values_mut().for_each(exact_numbers),
                _ => {}
            }
        }
        exact_numbers(&mut record);
        Ok(record)
    }
}
impl Drop for Rows {
    fn drop(&mut self) {
        unsafe { duckdb_destroy_result(&mut self.0) }
    }
}

pub(crate) struct Appender<'a> {
    raw: Handle,
    _db: &'a Db,
}
impl Appender<'_> {
    fn check(&self, code: u32) -> io::Result<()> {
        if code == 0 {
            Ok(())
        } else {
            Err(io::Error::other(unsafe {
                text(duckdb_appender_error(self.raw))
            }))
        }
    }
    pub fn number(&mut self, n: Option<u64>) -> io::Result<()> {
        self.check(unsafe {
            if let Some(n) = n {
                duckdb_append_uint64(self.raw, n)
            } else {
                duckdb_append_null(self.raw)
            }
        })
    }
    pub fn signed(&mut self, n: i64) -> io::Result<()> {
        self.check(unsafe { duckdb_append_int64(self.raw, n) })
    }
    pub fn boolean(&mut self, b: bool) -> io::Result<()> {
        self.check(unsafe { duckdb_append_bool(self.raw, b) })
    }
    pub fn text(&mut self, s: &str) -> io::Result<()> {
        self.check(unsafe {
            duckdb_append_varchar_length(self.raw, s.as_ptr().cast(), s.len() as u64)
        })
    }
    pub fn blob(&mut self, s: &[u8]) -> io::Result<()> {
        self.check(unsafe { duckdb_append_blob(self.raw, s.as_ptr().cast(), s.len() as u64) })
    }
    pub fn end(&mut self) -> io::Result<()> {
        self.check(unsafe { duckdb_appender_end_row(self.raw) })
    }
    pub fn finish(self) -> io::Result<()> {
        self.check(unsafe { duckdb_appender_close(self.raw) })
    }
}
impl Drop for Appender<'_> {
    fn drop(&mut self) {
        unsafe { duckdb_appender_destroy(&mut self.raw) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_engine_queries_exact_integers_and_nulls() {
        let db = Db::new().unwrap();
        assert_eq!(Db::version(), "v2.0.0");
        let row = db
            .query(
                "SELECT 18446744073709551615::UBIGINT AS bytes,NULL AS unknown,true AS ok",
                None,
            )
            .unwrap()
            .row(0)
            .unwrap();
        assert_eq!(row["bytes"], "18446744073709551615");
        assert!(row["unknown"].is_null());
        assert_eq!(row["ok"], true);
        assert!(db.validate_select("SELECT 1").is_ok());
        assert!(db.validate_select("SELECT 1; SELECT 2").is_err());
    }
}

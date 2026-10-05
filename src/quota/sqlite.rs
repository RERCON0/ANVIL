//! Read-only queries against SQLite databases other programs own (the OMP and
//! OpenCode 2 login stores) through the system's `winsqlite3.dll`, loaded from
//! System32 only. Both databases are in WAL mode and written by running CLIs,
//! which a hand-written file reader could not follow; no SQLite crate is linked.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::Path;

use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;
const SQLITE_OPEN_READONLY: c_int = 0x0000_0001;
#[cfg(test)]
const SQLITE_OPEN_READWRITE: c_int = 0x0000_0002;
#[cfg(test)]
const SQLITE_OPEN_CREATE: c_int = 0x0000_0004;
/// `SQLITE_TRANSIENT`: SQLite copies the bound text before `bind` returns.
const SQLITE_TRANSIENT: isize = -1;
const BUSY_TIMEOUT_MS: c_int = 500;
/// A login table holds a handful of rows; more is not a login store.
const MAX_ROWS: usize = 64;
const MAX_VALUE_BYTES: usize = 64 * 1024;

type Db = c_void;
type Stmt = c_void;

type OpenV2 = unsafe extern "system" fn(*const c_char, *mut *mut Db, c_int, *const c_char) -> c_int;
type BusyTimeout = unsafe extern "system" fn(*mut Db, c_int) -> c_int;
type PrepareV2 = unsafe extern "system" fn(*mut Db, *const c_char, c_int, *mut *mut Stmt, *mut *const c_char) -> c_int;
type BindText = unsafe extern "system" fn(*mut Stmt, c_int, *const c_char, c_int, isize) -> c_int;
type Step = unsafe extern "system" fn(*mut Stmt) -> c_int;
type ColumnText = unsafe extern "system" fn(*mut Stmt, c_int) -> *const u8;
type ColumnBytes = unsafe extern "system" fn(*mut Stmt, c_int) -> c_int;
type Finalize = unsafe extern "system" fn(*mut Stmt) -> c_int;
type CloseV2 = unsafe extern "system" fn(*mut Db) -> c_int;
type ErrMsg = unsafe extern "system" fn(*mut Db) -> *const c_char;
#[cfg(test)]
type Exec = unsafe extern "system" fn(*mut Db, *const c_char, *const c_void, *mut c_void, *mut *mut c_char) -> c_int;

struct Api {
    open_v2: OpenV2,
    busy_timeout: BusyTimeout,
    prepare_v2: PrepareV2,
    bind_text: BindText,
    step: Step,
    column_text: ColumnText,
    column_bytes: ColumnBytes,
    finalize: Finalize,
    close_v2: CloseV2,
    errmsg: ErrMsg,
    #[cfg(test)]
    exec: Exec,
}

/// The loaded library. `None` from [`Sqlite::load`] means the source is simply
/// unavailable; the DLL is never unloaded (it lives for the process).
pub struct Sqlite {
    api: Api,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SqliteError {
    /// The file does not exist: the CLI was never set up.
    Missing,
    /// SQLite result code and its message (never row contents).
    Sqlite(c_int, String),
    /// The path cannot be passed to SQLite (interior NUL).
    BadPath,
}

impl Sqlite {
    pub fn load() -> Option<Sqlite> {
        let name: Vec<u16> = "winsqlite3.dll".encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: NUL-terminated name; System32-only search keeps a planted
        // DLL next to the executable or in the working directory out.
        let module = unsafe { LoadLibraryExW(name.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
        if module.is_null() {
            return None;
        }
        macro_rules! sym {
            ($name:literal, $ty:ty) => {{
                // SAFETY: the symbol name is NUL-terminated; the signature it is
                // transmuted to is the documented SQLite C API for that name.
                let proc = unsafe { GetProcAddress(module, concat!($name, "\0").as_ptr()) }?;
                unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, $ty>(proc) }
            }};
        }
        Some(Sqlite {
            api: Api {
                open_v2: sym!("sqlite3_open_v2", OpenV2),
                busy_timeout: sym!("sqlite3_busy_timeout", BusyTimeout),
                prepare_v2: sym!("sqlite3_prepare_v2", PrepareV2),
                bind_text: sym!("sqlite3_bind_text", BindText),
                step: sym!("sqlite3_step", Step),
                column_text: sym!("sqlite3_column_text", ColumnText),
                column_bytes: sym!("sqlite3_column_bytes", ColumnBytes),
                finalize: sym!("sqlite3_finalize", Finalize),
                close_v2: sym!("sqlite3_close_v2", CloseV2),
                errmsg: sym!("sqlite3_errmsg", ErrMsg),
                #[cfg(test)]
                exec: sym!("sqlite3_exec", Exec),
            },
        })
    }

    /// Runs one statement with `?1 = param` on a read-only connection and
    /// returns the first `columns` columns of each row as text (NULL → "").
    pub fn query(&self, path: &Path, sql: &str, param: &str, columns: usize) -> Result<Vec<Vec<String>>, SqliteError> {
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_file() => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(SqliteError::Missing),
            _ => return Err(SqliteError::Sqlite(14, "cannot read database file".to_owned())),
        }
        let db = self.open(path, SQLITE_OPEN_READONLY)?;
        let sql = CString::new(sql).map_err(|_| SqliteError::BadPath)?;
        let param = CString::new(param).map_err(|_| SqliteError::BadPath)?;
        let mut stmt: *mut Stmt = std::ptr::null_mut();
        // SAFETY: valid connection; `sql` is NUL-terminated (length -1).
        let rc = unsafe { (self.api.prepare_v2)(db.raw, sql.as_ptr(), -1, &mut stmt, std::ptr::null_mut()) };
        if rc != SQLITE_OK {
            return Err(self.error(&db, rc));
        }
        let stmt = Statement { api: &self.api, raw: stmt };
        // SAFETY: valid statement; SQLITE_TRANSIENT makes SQLite copy the text.
        let rc = unsafe { (self.api.bind_text)(stmt.raw, 1, param.as_ptr(), -1, SQLITE_TRANSIENT) };
        if rc != SQLITE_OK {
            return Err(self.error(&db, rc));
        }
        let mut rows = Vec::new();
        loop {
            // SAFETY: valid statement.
            match unsafe { (self.api.step)(stmt.raw) } {
                SQLITE_ROW => {
                    if rows.len() == MAX_ROWS {
                        break;
                    }
                    rows.push((0..columns).map(|column| stmt.text(column as c_int)).collect());
                }
                SQLITE_DONE => break,
                rc => return Err(self.error(&db, rc)),
            }
        }
        Ok(rows)
    }

    fn open(&self, path: &Path, flags: c_int) -> Result<Connection<'_>, SqliteError> {
        let path = CString::new(path.to_str().ok_or(SqliteError::BadPath)?).map_err(|_| SqliteError::BadPath)?;
        let mut raw: *mut Db = std::ptr::null_mut();
        // SAFETY: NUL-terminated UTF-8 path, out-pointer to a local, default VFS.
        let rc = unsafe { (self.api.open_v2)(path.as_ptr(), &mut raw, flags, std::ptr::null()) };
        // Even a failed open returns a handle that must be closed.
        let db = Connection { api: &self.api, raw };
        if rc != SQLITE_OK {
            return Err(self.error(&db, rc));
        }
        // SAFETY: valid connection.
        unsafe { (self.api.busy_timeout)(db.raw, BUSY_TIMEOUT_MS) };
        Ok(db)
    }

    fn error(&self, db: &Connection<'_>, rc: c_int) -> SqliteError {
        let message = if db.raw.is_null() {
            String::new()
        } else {
            // SAFETY: errmsg returns a NUL-terminated string owned by the connection.
            unsafe { CStr::from_ptr((self.api.errmsg)(db.raw)) }.to_string_lossy().chars().take(200).collect()
        };
        SqliteError::Sqlite(rc, message)
    }

    /// Test fixture writer: creates or changes a database with full rights.
    /// Production code has no write path.
    #[cfg(test)]
    pub fn exec_for_test(&self, path: &Path, sql: &str) {
        let db = self.open(path, SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE).expect("open for test");
        let sql = CString::new(sql).unwrap();
        // SAFETY: valid connection, NUL-terminated SQL, no callback.
        let rc = unsafe {
            (self.api.exec)(db.raw, sql.as_ptr(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut())
        };
        assert_eq!(rc, SQLITE_OK, "{:?}", self.error(&db, rc));
    }

    #[cfg(test)]
    pub fn with_exclusive_for_test(&self, path: &Path, action: impl FnOnce()) {
        let db = self.open(path, SQLITE_OPEN_READWRITE).expect("open for test");
        let sql = CString::new("BEGIN EXCLUSIVE").unwrap();
        // SAFETY: valid test connection, NUL-terminated SQL, no callback.
        let rc = unsafe {
            (self.api.exec)(db.raw, sql.as_ptr(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut())
        };
        assert_eq!(rc, SQLITE_OK, "{:?}", self.error(&db, rc));
        action(); // Dropping the connection rolls back and releases the lock.
    }
}

struct Connection<'a> {
    api: &'a Api,
    raw: *mut Db,
}

impl Drop for Connection<'_> {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            // SAFETY: closes the handle opened by `open`, once.
            unsafe { (self.api.close_v2)(self.raw) };
        }
    }
}

struct Statement<'a> {
    api: &'a Api,
    raw: *mut Stmt,
}

impl Statement<'_> {
    fn text(&self, column: c_int) -> String {
        // SAFETY: valid statement positioned on a row; the text pointer stays
        // valid until the next step, and `column_bytes` is its length.
        unsafe {
            let ptr = (self.api.column_text)(self.raw, column);
            let len = (self.api.column_bytes)(self.raw, column).max(0) as usize;
            if ptr.is_null() || len > MAX_VALUE_BYTES {
                return String::new();
            }
            String::from_utf8_lossy(std::slice::from_raw_parts(ptr, len)).into_owned()
        }
    }
}

impl Drop for Statement<'_> {
    fn drop(&mut self) {
        // SAFETY: finalizes the prepared statement once.
        unsafe { (self.api.finalize)(self.raw) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_rows_through_a_read_only_connection() {
        let Some(sqlite) = Sqlite::load() else {
            eprintln!("winsqlite3.dll is not available; skipping");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.db");
        sqlite.exec_for_test(
            &path,
            "PRAGMA journal_mode=WAL;
             CREATE TABLE auth_credentials (id INTEGER PRIMARY KEY, provider TEXT, credential_type TEXT, data TEXT,
                                            disabled_cause TEXT, updated_at INTEGER);
             INSERT INTO auth_credentials VALUES (1, 'zai', 'api_key', '{\"key\":\"old\"}', NULL, 10);
             INSERT INTO auth_credentials VALUES (2, 'zai', 'api_key', '{\"key\":\"new\"}', NULL, 20);
             INSERT INTO auth_credentials VALUES (3, 'zai', 'api_key', '{\"key\":\"gone\"}', 'removed', 30);
             INSERT INTO auth_credentials VALUES (4, 'kimi-code', 'oauth', NULL, NULL, 40);",
        );
        let sql = "SELECT credential_type, data FROM auth_credentials WHERE disabled_cause IS NULL AND provider = ?1
                   ORDER BY updated_at DESC, id DESC";
        let rows = sqlite.query(&path, sql, "zai", 2).unwrap();
        assert_eq!(
            rows,
            vec![
                vec!["api_key".to_owned(), "{\"key\":\"new\"}".to_owned()],
                vec!["api_key".to_owned(), "{\"key\":\"old\"}".to_owned()],
            ]
        );
        assert_eq!(sqlite.query(&path, sql, "kimi-code", 2).unwrap(), vec![vec!["oauth".to_owned(), String::new()]]);
        assert_eq!(sqlite.query(&path, sql, "nobody", 2).unwrap(), Vec::<Vec<String>>::new());
    }

    #[test]
    fn the_connection_cannot_write() {
        let Some(sqlite) = Sqlite::load() else { return };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.db");
        sqlite.exec_for_test(&path, "CREATE TABLE t (v TEXT);");
        let error = sqlite.query(&path, "INSERT INTO t VALUES (?1)", "x", 0).unwrap_err();
        assert!(matches!(error, SqliteError::Sqlite(8, _)), "SQLITE_READONLY expected, got {error:?}");
        assert!(sqlite.query(&path, "SELECT v FROM t WHERE v = ?1", "x", 1).unwrap().is_empty());
    }

    #[test]
    fn missing_files_and_tables_are_errors_not_panics() {
        let Some(sqlite) = Sqlite::load() else { return };
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(sqlite.query(&dir.path().join("none.db"), "SELECT 1", "", 1), Err(SqliteError::Missing));
        let path = dir.path().join("empty.db");
        sqlite.exec_for_test(&path, "CREATE TABLE other (v TEXT);");
        assert!(matches!(
            sqlite.query(&path, "SELECT v FROM credential WHERE v = ?1", "", 1),
            Err(SqliteError::Sqlite(1, _))
        ));
        let garbage = dir.path().join("garbage.db");
        std::fs::write(&garbage, b"this is not a database at all, not even close............").unwrap();
        assert!(matches!(sqlite.query(&garbage, "SELECT 1 WHERE ?1 = ''", "", 1), Err(SqliteError::Sqlite(26, _))));
    }
}

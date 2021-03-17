use crate::prelude::*;
use chashmap::CHashMap;
use lazy_static::lazy_static;
use rusqlite::*;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

mod singleton_conn;

const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(30);

lazy_static! {

    pub(crate) static ref DATABASE_HANDLES: CHashMap<PathBuf, DbWrite> = {
        // This is just a convenient place that we know gets initialized
        // both in the final binary holochain && in all relevant tests
        //
        // Holochain (and most binaries) are left in invalid states
        // if a thread panic!s - switch to failing fast in that case.
        //
        // We tried putting `panic = "abort"` in the Cargo.toml,
        // but somehow that breaks the wasmer / test_utils integration.

        let orig_handler = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            // print the panic message
            eprintln!("FATAL PANIC {:#?}", panic_info);
            // invoke the original handler
            orig_handler(panic_info);
            // // Abort the process
            // // TODO - we need a better solution than this, but if there is
            // // no better solution, we can uncomment the following line:
            // std::process::abort();
        }));

        CHashMap::new()
    };
}

pub type ConnectionPool = r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>;
pub type PConnInner = r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>;

pub(crate) fn new_connection_pool(path: &Path, kind: DbKind) -> ConnectionPool {
    use r2d2_sqlite::SqliteConnectionManager;
    let manager = SqliteConnectionManager::file(path);
    let customizer = Box::new(ConnCustomizer { kind });
    r2d2::Pool::builder()
        .max_size(20)
        .connection_customizer(customizer)
        .build(manager)
        .unwrap()
}

#[derive(Debug)]
struct ConnCustomizer {
    // path: PathBuf,
    kind: DbKind,
}

impl r2d2::CustomizeConnection<Connection, rusqlite::Error> for ConnCustomizer {
    fn on_acquire(&self, conn: &mut Connection) -> Result<(), rusqlite::Error> {
        initialize_connection(conn, &self.kind, true)?;
        Ok(())
    }
}

fn initialize_connection(
    conn: &mut Connection,
    _kind: &DbKind,
    _is_first: bool,
) -> rusqlite::Result<()> {
    // tell SQLite to wait this long during write contention
    conn.busy_timeout(SQLITE_BUSY_TIMEOUT)?;

    #[cfg(feature = "db-encryption")]
    {
        use std::io::Write;
        let key = get_encryption_key_shim();
        let mut cmd =
            *br#"PRAGMA key = "x'0000000000000000000000000000000000000000000000000000000000000000'";"#;
        let mut c = std::io::Cursor::new(&mut cmd[16..80]);
        for b in &key {
            write!(c, "{:02X}", b)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        }
        conn.execute(std::str::from_utf8(&cmd).unwrap(), NO_PARAMS)?;
    }

    // set to faster write-ahead-log mode
    conn.pragma_update(None, "journal_mode", &"WAL".to_string())?;

    Ok(())
}

#[cfg(feature = "db-encryption")]
/// Simulate getting an encryption key from Lair.
fn get_encryption_key_shim() -> [u8; 32] {
    [
        26, 111, 7, 31, 52, 204, 156, 103, 203, 171, 156, 89, 98, 51, 158, 143, 57, 134, 93, 56,
        199, 225, 53, 141, 39, 77, 145, 130, 136, 108, 96, 201,
    ]
}

/// Singleton Connection.
#[derive(shrinkwraprs::Shrinkwrap)]
#[shrinkwrap(mutable, unsafe_ignore_visibility)]
pub struct PConn {
    #[shrinkwrap(main_field)]
    inner: PConnInner,
    _kind: DbKind,
}

impl PConn {
    pub(crate) fn new(inner: PConnInner, _kind: DbKind) -> Self {
        Self { inner, _kind }
    }

    #[cfg(feature = "test_utils")]
    pub fn open_single(&mut self, name: &str) -> Result<SingleTable, DatabaseError> {
        crate::table::initialize_table_single(&mut self.inner, name.to_string(), name.to_string())?;
        Ok(Table {
            name: TableName::TestSingle(name.to_string()),
        })
    }

    #[cfg(feature = "test_utils")]
    pub fn open_integer(&mut self, name: &str) -> Result<IntegerTable, DatabaseError> {
        self.open_single(name)
    }

    #[cfg(feature = "test_utils")]
    pub fn open_multi(&mut self, name: &str) -> Result<MultiTable, DatabaseError> {
        crate::table::initialize_table_multi(&mut self.inner, name.to_string(), name.to_string())?;
        Ok(Table {
            name: TableName::TestMulti(name.to_string()),
        })
    }
}
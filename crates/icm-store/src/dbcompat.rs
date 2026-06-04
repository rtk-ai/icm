//! dbcompat — a synchronous, **rusqlite-0.34-shaped** facade over the async
//! `libsql` client. It mirrors the exact rusqlite API surface `store.rs` /
//! `schema.rs` use (`Connection`, `Statement`, `Row<'_>`, `params!`, `ToSql`,
//! `types::ToSql`, `OptionalExtension`, `MappedRows`, `Error`, the `[]` / `[T;N]`
//! / `&[&dyn ToSql]` `Params` forms) so those files stay byte-for-byte upstream
//! except for one `use crate::dbcompat as rusqlite;` alias and the connection-open
//! path. The real database can be a local file, a remote libSQL/Turso server, or
//! an embedded replica — see `Connection`.
//!
//! Keeping the store code unchanged is deliberate: `store.rs` is upstream's most
//! actively edited file, so a minimal diff means near-conflict-free rebases.

use std::marker::PhantomData;
use std::path::Path;
use std::sync::Arc;

use libsql::Builder;
use once_cell::sync::Lazy;

pub use libsql::Value;

// ───────────────────────────── runtime ─────────────────────────────

static RT: Lazy<tokio::runtime::Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("BUG: failed to build dbcompat tokio runtime")
});

fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
        Err(_) => RT.block_on(fut),
    }
}

// ───────────────────────────── errors ─────────────────────────────

#[derive(Debug)]
pub enum Error {
    QueryReturnedNoRows,
    FromSqlConversion(String),
    Libsql(libsql::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::QueryReturnedNoRows => write!(f, "Query returned no rows"),
            Error::FromSqlConversion(s) => write!(f, "from-sql conversion error: {s}"),
            Error::Libsql(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<libsql::Error> for Error {
    fn from(e: libsql::Error) -> Self {
        Error::Libsql(e)
    }
}

// ───────────────────────────── ToSql ─────────────────────────────

/// Object-safe value conversion (mirrors `rusqlite::ToSql` / `rusqlite::types::ToSql`).
pub trait ToSql {
    fn to_value(&self) -> Value;
}

macro_rules! to_sql_int {
    ($($t:ty),*) => {$( impl ToSql for $t {
        fn to_value(&self) -> Value { Value::Integer(*self as i64) }
    } )*};
}
to_sql_int!(i8, i16, i32, i64, u8, u16, u32, u64, usize, isize);

impl ToSql for f32 {
    fn to_value(&self) -> Value { Value::Real(*self as f64) }
}
impl ToSql for f64 {
    fn to_value(&self) -> Value { Value::Real(*self) }
}
impl ToSql for bool {
    fn to_value(&self) -> Value { Value::Integer(if *self { 1 } else { 0 }) }
}
impl ToSql for String {
    fn to_value(&self) -> Value { Value::Text(self.clone()) }
}
impl ToSql for str {
    fn to_value(&self) -> Value { Value::Text(self.to_string()) }
}
impl ToSql for Vec<u8> {
    fn to_value(&self) -> Value { Value::Blob(self.clone()) }
}
impl ToSql for [u8] {
    fn to_value(&self) -> Value { Value::Blob(self.to_vec()) }
}
impl ToSql for Value {
    fn to_value(&self) -> Value { self.clone() }
}
impl<T: ToSql + ?Sized> ToSql for &T {
    fn to_value(&self) -> Value { (**self).to_value() }
}
impl<T: ToSql + ?Sized> ToSql for Box<T> {
    fn to_value(&self) -> Value { (**self).to_value() }
}
impl<T: ToSql> ToSql for Option<T> {
    fn to_value(&self) -> Value {
        match self {
            Some(v) => v.to_value(),
            None => Value::Null,
        }
    }
}

/// `rusqlite::types::ToSql` lives under a `types` module upstream.
pub mod types {
    pub use super::ToSql;
}

// ───────────────────────────── Params ─────────────────────────────
//
// Mirrors rusqlite 0.34's Params impls so both `[]` (empty) and `[x]`
// (non-empty) and `params![..]` (-> `&[&dyn ToSql]`) work unchanged. The empty
// array has its own `[&dyn …; 0]` impl, which forces a per-N macro for the
// non-empty `[T; N]` case (a generic `[T; N]` would clash on N=0).

pub trait Params {
    fn into_values(self) -> Vec<Value>;
}

impl Params for () {
    fn into_values(self) -> Vec<Value> { Vec::new() }
}
// bare `[]` — the only [_; 0] impl (so it's unambiguous), like rusqlite.
impl Params for [&(dyn ToSql + Send + Sync); 0] {
    fn into_values(self) -> Vec<Value> { Vec::new() }
}
impl Params for &[&dyn ToSql] {
    fn into_values(self) -> Vec<Value> { self.iter().map(|v| v.to_value()).collect() }
}
impl Params for &Vec<&dyn ToSql> {
    fn into_values(self) -> Vec<Value> { self.iter().map(|v| v.to_value()).collect() }
}
impl Params for &[Box<dyn ToSql>] {
    fn into_values(self) -> Vec<Value> { self.iter().map(|v| v.to_value()).collect() }
}
impl Params for &Vec<Box<dyn ToSql>> {
    fn into_values(self) -> Vec<Value> { self.iter().map(|v| v.to_value()).collect() }
}
macro_rules! array_params {
    ($($N:literal)+) => {$(
        impl<T: ToSql> Params for [T; $N] {
            fn into_values(self) -> Vec<Value> { self.iter().map(ToSql::to_value).collect() }
        }
        impl<T: ToSql + ?Sized> Params for &[&T; $N] {
            fn into_values(self) -> Vec<Value> { self.iter().map(|v| v.to_value()).collect() }
        }
    )+};
}
array_params!(1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32);

/// `params![a, b]` → `&[&dyn ToSql]`, matching `rusqlite::params!`.
#[macro_export]
macro_rules! dbcompat_params {
    () => { (&[] as &[&dyn $crate::dbcompat::ToSql]) };
    ($($x:expr),+ $(,)?) => {
        (&[$(&($x) as &dyn $crate::dbcompat::ToSql),+] as &[&dyn $crate::dbcompat::ToSql])
    };
}
pub use crate::dbcompat_params as params;

// ───────────────────────────── FromSql / Row ─────────────────────────────

pub trait FromSql: Sized {
    fn from_value(v: Value) -> Result<Self>;
}

fn conv(what: &str, v: &Value) -> Error {
    Error::FromSqlConversion(format!("cannot read {what} from {v:?}"))
}

macro_rules! from_sql_int {
    ($($t:ty),*) => {$( impl FromSql for $t {
        fn from_value(v: Value) -> Result<Self> {
            match v {
                Value::Integer(i) => Ok(i as $t),
                Value::Real(r) => Ok(r as $t),
                ref o => Err(conv(stringify!($t), o)),
            }
        }
    } )*};
}
from_sql_int!(i8, i16, i32, i64, u8, u16, u32, u64, usize, isize);

impl FromSql for f64 {
    fn from_value(v: Value) -> Result<Self> {
        match v {
            Value::Real(r) => Ok(r),
            Value::Integer(i) => Ok(i as f64),
            ref o => Err(conv("f64", o)),
        }
    }
}
impl FromSql for f32 {
    fn from_value(v: Value) -> Result<Self> { f64::from_value(v).map(|r| r as f32) }
}
impl FromSql for bool {
    fn from_value(v: Value) -> Result<Self> {
        match v {
            Value::Integer(i) => Ok(i != 0),
            ref o => Err(conv("bool", o)),
        }
    }
}
impl FromSql for String {
    fn from_value(v: Value) -> Result<Self> {
        match v {
            Value::Text(s) => Ok(s),
            Value::Blob(b) => String::from_utf8(b).map_err(|e| Error::FromSqlConversion(e.to_string())),
            ref o => Err(conv("String", o)),
        }
    }
}
impl FromSql for Vec<u8> {
    fn from_value(v: Value) -> Result<Self> {
        match v {
            Value::Blob(b) => Ok(b),
            Value::Text(s) => Ok(s.into_bytes()),
            ref o => Err(conv("Vec<u8>", o)),
        }
    }
}
impl FromSql for Value {
    fn from_value(v: Value) -> Result<Self> { Ok(v) }
}
impl<T: FromSql> FromSql for Option<T> {
    fn from_value(v: Value) -> Result<Self> {
        match v {
            Value::Null => Ok(None),
            other => T::from_value(other).map(Some),
        }
    }
}

pub trait RowIndex {
    fn idx(&self, row: &Row) -> Result<usize>;
}
impl RowIndex for usize {
    fn idx(&self, _row: &Row) -> Result<usize> { Ok(*self) }
}
impl RowIndex for &str {
    fn idx(&self, row: &Row) -> Result<usize> {
        row.names
            .iter()
            .position(|n| n == self)
            .ok_or_else(|| Error::FromSqlConversion(format!("no such column: {self}")))
    }
}

/// A fully-materialised result row. The lifetime is phantom (values are owned),
/// kept only so upstream signatures like `&Row<'_>` compile unchanged.
pub struct Row<'a> {
    names: Arc<Vec<String>>,
    values: Vec<Value>,
    _marker: PhantomData<&'a ()>,
}

impl<'a> Row<'a> {
    pub fn get<I: RowIndex, T: FromSql>(&self, idx: I) -> Result<T> {
        let i = idx.idx(self)?;
        let v = self
            .values
            .get(i)
            .cloned()
            .ok_or_else(|| Error::FromSqlConversion(format!("column index {i} out of range")))?;
        T::from_value(v)
    }
}

async fn materialize(rows: &mut libsql::Rows) -> Result<Vec<Row<'static>>> {
    let ncols = rows.column_count();
    let names: Arc<Vec<String>> = Arc::new(
        (0..ncols)
            .map(|i| rows.column_name(i).unwrap_or("").to_string())
            .collect(),
    );
    let mut out = Vec::new();
    while let Some(r) = rows.next().await? {
        let mut values = Vec::with_capacity(ncols as usize);
        for i in 0..ncols {
            values.push(r.get_value(i)?);
        }
        out.push(Row {
            names: names.clone(),
            values,
            _marker: PhantomData,
        });
    }
    Ok(out)
}

pub type MappedRows<T> = std::vec::IntoIter<Result<T>>;

// ───────────────────────────── Statement ─────────────────────────────

pub struct Statement {
    conn: Arc<libsql::Connection>,
    sql: String,
}

impl Statement {
    pub fn query_map<T, F>(&mut self, params: impl Params, mut f: F) -> Result<MappedRows<T>>
    where
        F: FnMut(&Row) -> Result<T>,
    {
        let vals = params.into_values();
        let rows = block_on(async {
            let mut rows = self.conn.query(&self.sql, vals).await?;
            materialize(&mut rows).await
        })?;
        let mapped: Vec<Result<T>> = rows.iter().map(|r| f(r)).collect();
        Ok(mapped.into_iter())
    }

    pub fn query_row<T, F>(&mut self, params: impl Params, f: F) -> Result<T>
    where
        F: FnOnce(&Row) -> Result<T>,
    {
        let vals = params.into_values();
        let rows = block_on(async {
            let mut rows = self.conn.query(&self.sql, vals).await?;
            materialize(&mut rows).await
        })?;
        match rows.first() {
            Some(r) => f(r),
            None => Err(Error::QueryReturnedNoRows),
        }
    }

    pub fn execute(&mut self, params: impl Params) -> Result<usize> {
        let vals = params.into_values();
        Ok(block_on(self.conn.execute(&self.sql, vals))? as usize)
    }
}

// ───────────────────────────── Connection ─────────────────────────────

#[derive(Clone)]
pub struct Connection {
    db: Arc<libsql::Database>,
    conn: Arc<libsql::Connection>,
    remote: bool,
}

impl Connection {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let db = block_on(async { Builder::new_local(path.as_ref().to_path_buf()).build().await })?;
        // First connect runs libsql's one-time threading config + sqlite init;
        // only AFTER that can sqlite-vec be registered (auto_extension itself
        // initialises SQLite, which would otherwise race libsql's config).
        let _warmup = db.connect()?;
        register_vec_after_init();
        let conn = db.connect()?;
        Ok(Self { db: Arc::new(db), conn: Arc::new(conn), remote: false })
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::open(":memory:")
    }

    /// Remote libSQL/Turso server: every process shares it → concurrent-safe writes.
    pub fn open_remote(url: String, auth_token: String) -> Result<Self> {
        let db = block_on(async { Builder::new_remote(url, auth_token).build().await })?;
        let conn = db.connect()?;
        Ok(Self { db: Arc::new(db), conn: Arc::new(conn), remote: true })
    }

    /// Local embedded replica that syncs to a remote primary.
    pub fn open_replica<P: AsRef<Path>>(path: P, url: String, auth_token: String) -> Result<Self> {
        let db = block_on(async {
            Builder::new_remote_replica(path.as_ref().to_path_buf(), url, auth_token).build().await
        })?;
        let _warmup = db.connect()?;
        register_vec_after_init();
        let conn = db.connect()?;
        Ok(Self { db: Arc::new(db), conn: Arc::new(conn), remote: false })
    }

    pub fn is_remote(&self) -> bool {
        self.remote
    }

    pub fn sync(&self) -> Result<()> {
        block_on(async { self.db.sync().await })?;
        Ok(())
    }

    pub fn execute(&self, sql: &str, params: impl Params) -> Result<usize> {
        let vals = params.into_values();
        Ok(block_on(self.conn.execute(sql, vals))? as usize)
    }

    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        block_on(async { self.conn.execute_batch(sql).await })?;
        Ok(())
    }

    pub fn prepare(&self, sql: &str) -> Result<Statement> {
        Ok(Statement { conn: self.conn.clone(), sql: sql.to_string() })
    }

    pub fn query_row<T, F>(&self, sql: &str, params: impl Params, f: F) -> Result<T>
    where
        F: FnOnce(&Row) -> Result<T>,
    {
        self.prepare(sql)?.query_row(params, f)
    }

    pub fn last_insert_rowid(&self) -> i64 {
        self.conn.last_insert_rowid()
    }

    /// Mirrors `rusqlite::Connection::unchecked_transaction` (rolls back on drop
    /// unless committed).
    pub fn unchecked_transaction(&self) -> Result<Transaction<'_>> {
        self.execute_batch("BEGIN")?;
        Ok(Transaction { conn: self, done: false })
    }
}

pub struct Transaction<'a> {
    conn: &'a Connection,
    done: bool,
}
impl<'a> Transaction<'a> {
    pub fn commit(mut self) -> Result<()> {
        self.conn.execute_batch("COMMIT")?;
        self.done = true;
        Ok(())
    }
}
impl<'a> std::ops::Deref for Transaction<'a> {
    type Target = Connection;
    fn deref(&self) -> &Connection { self.conn }
}
impl<'a> Drop for Transaction<'a> {
    fn drop(&mut self) {
        if !self.done {
            let _ = self.conn.execute_batch("ROLLBACK");
        }
    }
}

// ───────────────────────────── OptionalExtension ─────────────────────────────

pub trait OptionalExtension<T> {
    fn optional(self) -> Result<Option<T>>;
}
impl<T> OptionalExtension<T> for Result<T> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(t) => Ok(Some(t)),
            Err(Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

// ───────────────────────────── sqlite-vec ─────────────────────────────

/// Register sqlite-vec for the LOCAL backend. Must run AFTER libsql's first
/// `connect()` (which configures + initialises SQLite); `Connection::open`
/// arranges that via a warmup connect. No-op-safe to call repeatedly. On the
/// remote backend, vec lives on the server (`sqld --extensions-path`).
pub fn register_vec_after_init() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        #[allow(clippy::missing_transmute_annotations)]
        libsql_ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

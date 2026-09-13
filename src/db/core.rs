use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags};

use super::schema::{legacy_prerelease_schema_detected, prepare_schema, CURRENT_SCHEMA_VERSION};
use super::sqlite_helpers::row_usize;
use super::store::revision::bump_revision;
use super::types::{
    ArchiveChangeKind, Database, StorageCheckpointReport, StorageCompactReport, StorageFileSizes,
};

fn validate_initialization_target(conn: &Connection) -> Result<()> {
    let application_id: i64 = conn.query_row("PRAGMA application_id", [], |row| row.get(0))?;
    if application_id != 0 && application_id != CLIPMEM_APPLICATION_ID {
        bail!("refusing to initialize a database owned by another application");
    }
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > CURRENT_SCHEMA_VERSION {
        bail!("database schema version {version} is newer than supported version {CURRENT_SCHEMA_VERSION}");
    }
    let mut statement = conn.prepare(
        "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if names.is_empty() {
        return Ok(());
    }
    if application_id == CLIPMEM_APPLICATION_ID {
        return Ok(());
    }
    let known_tables = names.iter().all(|name| {
        super::schema::SCHEMA.contains(&format!("CREATE TABLE IF NOT EXISTS {name} ("))
            || [
                "snapshots_fts",
                "snapshots_literal_fts",
                "snapshot_file_url_fts",
                "snapshot_search_documents_fts",
                "snapshot_search_documents_literal_fts",
                "snapshot_ocr_fts",
                "snapshot_ocr_literal_fts",
            ]
            .iter()
            .any(|prefix| name == prefix || name.starts_with(&format!("{prefix}_")))
    });
    let legacy_shape = conn.prepare("SELECT id, sha256, snapshot_kind, item_count FROM snapshots LIMIT 0").is_ok()
        || (version > 0 && (conn.prepare("SELECT id, paused, retention_seconds FROM clipmem_settings LIMIT 0").is_ok()
            || conn.prepare("SELECT snapshot_id, item_index, uti, kind, byte_len, raw_sha256, blob_value FROM item_representations LIMIT 0").is_ok()));
    if !known_tables || !legacy_shape {
        bail!("refusing to initialize an unrecognized populated database; choose an empty file or an existing clipmem archive");
    }
    Ok(())
}

impl Database {
    /// Open the archive database at `path`, creating parent directories and schema state as needed.
    ///
    /// This method also hardens parent-directory, database-file, and SQLite sidecar permissions
    /// after opening.
    ///
    /// # Errors
    ///
    /// Returns an error if the parent directory cannot be created, the database cannot be opened,
    /// or the connection cannot be configured and bootstrapped.
    pub fn open_or_init_and_migrate(path: &Path) -> Result<Self> {
        if path.exists() {
            let existing = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            validate_initialization_target(&existing)?;
        }
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            let existed = parent.exists();
            Context::with_context(std::fs::create_dir_all(parent), || {
                format!("failed to create {}", parent.display())
            })?;
            if !existed {
                harden_path_permissions(parent, 0o700)?;
            }
        }

        let mut conn = open_connection(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        prepare_connection(&mut conn)?;
        harden_path_permissions(path, 0o600)?;
        harden_existing_sqlite_sidecar_permissions(path)?;

        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    /// Open a current clipmem archive through a truly read-only SQLite connection.
    ///
    /// This path never prepares or migrates schema and never changes filesystem permissions.
    pub fn open_read_only_current(
        path: &Path,
    ) -> std::result::Result<Self, super::types::DatabaseOpenError> {
        ensure_existing_file(path)?;
        let conn = open_current_read_only_connection(path)?;
        configure_current_connection(&conn, true)?;
        validate_current_archive(&conn, path)?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    /// Open a current clipmem archive for mutation without creating or migrating schema.
    pub fn open_read_write_current(
        path: &Path,
    ) -> std::result::Result<Self, super::types::DatabaseOpenError> {
        ensure_existing_file(path)?;
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(super::types::DatabaseOpenError::Sqlite)?;
        configure_current_connection(&conn, false)?;
        validate_current_archive(&conn, path)?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    /// Compatibility wrapper retaining the historical create-and-migrate behavior.
    pub fn open_or_init(path: &Path) -> Result<Self> {
        Self::open_or_init_and_migrate(path)
    }

    /// Deprecated compatibility wrapper for a read-write current-schema open.
    ///
    /// This method does not create, migrate, run DDL, or change filesystem permissions.
    ///
    /// # Errors
    ///
    /// Returns a typed current-schema open error wrapped in `anyhow::Error`.
    #[deprecated(note = "use open_read_only_current or open_read_write_current")]
    pub fn open_existing(path: &Path) -> Result<Self> {
        Self::open_read_write_current(path).map_err(anyhow::Error::from)
    }

    pub(crate) fn ensure_supported_schema_shape(&self) -> Result<()> {
        if legacy_prerelease_schema_detected(&self.conn)?
            || self
                .conn
                .prepare("SELECT kind FROM item_representations LIMIT 0")
                .is_err_and(|error| error.to_string().contains("no such column: kind"))
        {
            bail!(
                "database operation failed; this may be an incompatible prerelease schema. Move the database aside and run `clipmem setup`."
            );
        }
        Ok(())
    }

    #[must_use]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn compact_storage(&mut self, dry_run: bool) -> Result<StorageCompactReport> {
        let before = storage_file_sizes(&self.path)?;
        let total_before_bytes = before.total_bytes();

        let checkpoint = if dry_run {
            run_wal_checkpoint(&self.conn, "PASSIVE")?
        } else {
            let _ = run_wal_checkpoint(&self.conn, "TRUNCATE")?;
            self.conn
                .execute_batch("VACUUM")
                .context("vacuum database")?;
            self.conn
                .execute_batch("PRAGMA optimize")
                .context("optimize database")?;
            run_wal_checkpoint(&self.conn, "TRUNCATE")?
        };

        if !dry_run {
            bump_revision(&self.conn, &[ArchiveChangeKind::Storage])?;
        }

        let page_count = pragma_usize(&self.conn, "page_count")?;
        let freelist_count = pragma_usize(&self.conn, "freelist_count")?;
        let page_size = pragma_usize(&self.conn, "page_size")?;
        let after = storage_file_sizes(&self.path)?;
        let total_after_bytes = after.total_bytes();

        Ok(StorageCompactReport {
            db_path: self.path.display().to_string(),
            before,
            after,
            total_before_bytes,
            total_after_bytes,
            reclaimed_bytes: total_before_bytes.saturating_sub(total_after_bytes),
            estimated_reclaimable_bytes: (page_size as u64).saturating_mul(freelist_count as u64),
            page_count,
            freelist_count,
            checkpoint,
            dry_run,
            completed: !dry_run,
        })
    }

    #[cfg(test)]
    pub(crate) fn open_in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        prepare_connection(&mut conn)?;
        Ok(Self {
            conn,
            path: PathBuf::from(":memory:"),
        })
    }
}

fn open_current_read_only_connection(
    path: &Path,
) -> std::result::Result<Connection, super::types::DatabaseOpenError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_URI
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    // An open/probe failure can mean a live writer holds an exclusive lock.
    // Never bypass SQLite locking or WAL visibility with immutable=1.
    let conn = Connection::open_with_flags(path, flags)
        .map_err(super::types::DatabaseOpenError::Sqlite)?;
    conn.query_row("PRAGMA schema_version", [], |row| row.get::<_, i64>(0))
        .map_err(super::types::DatabaseOpenError::Sqlite)?;
    Ok(conn)
}

const CLIPMEM_APPLICATION_ID: i64 = 1_129_137_485;

fn ensure_existing_file(path: &Path) -> std::result::Result<(), super::types::DatabaseOpenError> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(super::types::DatabaseOpenError::Missing(path.to_path_buf())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(super::types::DatabaseOpenError::Missing(path.to_path_buf()))
        }
        Err(error) => Err(super::types::DatabaseOpenError::Io(error)),
    }
}

fn configure_current_connection(
    conn: &Connection,
    read_only: bool,
) -> std::result::Result<(), super::types::DatabaseOpenError> {
    conn.busy_timeout(if read_only {
        Duration::from_millis(250)
    } else {
        Duration::from_millis(1_500)
    })
    .map_err(super::types::DatabaseOpenError::Sqlite)?;
    conn.execute_batch(if read_only {
        // The READ_ONLY open flag protects the main archive while TEMP remains available to
        // doctor/stats queries that legitimately create connection-local working tables.
        "PRAGMA foreign_keys = ON; PRAGMA temp_store = MEMORY;"
    } else {
        "PRAGMA foreign_keys = ON; PRAGMA temp_store = MEMORY; PRAGMA synchronous = NORMAL; PRAGMA secure_delete = ON;"
    })
    .map_err(super::types::DatabaseOpenError::Sqlite)
}

fn validate_current_archive(
    conn: &Connection,
    path: &Path,
) -> std::result::Result<(), super::types::DatabaseOpenError> {
    use super::types::DatabaseOpenError;

    let core_table_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name IN ('snapshots', 'snapshot_items', 'item_representations', 'capture_events')",
            [],
            |row| row.get(0),
        )
        .map_err(DatabaseOpenError::Sqlite)?;
    if core_table_count != 4 {
        return Err(DatabaseOpenError::NotArchive(path.to_path_buf()));
    }
    if legacy_prerelease_schema_detected(conn)
        .map_err(|error| DatabaseOpenError::InvalidSchema(error.to_string()))?
    {
        return Err(DatabaseOpenError::InvalidSchema(
            "incompatible prerelease schema; move it aside and run `clipmem setup`".to_string(),
        ));
    }

    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(DatabaseOpenError::Sqlite)?;
    if version < CURRENT_SCHEMA_VERSION {
        return Err(DatabaseOpenError::MigrationRequired {
            found: version,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }
    if version > CURRENT_SCHEMA_VERSION {
        return Err(DatabaseOpenError::NewerSchema {
            found: version,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }

    let application_id: i64 = conn
        .query_row("PRAGMA application_id", [], |row| row.get(0))
        .map_err(DatabaseOpenError::Sqlite)?;
    if application_id != CLIPMEM_APPLICATION_ID {
        return Err(DatabaseOpenError::NotArchive(path.to_path_buf()));
    }

    let metadata = conn.query_row(
        "SELECT archive_magic, archive_uuid, projection_version FROM archive_metadata WHERE id = 1",
        [],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        },
    );
    match metadata {
        Ok((magic, uuid, projection_version))
            if magic == "clipmem-archive" && uuid.len() == 32 && projection_version >= 0 => {}
        Ok(_) => {
            return Err(DatabaseOpenError::InvalidSchema(
                "invalid archive metadata row".to_string(),
            ))
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            return Err(DatabaseOpenError::InvalidSchema(
                "archive metadata row is missing".to_string(),
            ));
        }
        Err(error) => return Err(DatabaseOpenError::Sqlite(error)),
    }

    conn.prepare("SELECT item_index, primary_kind FROM snapshot_items LIMIT 0")
        .and_then(|_| {
            conn.prepare("SELECT uti, kind, byte_len, raw_sha256 FROM item_representations LIMIT 0")
        })
        .map_err(|error| DatabaseOpenError::InvalidSchema(error.to_string()))?;
    Ok(())
}

impl StorageFileSizes {
    #[must_use]
    pub(crate) const fn total_bytes(&self) -> u64 {
        self.db + self.wal + self.shm
    }
}

pub(in crate::db) fn storage_file_sizes(path: &Path) -> Result<StorageFileSizes> {
    Ok(StorageFileSizes {
        db: metadata_len(path)?,
        wal: metadata_len(&sidecar_path(path, "-wal"))?,
        shm: metadata_len(&sidecar_path(path, "-shm"))?,
    })
}

pub(in crate::db) fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", path.display()))
}

pub(in crate::db) fn harden_existing_sqlite_sidecar_permissions(path: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm"] {
        let sidecar = sidecar_path(path, suffix);
        match std::fs::metadata(&sidecar) {
            Ok(_) => harden_path_permissions(&sidecar, 0o600)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("read metadata for {}", sidecar.display()));
            }
        }
    }
    Ok(())
}

pub(in crate::db) fn metadata_len(path: &Path) -> Result<u64> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error).with_context(|| format!("read size of {}", path.display())),
    }
}

pub(in crate::db) fn pragma_usize(conn: &Connection, pragma: &str) -> Result<usize> {
    let sql = format!("PRAGMA {pragma}");
    conn.query_row(&sql, [], |row| row_usize(row, 0))
        .with_context(|| format!("read PRAGMA {pragma}"))
}

pub(in crate::db) fn run_wal_checkpoint(
    conn: &Connection,
    mode: &str,
) -> Result<StorageCheckpointReport> {
    let sql = format!("PRAGMA wal_checkpoint({mode})");
    conn.query_row(&sql, [], |row| {
        Ok(StorageCheckpointReport {
            busy: row.get(0)?,
            log: row.get(1)?,
            checkpointed: row.get(2)?,
        })
    })
    .with_context(|| format!("run WAL checkpoint {mode}"))
}

pub(in crate::db) fn clamp_result_limit(limit: usize) -> usize {
    limit.clamp(1, 250)
}

pub(in crate::db) fn open_connection(path: &Path, flags: OpenFlags) -> Result<Connection> {
    anyhow::Context::with_context(Connection::open_with_flags(path, flags), || {
        format!("failed to open {}", path.display())
    })
}

pub(in crate::db) fn prepare_connection(conn: &mut Connection) -> Result<()> {
    Context::context(configure_connection(conn), "configure database connection")?;
    Context::context(prepare_schema(conn), "prepare database schema")?;
    Ok(())
}

#[cfg(unix)]
pub(in crate::db) fn harden_path_permissions(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    anyhow::Context::with_context(
        std::fs::set_permissions(path, PermissionsExt::from_mode(mode)),
        || format!("failed to set permissions on {}", path.display()),
    )
}

#[cfg(not(unix))]
pub(in crate::db) fn harden_path_permissions(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

pub(in crate::db) fn configure_connection(conn: &Connection) -> Result<()> {
    configure_pragma(conn, "journal_mode", "WAL")?;
    configure_pragma(conn, "synchronous", "NORMAL")?;
    configure_pragma(conn, "foreign_keys", "ON")?;
    configure_pragma(conn, "secure_delete", "ON")?;
    configure_pragma(conn, "temp_store", "MEMORY")?;
    conn.busy_timeout(Duration::from_millis(1_500))
        .context("configure SQLite busy timeout")?;
    Ok(())
}

pub(in crate::db) fn configure_pragma(conn: &Connection, pragma: &str, value: &str) -> Result<()> {
    Context::with_context(conn.pragma_update(None, pragma, value), || {
        format!("configure {pragma} pragma")
    })?;
    Ok(())
}

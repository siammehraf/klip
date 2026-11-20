use rusqlite::{params, Connection};
use std::fs;
use std::path::{Path, PathBuf};

pub struct StoragePaths {
    pub app_dir: PathBuf,
    pub db_path: PathBuf,
    pub images_dir: PathBuf,
}

pub fn resolve_storage_paths() -> Result<StoragePaths, String> {
    let base_data = dirs::data_dir()
        .ok_or_else(|| "Unable to locate Windows AppData Roaming directory".to_string())?;
    let app_dir = base_data.join("Klip");
    let images_dir = app_dir.join("images");
    let db_path = app_dir.join("klip.db");

    fs::create_dir_all(&app_dir).map_err(|e| format!("Failed creating Klip dir: {e}"))?;
    fs::create_dir_all(&images_dir).map_err(|e| format!("Failed creating images dir: {e}"))?;

    Ok(StoragePaths {
        app_dir,
        db_path,
        images_dir,
    })
}

pub fn open_and_init(db_path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(db_path).map_err(|e| format!("Failed opening SQLite DB: {e}"))?;

    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA foreign_keys = ON;
        PRAGMA synchronous = NORMAL;

        CREATE TABLE IF NOT EXISTS clips (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            clip_type TEXT NOT NULL,
            text_content TEXT,
            html_content TEXT,
            image_path TEXT,
            content_size INTEGER NOT NULL DEFAULT 0,
            source_app TEXT,
            source_icon_path TEXT,
            is_pinned INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_clips_created_at ON clips(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_clips_pinned ON clips(is_pinned, created_at DESC);

        CREATE TABLE IF NOT EXISTS tags (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            color TEXT NOT NULL DEFAULT '#64748b',
            created_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS clip_tags (
            clip_id INTEGER NOT NULL REFERENCES clips(id) ON DELETE CASCADE,
            tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
            PRIMARY KEY (clip_id, tag_id)
        );

        CREATE TABLE IF NOT EXISTS snippets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            title TEXT NOT NULL,
            content TEXT NOT NULL,
            shortcut TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
    )
    .map_err(|e| format!("Failed initializing schema: {e}"))?;

    let defaults = [
        ("hotkey", "Ctrl+Shift+Backquote"),
        ("theme", "dark"),
        ("font_family", "Segoe UI, Nirmala UI, sans-serif"),
        ("paste_mode", "auto_paste"),
        ("autostart", "true"),
        ("retention_days", "30"),
        ("max_clip_size_mb", "10"),
        ("excluded_apps", "[]"),
        ("history_locked", "false"),
        ("paused", "false"),
        ("backup_folder", ""),
        ("last_backup_prompt_date", ""),
    ];

    for (key, value) in defaults {
        conn.execute(
            "INSERT OR IGNORE INTO settings (key, value) VALUES (?1, ?2)",
            params![key, value],
        )
        .map_err(|e| format!("Failed seeding setting {key}: {e}"))?;
    }

    Ok(conn)
}

pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>, String> {
    let mut stmt = conn
        .prepare("SELECT value FROM settings WHERE key = ?1")
        .map_err(|e| format!("Failed preparing get_setting: {e}"))?;

    let mut rows = stmt
        .query(params![key])
        .map_err(|e| format!("Failed querying setting {key}: {e}"))?;

    if let Some(row) = rows
        .next()
        .map_err(|e| format!("Failed reading setting {key}: {e}"))?
    {
        let val: String = row
            .get(0)
            .map_err(|e| format!("Failed decoding setting {key}: {e}"))?;
        Ok(Some(val))
    } else {
        Ok(None)
    }
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .map_err(|e| format!("Failed saving setting {key}: {e}"))?;
    Ok(())
}

pub fn list_tables(conn: &Connection) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .map_err(|e| format!("Failed listing tables: {e}"))?;

    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("Failed querying tables: {e}"))?;

    let mut tables = Vec::new();
    for item in rows {
        tables.push(item.map_err(|e| format!("Failed reading table name: {e}"))?);
    }
    Ok(tables)
}
use rusqlite::{params, params_from_iter, types::Value, Connection};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct StoragePaths {
    pub app_dir: PathBuf,
    pub db_path: PathBuf,
    pub images_dir: PathBuf,
    pub app_icons_dir: PathBuf,
}

#[derive(Serialize)]
pub struct SettingEntry {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Serialize)]
pub struct ClipItem {
    pub id: i64,
    pub clip_type: String,
    pub text_content: Option<String>,
    pub html_content: Option<String>,
    pub image_path: Option<String>,
    pub content_size: i64,
    pub source_app: Option<String>,
    pub source_icon_path: Option<String>,
    pub source_site: Option<String>,
    pub is_pinned: bool,
    pub created_at: i64,
}

pub struct NewClip<'a> {
    pub clip_type: &'a str,
    pub text_content: Option<&'a str>,
    pub html_content: Option<&'a str>,
    pub image_path: Option<&'a str>,
    pub content_size: i64,
    pub source_app: Option<&'a str>,
    pub source_icon_path: Option<&'a str>,
    pub source_site: Option<&'a str>,
    pub created_at: i64,
}

pub fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn resolve_storage_paths() -> Result<StoragePaths, String> {
    let base_data = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| "Unable to locate Windows APPDATA directory".to_string())?;
    let app_dir = base_data.join("Klip");
    let images_dir = app_dir.join("images");
    let app_icons_dir = app_dir.join("app_icons");
    let db_path = app_dir.join("klip.db");

    fs::create_dir_all(&app_dir).map_err(|e| format!("Failed creating Klip dir: {e}"))?;
    fs::create_dir_all(&images_dir).map_err(|e| format!("Failed creating images dir: {e}"))?;
    fs::create_dir_all(&app_icons_dir)
        .map_err(|e| format!("Failed creating app_icons dir: {e}"))?;

    Ok(StoragePaths {
        app_dir,
        db_path,
        images_dir,
        app_icons_dir,
    })
}

fn ensure_column_exists(conn: &Connection, table: &str, column: &str, col_def: &str) -> Result<(), String> {
    let pragma = format!("PRAGMA table_info({table})");
    let mut stmt = conn
        .prepare(&pragma)
        .map_err(|e| format!("Failed inspecting {table}: {e}"))?;
    let cols = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| format!("Failed reading {table} columns: {e}"))?;

    let mut found = false;
    for col in cols.flatten() {
        if col == column {
            found = true;
            break;
        }
    }
    drop(stmt);

    if !found {
        let alter = format!("ALTER TABLE {table} ADD COLUMN {column} {col_def}");
        conn.execute(&alter, [])
            .map_err(|e| format!("Failed adding {column} to {table}: {e}"))?;
    }
    Ok(())
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
            source_site TEXT,
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

    // Safe migration for Phase 1/2 databases
    ensure_column_exists(&conn, "clips", "source_site", "TEXT")?;

    let default_hotkeys = r#"{"toggle_popup":"Ctrl+Shift+Backquote","open_full_window":"Ctrl+Shift+K","toggle_pause":"Ctrl+Shift+P","paste_normal":"Enter","paste_plain":"Shift+Enter","pin_clip":"Ctrl+P","delete_clip":"Delete","focus_search":"Ctrl+F"}"#;

    let defaults = [
        ("hotkey", "Ctrl+Shift+Backquote"),
        ("hotkeys_json", default_hotkeys),
        ("theme", "dark"),
        ("font_family", "Segoe UI, Nirmala UI, sans-serif"),
        ("popup_position", "mouse"),
        ("popup_last_x", ""),
        ("popup_last_y", ""),
        ("paste_mode", "auto_paste"),
        ("autostart", "true"),
        ("collection_limit", "1000"),
        ("retention_days", "30"),
        ("max_clip_size_mb", "10"),
        ("excluded_apps", "[]"),
        ("history_locked", "false"),
        ("paused", "false"),
        ("backup_folder", ""),
        ("backup_format", "sqlite"),
        ("backup_schedule", "off"),
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

pub fn list_settings(conn: &Connection) -> Result<Vec<SettingEntry>, String> {
    let mut stmt = conn
        .prepare("SELECT key, value FROM settings ORDER BY key")
        .map_err(|e| format!("Failed listing settings: {e}"))?;

    let rows = stmt
        .query_map([], |row| {
            Ok(SettingEntry {
                key: row.get(0)?,
                value: row.get(1)?,
            })
        })
        .map_err(|e| format!("Failed querying settings: {e}"))?;

    let mut entries = Vec::new();
    for item in rows {
        entries.push(item.map_err(|e| format!("Failed reading setting entry: {e}"))?);
    }
    Ok(entries)
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

pub fn insert_clip(conn: &Connection, clip: &NewClip<'_>) -> Result<ClipItem, String> {
    conn.execute(
        "INSERT INTO clips (
            clip_type, text_content, html_content, image_path,
            content_size, source_app, source_icon_path, source_site, is_pinned, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9)",
        params![
            clip.clip_type,
            clip.text_content,
            clip.html_content,
            clip.image_path,
            clip.content_size,
            clip.source_app,
            clip.source_icon_path,
            clip.source_site,
            clip.created_at
        ],
    )
    .map_err(|e| format!("Failed inserting clip: {e}"))?;

    let id = conn.last_insert_rowid();
    Ok(ClipItem {
        id,
        clip_type: clip.clip_type.to_string(),
        text_content: clip.text_content.map(|s| s.to_string()),
        html_content: clip.html_content.map(|s| s.to_string()),
        image_path: clip.image_path.map(|s| s.to_string()),
        content_size: clip.content_size,
        source_app: clip.source_app.map(|s| s.to_string()),
        source_icon_path: clip.source_icon_path.map(|s| s.to_string()),
        source_site: clip.source_site.map(|s| s.to_string()),
        is_pinned: false,
        created_at: clip.created_at,
    })
}

pub fn get_clip_by_id(conn: &Connection, clip_id: i64) -> Result<Option<ClipItem>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, clip_type, text_content, html_content, image_path, content_size,
                    source_app, source_icon_path, source_site, is_pinned, created_at
             FROM clips WHERE id = ?1",
        )
        .map_err(|e| format!("Failed preparing get_clip_by_id: {e}"))?;

    let mut rows = stmt
        .query_map(params![clip_id], |row| {
            Ok(ClipItem {
                id: row.get(0)?,
                clip_type: row.get(1)?,
                text_content: row.get(2)?,
                html_content: row.get(3)?,
                image_path: row.get(4)?,
                content_size: row.get(5)?,
                source_app: row.get(6)?,
                source_icon_path: row.get(7)?,
                source_site: row.get(8)?,
                is_pinned: row.get::<_, i64>(9)? != 0,
                created_at: row.get(10)?,
            })
        })
        .map_err(|e| format!("Failed querying clip {clip_id}: {e}"))?;

    if let Some(res) = rows.next() {
        Ok(Some(res.map_err(|e| format!("Failed decoding clip: {e}"))?))
    } else {
        Ok(None)
    }
}

fn escape_like_pattern(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 4);
    for ch in input.chars() {
        match ch {
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

pub fn list_clips(
    conn: &Connection,
    search: Option<&str>,
    pinned_only: bool,
    limit: i64,
) -> Result<Vec<ClipItem>, String> {
    let terms: Vec<String> = search
        .unwrap_or("")
        .split_whitespace()
        .take(12)
        .map(|w| format!("%{}%", escape_like_pattern(w)))
        .collect();

    let mut sql = String::from(
        "SELECT id, clip_type, text_content, html_content, image_path, content_size,
                source_app, source_icon_path, source_site, is_pinned, created_at
         FROM clips
         WHERE 1=1",
    );

    if pinned_only {
        sql.push_str(" AND is_pinned = 1");
    }

    let mut bind_values: Vec<Value> = Vec::with_capacity(terms.len() + 1);
    for (idx, pattern) in terms.iter().enumerate() {
        let p_num = idx + 1;
        sql.push_str(&format!(
            " AND (COALESCE(text_content, '') LIKE ?{p_num} ESCAPE '\\' OR COALESCE(source_app, '') LIKE ?{p_num} ESCAPE '\\' OR COALESCE(source_site, '') LIKE ?{p_num} ESCAPE '\\' OR clip_type LIKE ?{p_num} ESCAPE '\\')"
        ));
        bind_values.push(Value::Text(pattern.clone()));
    }

    let limit_param = bind_values.len() + 1;
    sql.push_str(&format!(
        " ORDER BY created_at DESC, id DESC LIMIT ?{limit_param}"
    ));
    bind_values.push(Value::Integer(limit));

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("Failed preparing list_clips: {e}"))?;

    let rows = stmt
        .query_map(params_from_iter(bind_values), |row| {
            Ok(ClipItem {
                id: row.get(0)?,
                clip_type: row.get(1)?,
                text_content: row.get(2)?,
                html_content: row.get(3)?,
                image_path: row.get(4)?,
                content_size: row.get(5)?,
                source_app: row.get(6)?,
                source_icon_path: row.get(7)?,
                source_site: row.get(8)?,
                is_pinned: row.get::<_, i64>(9)? != 0,
                created_at: row.get(10)?,
            })
        })
        .map_err(|e| format!("Failed querying clips: {e}"))?;

    let mut items = Vec::new();
    for r in rows {
        items.push(r.map_err(|e| format!("Failed reading clip row: {e}"))?);
    }
    Ok(items)
}

pub fn toggle_pin(conn: &Connection, clip_id: i64) -> Result<bool, String> {
    conn.execute(
        "UPDATE clips SET is_pinned = CASE WHEN is_pinned = 0 THEN 1 ELSE 0 END WHERE id = ?1",
        params![clip_id],
    )
    .map_err(|e| format!("Failed toggling pin: {e}"))?;

    let mut stmt = conn
        .prepare("SELECT is_pinned FROM clips WHERE id = ?1")
        .map_err(|e| format!("Failed reading pin state: {e}"))?;
    let pinned: i64 = stmt
        .query_row(params![clip_id], |r| r.get(0))
        .map_err(|e| format!("Clip not found after pin toggle: {e}"))?;
    Ok(pinned != 0)
}

pub fn delete_clip(conn: &Connection, clip_id: i64) -> Result<(), String> {
    let img_path: Option<String> = {
        let mut stmt = conn
            .prepare("SELECT image_path FROM clips WHERE id = ?1")
            .map_err(|e| format!("Failed preparing delete lookup: {e}"))?;
        let mut rows = stmt
            .query(params![clip_id])
            .map_err(|e| format!("Failed querying delete lookup: {e}"))?;
        if let Some(row) = rows
            .next()
            .map_err(|e| format!("Failed reading delete lookup: {e}"))?
        {
            row.get(0).ok()
        } else {
            None
        }
    };

    conn.execute("DELETE FROM clips WHERE id = ?1", params![clip_id])
        .map_err(|e| format!("Failed deleting clip: {e}"))?;

    if let Some(path) = img_path {
        if !path.is_empty() {
            let _ = fs::remove_file(path);
        }
    }

    Ok(())
}

pub fn enforce_retention_and_limits(conn: &Connection) -> Result<Vec<i64>, String> {
    let mut removed_ids = Vec::new();
    let now_ms = now_unix_ms();

    let retention_raw = get_setting(conn, "retention_days")?.unwrap_or_else(|| "30".to_string());
    if retention_raw != "all" {
        if let Ok(days) = retention_raw.parse::<i64>() {
            if days > 0 {
                let cutoff = now_ms - days * 86_400_000;
                let mut stmt = conn
                    .prepare("SELECT id, image_path FROM clips WHERE is_pinned = 0 AND created_at < ?1")
                    .map_err(|e| format!("Failed querying expired clips: {e}"))?;
                let rows = stmt
                    .query_map(params![cutoff], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))
                    })
                    .map_err(|e| format!("Failed reading expired clips: {e}"))?;

                let mut to_remove = Vec::new();
                for row in rows.flatten() {
                    to_remove.push(row);
                }
                drop(stmt);

                for (id, img) in to_remove {
                    conn.execute("DELETE FROM clips WHERE id = ?1", params![id])
                        .map_err(|e| format!("Failed pruning expired clip {id}: {e}"))?;
                    if let Some(p) = img {
                        let _ = fs::remove_file(p);
                    }
                    removed_ids.push(id);
                }
            }
        }
    }

    let limit_raw = get_setting(conn, "collection_limit")?.unwrap_or_else(|| "1000".to_string());
    if limit_raw != "unlimited" {
        if let Ok(limit) = limit_raw.parse::<i64>() {
            if limit > 0 {
                let total: i64 = conn
                    .query_row("SELECT COUNT(*) FROM clips", [], |r| r.get(0))
                    .map_err(|e| format!("Failed counting clips: {e}"))?;

                if total > limit {
                    let excess = total - limit;
                    let mut stmt = conn
                        .prepare(
                            "SELECT id, image_path FROM clips
                             WHERE is_pinned = 0
                             ORDER BY created_at ASC, id ASC
                             LIMIT ?1",
                        )
                        .map_err(|e| format!("Failed querying excess clips: {e}"))?;

                    let rows = stmt
                        .query_map(params![excess], |r| {
                            Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))
                        })
                        .map_err(|e| format!("Failed reading excess clips: {e}"))?;

                    let mut to_remove = Vec::new();
                    for row in rows.flatten() {
                        to_remove.push(row);
                    }
                    drop(stmt);

                    for (id, img) in to_remove {
                        conn.execute("DELETE FROM clips WHERE id = ?1", params![id])
                            .map_err(|e| format!("Failed trimming clip {id}: {e}"))?;
                        if let Some(p) = img {
                            let _ = fs::remove_file(p);
                        }
                        removed_ids.push(id);
                    }
                }
            }
        }
    }

    Ok(removed_ids)
}
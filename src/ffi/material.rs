use crate::utils::cjson::{handle, parse_input, parse_optional};
use crate::utils::{chatllm, db};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::ffi::{c_char, c_int};

const FILE_FIELDS: &[&str] = &[
    "project_uid",
    "organ_uid",
    "flow_uid",
    "type_uid",
    "name",
    "description",
    "cover",
    "create_by",
];
const TYPE_FIELDS: &[&str] = &["type_name", "icon"];
const TAG_FIELDS: &[&str] = &["name", "kind", "color", "sort", "source"];
const TAG_KINDS: &[&str] = &[
    "general",
    "color",
    "style",
    "scene",
    "element",
    "usage",
    "format",
];
const TAG_SOURCES: &[&str] = &["manual", "ai"];
const VERSION_FIELDS: &[&str] = &[
    "cover",
    "design_uid",
    "child_uid",
    "soft_ver",
    "name",
    "logs",
    "fuid",
    "create_by",
];

#[derive(Deserialize, Default)]
struct SetupReq {}

#[derive(Deserialize, Default)]
struct ListReq {
    page: Option<i64>,
    page_size: Option<i64>,
    type_uid: Option<String>,
    project_uid: Option<String>,
    tag_uuid: Option<String>,
    design_uid: Option<String>,
    fuid: Option<String>,
    keyword: Option<String>,
    include_deleted: Option<bool>,
}

#[derive(Deserialize, Default)]
struct FileSearchReq {
    query: Option<String>,
    top_k: Option<i64>,
    min_score: Option<f32>,
    type_uid: Option<String>,
    tag_uuid: Option<String>,
}

#[derive(Deserialize, Default)]
struct LocateReq {
    id: Option<i64>,
    uuid: Option<String>,
    include_deleted: Option<bool>,
}

#[derive(Deserialize, Default)]
struct CreateReq {
    uuid: Option<String>,
    project_uid: Option<String>,
    organ_uid: Option<String>,
    flow_uid: Option<String>,
    type_uid: Option<String>,
    name: Option<String>,
    description: Option<String>,
    cover: Option<String>,
    create_by: Option<String>,
    design_uid: Option<String>,
    child_uid: Option<String>,
    soft_ver: Option<String>,
    logs: Option<String>,
    fuid: Option<String>,
    icon: Option<String>,
    type_name: Option<String>,
}

#[derive(Deserialize, Default)]
struct DeleteReq {
    id: Option<i64>,
    uuid: Option<String>,
    hard: Option<bool>,
}

#[derive(Deserialize, Default)]
struct TagListReq {
    page: Option<i64>,
    page_size: Option<i64>,
    kind: Option<String>,
    keyword: Option<String>,
    include_deleted: Option<bool>,
}

#[derive(Deserialize, Default)]
struct TagCreateReq {
    uuid: Option<String>,
    name: Option<String>,
    kind: Option<String>,
    color: Option<String>,
    sort: Option<i64>,
    source: Option<String>,
}

#[derive(Deserialize, Default)]
struct FileTagListReq {
    file_uuid: Option<String>,
}

#[derive(Deserialize, Default)]
struct FileTagItem {
    tag_uuid: Option<String>,
    source: Option<String>,
    confidence: Option<f64>,
}

#[derive(Deserialize, Default)]
struct FileTagSetReq {
    file_uuid: Option<String>,
    tags: Option<Vec<FileTagItem>>,
    replace: Option<bool>,
}

/// 把平铺的更新请求转成字段集合（剔除定位参数）。
fn patch_map(req: &Value) -> Map<String, Value> {
    let mut map = Map::new();
    if let Some(obj) = req.as_object() {
        for (key, value) in obj {
            if key != "id" && key != "uuid" && key != "include_deleted" {
                map.insert(key.clone(), value.clone());
            }
        }
    }
    map
}

fn page_of(req: &ListReq) -> (i64, i64, i64) {
    let page = req.page.unwrap_or(1).max(1);
    let size = req.page_size.unwrap_or(20).clamp(1, 100);
    (page, size, (page - 1) * size)
}

fn deleted_clause(include: Option<bool>) -> &'static str {
    if include.unwrap_or(false) {
        "1 = 1"
    } else {
        "deleted_at IS NULL"
    }
}

fn like(value: &str) -> String {
    let escaped = value.replace('%', "\\%").replace('_', "\\_");
    format!("%{}%", escaped)
}

fn locate(id: Option<i64>, uuid: Option<&str>) -> Result<String, (i32, String)> {
    if let Some(id) = id {
        if id > 0 {
            return Ok(format!("id = {}", id));
        }
    }
    if let Some(uuid) = uuid {
        if !uuid.is_empty() {
            return Ok(format!("uuid = {}", db::esc(uuid)));
        }
    }
    Err((400104, "id or uuid is required".to_string()))
}

fn sql_value(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(db::esc(s)),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(if *b { "1" } else { "0" }.to_string()),
        _ => None,
    }
}

fn build_set(
    set: &Map<String, Value>,
    allowed: &[&str],
) -> Result<Vec<String>, (i32, String)> {
    let mut fields = Vec::new();
    for (key, value) in set {
        if !allowed.contains(&key.as_str()) {
            return Err((400104, format!("field not allowed: {}", key)));
        }
        if let Some(sql) = sql_value(value) {
            fields.push(format!("{} = {}", key, sql));
        }
    }
    if fields.is_empty() {
        return Err((400103, "set is empty".to_string()));
    }
    fields.push(format!("updated_at = {}", db::now_sql()));
    Ok(fields)
}

fn json_row(db_name: &str, table: &str, where_sql: &str) -> Result<Value, (i32, String)> {
    let sql = format!("SELECT * FROM {} WHERE {} LIMIT 1", table, where_sql);
    db::query(db_name, &sql)?
        .into_iter()
        .next()
        .ok_or_else(|| (404100, format!("{} not found", table)))
}

fn paged(
    db_name: &str,
    table: &str,
    where_sql: &str,
    order: &str,
    req: &ListReq,
) -> Result<Value, (i32, String)> {
    let (page, size, offset) = page_of(req);
    let count_sql = format!("SELECT count(*) AS n FROM {} WHERE {}", table, where_sql);
    let total = db::query_count(db_name, &count_sql)?;
    let list_sql = format!(
        "SELECT * FROM {} WHERE {} ORDER BY {} LIMIT {} OFFSET {}",
        table, where_sql, order, size, offset
    );
    let list = db::query(db_name, &list_sql)?;
    Ok(json!({ "list": list, "page": page, "page_size": size, "total": total }))
}

// ---------------------------------------------------------------------------
// setup
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn post_setup(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let _: SetupReq = parse_optional(json_in)?;
        db::create_table(
            db::MATERIAL_DB,
            "material_file",
            "id INTEGER PRIMARY KEY AUTOINCREMENT, uuid TEXT NOT NULL UNIQUE, project_uid TEXT, organ_uid TEXT, flow_uid TEXT, type_uid TEXT, name TEXT, description TEXT, cover TEXT, create_by TEXT, created_at TEXT DEFAULT (datetime('now','localtime')), updated_at TEXT DEFAULT (datetime('now','localtime')), deleted_at TEXT",
        )?;
        db::create_table(
            db::MATERIAL_DB,
            "material_type",
            "id INTEGER PRIMARY KEY AUTOINCREMENT, uuid TEXT NOT NULL UNIQUE, type_name TEXT UNIQUE, icon TEXT, created_at TEXT DEFAULT (datetime('now','localtime')), updated_at TEXT DEFAULT (datetime('now','localtime')), deleted_at TEXT",
        )?;
        db::create_table(
            db::MATERIAL_DB,
            "material_version",
            "id INTEGER PRIMARY KEY AUTOINCREMENT, uuid TEXT NOT NULL UNIQUE, cover TEXT, design_uid TEXT, child_uid TEXT, soft_ver TEXT, name TEXT, logs TEXT, fuid TEXT, create_by TEXT, created_at TEXT DEFAULT (datetime('now','localtime')), updated_at TEXT DEFAULT (datetime('now','localtime')), deleted_at TEXT",
        )?;
        db::create_table(
            db::MATERIAL_DB,
            "material_tag",
            "id INTEGER PRIMARY KEY AUTOINCREMENT, uuid TEXT NOT NULL UNIQUE, name TEXT NOT NULL UNIQUE, kind TEXT NOT NULL DEFAULT 'general', color TEXT, sort INTEGER NOT NULL DEFAULT 0, source TEXT NOT NULL DEFAULT 'manual', created_at TEXT DEFAULT (datetime('now','localtime')), updated_at TEXT DEFAULT (datetime('now','localtime')), deleted_at TEXT",
        )?;
        db::create_table(
            db::MATERIAL_DB,
            "material_file_tag",
            "id INTEGER PRIMARY KEY AUTOINCREMENT, file_uuid TEXT NOT NULL, tag_uuid TEXT NOT NULL, source TEXT NOT NULL DEFAULT 'manual', confidence REAL NOT NULL DEFAULT 1.0, created_at TEXT DEFAULT (datetime('now','localtime')), UNIQUE(file_uuid, tag_uuid)",
        )?;

        let indexes = [
            "CREATE INDEX IF NOT EXISTS idx_material_file_project_uid ON material_file(project_uid)",
            "CREATE INDEX IF NOT EXISTS idx_material_file_organ_uid ON material_file(organ_uid)",
            "CREATE INDEX IF NOT EXISTS idx_material_file_type_uid ON material_file(type_uid)",
            "CREATE INDEX IF NOT EXISTS idx_material_file_deleted_at ON material_file(deleted_at)",
            "CREATE INDEX IF NOT EXISTS idx_material_file_type_deleted ON material_file(type_uid, deleted_at)",
            "CREATE INDEX IF NOT EXISTS idx_material_file_project_deleted ON material_file(project_uid, deleted_at)",
            "CREATE INDEX IF NOT EXISTS idx_material_type_uuid ON material_type(uuid)",
            "CREATE INDEX IF NOT EXISTS idx_material_version_design_uid ON material_version(design_uid)",
            "CREATE INDEX IF NOT EXISTS idx_material_version_child_uid ON material_version(child_uid)",
            "CREATE INDEX IF NOT EXISTS idx_material_version_fuid ON material_version(fuid)",
            "CREATE INDEX IF NOT EXISTS idx_material_version_design_deleted ON material_version(design_uid, deleted_at)",
            "CREATE INDEX IF NOT EXISTS idx_material_version_child_deleted ON material_version(child_uid, deleted_at)",
            "CREATE INDEX IF NOT EXISTS idx_material_tag_kind ON material_tag(kind, sort)",
            "CREATE INDEX IF NOT EXISTS idx_material_tag_name ON material_tag(name)",
            "CREATE INDEX IF NOT EXISTS idx_material_tag_deleted ON material_tag(deleted_at)",
            "CREATE INDEX IF NOT EXISTS idx_material_file_tag_file ON material_file_tag(file_uuid)",
            "CREATE INDEX IF NOT EXISTS idx_material_file_tag_tag ON material_file_tag(tag_uuid)",
        ];
        for sql in indexes {
            db::update(db::MATERIAL_DB, sql)?;
        }

        Ok(json!({ "tables": 5, "indexes": indexes.len() }))
    })
}

// ---------------------------------------------------------------------------
// file
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn post_file_list(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: ListReq = parse_input(json_in)?;
        let mut where_parts = vec![format!("f.{}", deleted_clause(req.include_deleted))];
        let mut joins = String::new();
        if let Some(v) = req.type_uid.as_deref().filter(|s| !s.is_empty()) {
            where_parts.push(format!("f.type_uid = {}", db::esc(v)));
        }
        if let Some(v) = req.project_uid.as_deref().filter(|s| !s.is_empty()) {
            where_parts.push(format!("f.project_uid = {}", db::esc(v)));
        }
        if let Some(v) = req.tag_uuid.as_deref().filter(|s| !s.is_empty()) {
            joins.push_str(
                " JOIN material_file_tag ft ON ft.file_uuid = f.uuid \
                 JOIN material_tag tg ON tg.uuid = ft.tag_uuid",
            );
            where_parts.push(format!(
                "ft.tag_uuid = {} AND tg.deleted_at IS NULL",
                db::esc(v)
            ));
        }
        if let Some(v) = req.keyword.as_deref().filter(|s| !s.is_empty()) {
            let p = db::esc(&like(v));
            where_parts.push(format!("(f.name LIKE {} OR f.description LIKE {})", p, p));
        }
        let where_sql = where_parts.join(" AND ");

        let (page, size, offset) = page_of(&req);
        let count_sql = format!(
            "SELECT count(*) AS n FROM material_file f{} WHERE {}",
            joins, where_sql
        );
        let total = db::query_count(db::MATERIAL_DB, &count_sql)?;
        let list_sql = format!(
            "SELECT f.*, t.type_name AS type_name FROM material_file f \
             LEFT JOIN material_type t ON f.type_uid = t.uuid{} \
             WHERE {} ORDER BY f.id DESC LIMIT {} OFFSET {}",
            joins, where_sql, size, offset
        );
        let mut list = db::query(db::MATERIAL_DB, &list_sql)?;
        attach_tags(&mut list)?;
        Ok(json!({ "list": list, "page": page, "page_size": size, "total": total }))
    })
}

#[no_mangle]
pub extern "C" fn post_file_search(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: FileSearchReq = parse_input(json_in)?;
        let query = req.query.unwrap_or_default();
        let query = query.trim();
        if query.is_empty() {
            return Err((400103, "query is required".to_string()));
        }
        let top_k = req.top_k.unwrap_or(5).clamp(1, 50);
        let min_score = req.min_score.unwrap_or(0.2);
        let type_uid = req.type_uid;
        let tag_uuid = req.tag_uuid;

        let result = chatllm::material_search(query, top_k, min_score)?;
        let scored = result
            .get("list")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut list = Vec::new();
        for item in scored {
            let Some(uuid) = item.get("uuid").and_then(Value::as_str) else {
                continue;
            };
            let sql = format!(
                "SELECT f.*, t.type_name AS type_name FROM material_file f \
                 LEFT JOIN material_type t ON f.type_uid = t.uuid \
                 WHERE f.uuid = {} AND f.deleted_at IS NULL LIMIT 1",
                db::esc(uuid)
            );
            let mut rows = db::query(db::MATERIAL_DB, &sql)?;
            let Some(mut row) = rows.pop() else {
                continue;
            };
            attach_tags(std::slice::from_mut(&mut row))?;

            if let Some(type_uid) = type_uid.as_deref().filter(|value| !value.is_empty()) {
                if row.get("type_uid").and_then(Value::as_str) != Some(type_uid) {
                    continue;
                }
            }
            if let Some(tag_uuid) = tag_uuid.as_deref().filter(|value| !value.is_empty()) {
                let matched = row
                    .get("tags")
                    .and_then(Value::as_array)
                    .map(|tags| {
                        tags.iter().any(|tag| {
                            tag.get("tag_uuid").and_then(Value::as_str) == Some(tag_uuid)
                        })
                    })
                    .unwrap_or(false);
                if !matched {
                    continue;
                }
            }
            if let Some(score) = item.get("score").cloned() {
                if let Some(obj) = row.as_object_mut() {
                    obj.insert("score".to_string(), score);
                }
            }
            list.push(row);
        }

        Ok(json!({ "list": list, "total": list.len() }))
    })
}

#[no_mangle]
pub extern "C" fn post_file_get(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: LocateReq = parse_input(json_in)?;
        let base = locate(req.id, req.uuid.as_deref())?;
        let where_sql = format!("{} AND {}", base, deleted_clause(req.include_deleted));
        json_row(db::MATERIAL_DB, "material_file", &where_sql)
    })
}

#[no_mangle]
pub extern "C" fn post_file_create(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: CreateReq = parse_input(json_in)?;
        let uuid = req
            .uuid
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let name = req.name.unwrap_or_default();
        if name.trim().is_empty() {
            return Err((400103, "name is required".to_string()));
        }
        let sql = format!(
            "INSERT INTO material_file (uuid, project_uid, organ_uid, flow_uid, type_uid, name, description, cover, create_by, created_at, updated_at) \
             VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {})",
            db::esc(&uuid),
            db::esc(req.project_uid.as_deref().unwrap_or("")),
            db::esc(req.organ_uid.as_deref().unwrap_or("")),
            db::esc(req.flow_uid.as_deref().unwrap_or("")),
            db::esc(req.type_uid.as_deref().unwrap_or("")),
            db::esc(&name),
            db::esc(req.description.as_deref().unwrap_or("")),
            db::esc(req.cover.as_deref().unwrap_or("")),
            db::esc(req.create_by.as_deref().unwrap_or("")),
            db::now_sql(),
            db::now_sql(),
        );
        db::insert(db::MATERIAL_DB, &sql)?;
        let row = json_row(
            db::MATERIAL_DB,
            "material_file",
            &format!("uuid = {}", db::esc(&uuid)),
        )?;
        Ok(json!({ "id": row.get("id").cloned().unwrap_or(Value::Null), "uuid": uuid }))
    })
}

#[no_mangle]
pub extern "C" fn post_file_update(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: Value = parse_input(json_in)?;
        let where_sql = locate(
            req.get("id").and_then(Value::as_i64),
            req.get("uuid").and_then(Value::as_str),
        )?;
        let fields = build_set(&patch_map(&req), FILE_FIELDS)?;
        let sql = format!(
            "UPDATE material_file SET {} WHERE {}",
            fields.join(", "),
            where_sql
        );
        db::update(db::MATERIAL_DB, &sql)?;
        json_row(db::MATERIAL_DB, "material_file", &where_sql)
    })
}

#[no_mangle]
pub extern "C" fn post_file_delete(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: DeleteReq = parse_input(json_in)?;
        let where_sql = locate(req.id, req.uuid.as_deref())?;
        let hard = req.hard.unwrap_or(false);
        let sql = if hard {
            format!("DELETE FROM material_file WHERE {}", where_sql)
        } else {
            format!(
                "UPDATE material_file SET deleted_at = {} WHERE {}",
                db::now_sql(),
                where_sql
            )
        };
        if hard {
            db::delete(db::MATERIAL_DB, &sql)?;
        } else {
            db::update(db::MATERIAL_DB, &sql)?;
        }
        Ok(json!({ "deleted": true, "hard": hard }))
    })
}

// ---------------------------------------------------------------------------
// type
// ---------------------------------------------------------------------------

fn type_where(req: &ListReq) -> String {
    let mut parts = vec![deleted_clause(req.include_deleted).to_string()];
    if let Some(v) = req.keyword.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("type_name LIKE {}", db::esc(&like(v))));
    }
    parts.join(" AND ")
}

#[no_mangle]
pub extern "C" fn post_type_list(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: ListReq = parse_input(json_in)?;
        paged(
            db::MATERIAL_DB,
            "material_type",
            &type_where(&req),
            "id DESC",
            &req,
        )
    })
}

#[no_mangle]
pub extern "C" fn post_type_get(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: LocateReq = parse_input(json_in)?;
        let base = locate(req.id, req.uuid.as_deref())?;
        json_row(
            db::MATERIAL_DB,
            "material_type",
            &format!("{} AND {}", base, deleted_clause(req.include_deleted)),
        )
    })
}

#[no_mangle]
pub extern "C" fn post_type_create(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: CreateReq = parse_input(json_in)?;
        let type_name = req.type_name.unwrap_or_default();
        if type_name.trim().is_empty() {
            return Err((400103, "type_name is required".to_string()));
        }
        let uuid = req
            .uuid
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let sql = format!(
            "INSERT INTO material_type (uuid, type_name, icon, created_at, updated_at) VALUES ({}, {}, {}, {}, {})",
            db::esc(&uuid),
            db::esc(&type_name),
            db::esc(req.icon.as_deref().unwrap_or("")),
            db::now_sql(),
            db::now_sql(),
        );
        db::insert(db::MATERIAL_DB, &sql)?;
        let row = json_row(
            db::MATERIAL_DB,
            "material_type",
            &format!("uuid = {}", db::esc(&uuid)),
        )?;
        Ok(json!({ "id": row.get("id").cloned().unwrap_or(Value::Null), "uuid": uuid }))
    })
}

#[no_mangle]
pub extern "C" fn post_type_update(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: Value = parse_input(json_in)?;
        let where_sql = locate(
            req.get("id").and_then(Value::as_i64),
            req.get("uuid").and_then(Value::as_str),
        )?;
        let fields = build_set(&patch_map(&req), TYPE_FIELDS)?;
        db::update(
            db::MATERIAL_DB,
            &format!(
                "UPDATE material_type SET {} WHERE {}",
                fields.join(", "),
                where_sql
            ),
        )?;
        json_row(db::MATERIAL_DB, "material_type", &where_sql)
    })
}

#[no_mangle]
pub extern "C" fn post_type_delete(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: DeleteReq = parse_input(json_in)?;
        let where_sql = locate(req.id, req.uuid.as_deref())?;
        let hard = req.hard.unwrap_or(false);
        if hard {
            db::delete(
                db::MATERIAL_DB,
                &format!("DELETE FROM material_type WHERE {}", where_sql),
            )?;
        } else {
            db::update(
                db::MATERIAL_DB,
                &format!(
                    "UPDATE material_type SET deleted_at = {} WHERE {}",
                    db::now_sql(),
                    where_sql
                ),
            )?;
        }
        Ok(json!({ "deleted": true, "hard": hard }))
    })
}

// ---------------------------------------------------------------------------
// version
// ---------------------------------------------------------------------------

fn version_where(req: &ListReq) -> String {
    let mut parts = vec![deleted_clause(req.include_deleted).to_string()];
    if let Some(v) = req.design_uid.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("design_uid = {}", db::esc(v)));
    }
    if let Some(v) = req.fuid.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("fuid = {}", db::esc(v)));
    }
    if let Some(v) = req.keyword.as_deref().filter(|s| !s.is_empty()) {
        let p = db::esc(&like(v));
        parts.push(format!("(name LIKE {} OR logs LIKE {})", p, p));
    }
    parts.join(" AND ")
}

#[no_mangle]
pub extern "C" fn post_version_list(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: ListReq = parse_input(json_in)?;
        paged(
            db::MATERIAL_DB,
            "material_version",
            &version_where(&req),
            "id DESC",
            &req,
        )
    })
}

#[no_mangle]
pub extern "C" fn post_version_get(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: LocateReq = parse_input(json_in)?;
        let base = locate(req.id, req.uuid.as_deref())?;
        json_row(
            db::MATERIAL_DB,
            "material_version",
            &format!("{} AND {}", base, deleted_clause(req.include_deleted)),
        )
    })
}

#[no_mangle]
pub extern "C" fn post_version_create(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: CreateReq = parse_input(json_in)?;
        let uuid = req
            .uuid
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let sql = format!(
            "INSERT INTO material_version (uuid, cover, design_uid, child_uid, soft_ver, name, logs, fuid, create_by, created_at, updated_at) \
             VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {})",
            db::esc(&uuid),
            db::esc(req.cover.as_deref().unwrap_or("")),
            db::esc(req.design_uid.as_deref().unwrap_or("")),
            db::esc(req.child_uid.as_deref().unwrap_or("")),
            db::esc(req.soft_ver.as_deref().unwrap_or("")),
            db::esc(req.name.as_deref().unwrap_or("")),
            db::esc(req.logs.as_deref().unwrap_or("")),
            db::esc(req.fuid.as_deref().unwrap_or("")),
            db::esc(req.create_by.as_deref().unwrap_or("")),
            db::now_sql(),
            db::now_sql(),
        );
        db::insert(db::MATERIAL_DB, &sql)?;
        let row = json_row(
            db::MATERIAL_DB,
            "material_version",
            &format!("uuid = {}", db::esc(&uuid)),
        )?;
        Ok(json!({ "id": row.get("id").cloned().unwrap_or(Value::Null), "uuid": uuid }))
    })
}

#[no_mangle]
pub extern "C" fn post_version_update(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: Value = parse_input(json_in)?;
        let where_sql = locate(
            req.get("id").and_then(Value::as_i64),
            req.get("uuid").and_then(Value::as_str),
        )?;
        let fields = build_set(&patch_map(&req), VERSION_FIELDS)?;
        db::update(
            db::MATERIAL_DB,
            &format!(
                "UPDATE material_version SET {} WHERE {}",
                fields.join(", "),
                where_sql
            ),
        )?;
        json_row(db::MATERIAL_DB, "material_version", &where_sql)
    })
}

#[no_mangle]
pub extern "C" fn post_version_delete(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: DeleteReq = parse_input(json_in)?;
        let where_sql = locate(req.id, req.uuid.as_deref())?;
        let hard = req.hard.unwrap_or(false);
        if hard {
            db::delete(
                db::MATERIAL_DB,
                &format!("DELETE FROM material_version WHERE {}", where_sql),
            )?;
        } else {
            db::update(
                db::MATERIAL_DB,
                &format!(
                    "UPDATE material_version SET deleted_at = {} WHERE {}",
                    db::now_sql(),
                    where_sql
                ),
            )?;
        }
        Ok(json!({ "deleted": true, "hard": hard }))
    })
}

// ---------------------------------------------------------------------------
// tag
// ---------------------------------------------------------------------------

fn tag_where(req: &TagListReq) -> String {
    let mut parts = vec![deleted_clause(req.include_deleted).to_string()];
    if let Some(v) = req.kind.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("kind = {}", db::esc(v)));
    }
    if let Some(v) = req.keyword.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("name LIKE {}", db::esc(&like(v))));
    }
    parts.join(" AND ")
}

fn valid_tag_kind(kind: &str) -> bool {
    TAG_KINDS.contains(&kind)
}

fn valid_tag_source(source: &str) -> bool {
    TAG_SOURCES.contains(&source)
}

#[no_mangle]
pub extern "C" fn post_tag_list(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: TagListReq = parse_input(json_in)?;
        let list_req = ListReq {
            page: req.page,
            page_size: req.page_size,
            include_deleted: req.include_deleted,
            keyword: req.keyword.clone(),
            ..Default::default()
        };
        paged(
            db::MATERIAL_DB,
            "material_tag",
            &tag_where(&req),
            "sort ASC, id DESC",
            &list_req,
        )
    })
}

#[no_mangle]
pub extern "C" fn post_tag_get(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: LocateReq = parse_input(json_in)?;
        let base = locate(req.id, req.uuid.as_deref())?;
        json_row(
            db::MATERIAL_DB,
            "material_tag",
            &format!("{} AND {}", base, deleted_clause(req.include_deleted)),
        )
    })
}

#[no_mangle]
pub extern "C" fn post_tag_create(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: TagCreateReq = parse_input(json_in)?;
        let name = req.name.unwrap_or_default();
        if name.trim().is_empty() {
            return Err((400103, "name is required".to_string()));
        }
        let kind = req.kind.unwrap_or_else(|| "general".to_string());
        if !valid_tag_kind(&kind) {
            return Err((400104, format!("invalid tag kind: {}", kind)));
        }
        let source = req.source.unwrap_or_else(|| "manual".to_string());
        if !valid_tag_source(&source) {
            return Err((400104, format!("invalid tag source: {}", source)));
        }
        let uuid = req
            .uuid
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let sql = format!(
            "INSERT INTO material_tag (uuid, name, kind, color, sort, source, created_at, updated_at) \
             VALUES ({}, {}, {}, {}, {}, {}, {}, {})",
            db::esc(&uuid),
            db::esc(&name),
            db::esc(&kind),
            db::esc(req.color.as_deref().unwrap_or("")),
            req.sort.unwrap_or(0),
            db::esc(&source),
            db::now_sql(),
            db::now_sql(),
        );
        db::insert(db::MATERIAL_DB, &sql)?;
        let row = json_row(
            db::MATERIAL_DB,
            "material_tag",
            &format!("uuid = {}", db::esc(&uuid)),
        )?;
        Ok(json!({ "id": row.get("id").cloned().unwrap_or(Value::Null), "uuid": uuid }))
    })
}

#[no_mangle]
pub extern "C" fn post_tag_update(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: Value = parse_input(json_in)?;
        let where_sql = locate(
            req.get("id").and_then(Value::as_i64),
            req.get("uuid").and_then(Value::as_str),
        )?;
        let set = patch_map(&req);
        if let Some(name) = set.get("name").and_then(Value::as_str) {
            if name.trim().is_empty() {
                return Err((400103, "name is empty".to_string()));
            }
        }
        if let Some(kind) = set.get("kind").and_then(Value::as_str) {
            if !valid_tag_kind(kind) {
                return Err((400104, format!("invalid tag kind: {}", kind)));
            }
        }
        if let Some(source) = set.get("source").and_then(Value::as_str) {
            if !valid_tag_source(source) {
                return Err((400104, format!("invalid tag source: {}", source)));
            }
        }
        let fields = build_set(&set, TAG_FIELDS)?;
        db::update(
            db::MATERIAL_DB,
            &format!(
                "UPDATE material_tag SET {} WHERE {}",
                fields.join(", "),
                where_sql
            ),
        )?;
        json_row(db::MATERIAL_DB, "material_tag", &where_sql)
    })
}

#[no_mangle]
pub extern "C" fn post_tag_delete(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: DeleteReq = parse_input(json_in)?;
        let where_sql = locate(req.id, req.uuid.as_deref())?;
        let row = json_row(
            db::MATERIAL_DB,
            "material_tag",
            &format!("{} AND deleted_at IS NULL", where_sql),
        )?;
        let tag_uuid = row.get("uuid").and_then(Value::as_str).unwrap_or("");
        let hard = req.hard.unwrap_or(false);
        if hard {
            db::delete(
                db::MATERIAL_DB,
                &format!(
                    "DELETE FROM material_file_tag WHERE tag_uuid = {}",
                    db::esc(tag_uuid)
                ),
            )?;
            db::delete(
                db::MATERIAL_DB,
                &format!("DELETE FROM material_tag WHERE {}", where_sql),
            )?;
        } else {
            db::update(
                db::MATERIAL_DB,
                &format!(
                    "UPDATE material_tag SET deleted_at = {}, updated_at = {} WHERE {}",
                    db::now_sql(),
                    db::now_sql(),
                    where_sql
                ),
            )?;
        }
        Ok(json!({ "deleted": true, "hard": hard }))
    })
}

// ---------------------------------------------------------------------------
// file tag
// ---------------------------------------------------------------------------

fn attach_tags(list: &mut [Value]) -> Result<(), (i32, String)> {
    for row in list.iter_mut() {
        let uuid = row
            .get("uuid")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(uuid) = uuid {
            let tags = file_tags(&uuid)?;
            let tags = tags.get("list").cloned().unwrap_or_else(|| json!([]));
            if let Some(obj) = row.as_object_mut() {
                obj.insert("tags".to_string(), tags);
            }
        }
    }
    Ok(())
}

fn file_tags(file_uuid: &str) -> Result<Value, (i32, String)> {
    let sql = format!(
        "SELECT ft.tag_uuid, ft.source, ft.confidence, t.name, t.kind, t.color, t.sort \
         FROM material_file_tag ft JOIN material_tag t ON t.uuid = ft.tag_uuid \
         WHERE ft.file_uuid = {} AND t.deleted_at IS NULL \
         ORDER BY t.sort ASC, t.name ASC",
        db::esc(file_uuid)
    );
    Ok(json!({ "list": db::query(db::MATERIAL_DB, &sql)? }))
}

#[no_mangle]
pub extern "C" fn post_file_tag_list(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: FileTagListReq = parse_input(json_in)?;
        let file_uuid = req.file_uuid.unwrap_or_default();
        if file_uuid.trim().is_empty() {
            return Err((400103, "file_uuid is required".to_string()));
        }
        file_tags(&file_uuid)
    })
}

#[no_mangle]
pub extern "C" fn post_file_tag_set(json_in: *const c_char, json_out: *mut c_char) -> c_int {
    handle(json_in, json_out, || {
        let req: FileTagSetReq = parse_input(json_in)?;
        let file_uuid = req.file_uuid.unwrap_or_default();
        if file_uuid.trim().is_empty() {
            return Err((400103, "file_uuid is required".to_string()));
        }
        let exists = db::query(
            db::MATERIAL_DB,
            &format!(
                "SELECT uuid FROM material_file WHERE uuid = {} AND deleted_at IS NULL LIMIT 1",
                db::esc(&file_uuid)
            ),
        )?;
        if exists.is_empty() {
            return Err((404100, "material_file not found".to_string()));
        }

        let mut seen = HashSet::new();
        let mut items = Vec::new();
        for item in req.tags.unwrap_or_default() {
            let tag_uuid = item.tag_uuid.unwrap_or_default();
            if tag_uuid.trim().is_empty() {
                return Err((400103, "tag_uuid is required".to_string()));
            }
            if !seen.insert(tag_uuid.clone()) {
                continue;
            }
            let tag = db::query(
                db::MATERIAL_DB,
                &format!(
                    "SELECT uuid FROM material_tag WHERE uuid = {} AND deleted_at IS NULL LIMIT 1",
                    db::esc(&tag_uuid)
                ),
            )?;
            if tag.is_empty() {
                return Err((404100, format!("tag not found: {}", tag_uuid)));
            }
            let source = item.source.unwrap_or_else(|| "manual".to_string());
            if !valid_tag_source(&source) {
                return Err((400104, format!("invalid tag source: {}", source)));
            }
            let confidence = item.confidence.unwrap_or(1.0);
            if !(0.0..=1.0).contains(&confidence) {
                return Err((400104, "confidence must be between 0 and 1".to_string()));
            }
            items.push((tag_uuid, source, confidence));
        }

        let replace = req.replace.unwrap_or(true);
        if replace {
            db::delete(
                db::MATERIAL_DB,
                &format!(
                    "DELETE FROM material_file_tag WHERE file_uuid = {}",
                    db::esc(&file_uuid)
                ),
            )?;
        }
        for (tag_uuid, source, confidence) in items {
            let sql = format!(
                "INSERT INTO material_file_tag (file_uuid, tag_uuid, source, confidence, created_at) \
                 VALUES ({}, {}, {}, {}, {}) \
                 ON CONFLICT(file_uuid, tag_uuid) DO UPDATE SET \
                 source = excluded.source, confidence = excluded.confidence",
                db::esc(&file_uuid),
                db::esc(&tag_uuid),
                db::esc(&source),
                confidence,
                db::now_sql(),
            );
            db::insert(db::MATERIAL_DB, &sql)?;
        }
        file_tags(&file_uuid)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_defaults_and_clamps() {
        let req = ListReq::default();
        assert_eq!(page_of(&req), (1, 20, 0));
        let req = ListReq {
            page: Some(3),
            page_size: Some(500),
            ..Default::default()
        };
        assert_eq!(page_of(&req), (3, 100, 200));
    }

    #[test]
    fn locate_requires_id_or_uuid() {
        assert!(locate(None, None).is_err());
        assert_eq!(locate(Some(3), None).unwrap(), "id = 3");
        assert_eq!(locate(None, Some("abc")).unwrap(), "uuid = 'abc'");
    }

    #[test]
    fn like_escapes_wildcards() {
        assert_eq!(like("a%b_c"), "%a\\%b\\_c%");
    }

    #[test]
    fn build_set_rejects_unknown_fields() {
        let mut set = Map::new();
        set.insert("bad".to_string(), json!("x"));
        assert!(matches!(
            build_set(&set, FILE_FIELDS),
            Err((400104, _))
        ));
    }

    #[test]
    fn build_set_accepts_known_fields() {
        let mut set = Map::new();
        set.insert("name".to_string(), json!("n"));
        set.insert("cover".to_string(), Value::Null);
        let fields = build_set(&set, FILE_FIELDS).unwrap();
        assert_eq!(fields.len(), 2);
        assert!(fields[0].starts_with("name = '"));
    }

    #[test]
    fn esc_quotes_single_quote() {
        assert_eq!(db::esc("a'b"), "'a''b'");
    }

    #[test]
    fn tag_kinds_and_sources_are_limited() {
        assert!(valid_tag_kind("color"));
        assert!(!valid_tag_kind("bad"));
        assert!(valid_tag_source("ai"));
        assert!(!valid_tag_source("robot"));
    }

    #[test]
    fn tag_fields_reject_unknown() {
        let mut set = Map::new();
        set.insert("bad".to_string(), json!("x"));
        assert!(matches!(build_set(&set, TAG_FIELDS), Err((400104, _))));
    }
}

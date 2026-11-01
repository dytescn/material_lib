# material_lib MCP 服务

> 状态：接口文档（对应 `material_lib` 1.0.1 的 FFI）
> 更新日期：2026-10-28
> 打包产物：`target/release/material_lib.dll`
> 底层接口：`post_setup`、`post_file_*`、`post_type_*`、`post_version_*`、`post_tag_*`、`post_file_tag_*`

本文把 `material_lib` 的 24 个 FFI 接口整理为 MCP 工具：工具名、输入结构、
错误语义与实现注意事项。MCP server 由宿主实现，底层通过 FFI 调用
`material_lib.dll`，也可以让 server 直接依赖本 crate 的 `rlib`。

`material_lib` 只负责素材文件、类型、版本、标签与内容搜索；页面不直连
`database.dll` 或文件系统。

---

## 1. 工具映射规则

| 项 | 规则 |
| --- | --- |
| 工具名 | `material_` + FFI-Symbol（如 `material_file_list`） |
| 底层符号 | `post_` + FFI-Symbol（如 `post_file_list`） |
| HTTP（可选） | `POST /material_lib` + `FFI-Symbol: file_list` |
| 输出外壳 | `{ "code": 200, "data": {}, "msg": "success" }` |
| 只读工具 | `*_list` / `*_get` / `*_search` |
| 写工具 | `*_create` / `*_update` / `*_delete` / `*_set` / `setup` |

MCP 侧建议：

- 工具结果同时返回 `content`（摘要文本）与 `structuredContent`（原始 `{code,data,msg}`）；
- `code=200` 时 `isError=false`；其余 `isError=true`，并把 `msg` 放在文本中；
- FFI 返回 `-1` / `-2` 视为 server 内部错误（参数或缓冲区问题）。

---

## 2. 调用约定

### 2.1 FFI

```c
int post_file_list(const char *json_in, char *json_out);
```

| 项 | 约定 |
| --- | --- |
| `json_in` | NUL 结尾 UTF-8 JSON；`setup` 可为空 / `null` |
| `json_out` | 至少 65536 字节（含结尾 NUL） |
| 返回值 | `0` 已写入；`-1` 输出指针为空；`-2` 输出超过 64 KiB |
| 字段命名 | `snake_case` |
| 分页 | `page` 从 1 开始；`page_size` 默认 20、上限 100 |
| 时间 | 本地时间字符串（`datetime('now','localtime')`） |

### 2.2 错误码

| code | 含义 |
| --- | --- |
| `200` | 成功 |
| `400100` | `json_in` 为空 |
| `400101` | 入参不是合法 UTF-8 |
| `400102` | JSON 解析失败 / 字段类型不符 |
| `400103` | 必填字段为空（`name`、`type_name`、`file_uuid`、`query` 等） |
| `400104` | 非法参数（未知更新 / 过滤字段、非法 `kind` / `source` / `confidence`、缺 `id`/`uuid`） |
| `404100` | 素材 / 类型 / 版本 / 标签不存在 |
| `409100` | 唯一键冲突（`uuid`、`type_name`） |
| `500100` | 打开数据库失败 |
| `500101` | SQL 执行失败 |
| `500102` | 输出超过 64 KiB |
| `500103` | `chatllm.material_search` 调用失败 |

---

## 3. 输入结构

### 3.1 列表（`ListReq`）

`file_list`、`type_list`、`version_list` 共用：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `page` | int | 默认 1 |
| `page_size` | int | 默认 20，上限 100 |
| `type_uid` | string | 素材文件 / 类型过滤 |
| `project_uid` | string | 按项目过滤（文件） |
| `tag_uuid` | string | 按标签过滤（文件） |
| `design_uid` | string | 按设计文件过滤（版本） |
| `fuid` | string | 按存储编号过滤（版本） |
| `keyword` | string | 名称 / 描述模糊匹配 |
| `include_deleted` | bool | 是否包含已删除记录 |

### 3.2 定位（`LocateReq`）

`*_get`、`*_update`、`*_delete` 使用：

```json
{ "id": 12 }
```

```json
{ "uuid": "b1f0c2e4-..." }
```

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `id` | int | 自增主键；`<=0` 视为未提供 |
| `uuid` | string | 业务键 |
| `include_deleted` | bool | 读取时是否包含已删除 |

`id` 与 `uuid` 至少提供一个，否则 `400104`。

### 3.3 创建 / 更新（`CreateReq`）

`file_create`、`type_create`、`version_create` 共用字段（按资源取子集）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `uuid` | string | 可选，缺省自动生成 |
| `name` | string | 文件 / 版本名称；`file_create` 必填 |
| `type_name` | string | 类型名称；`type_create` 必填 |
| `icon` | string | 类型图标 |
| `project_uid` / `organ_uid` / `flow_uid` / `type_uid` | string | 关联标识 |
| `description` | string | 描述 |
| `cover` | string | 封面 |
| `create_by` | string | 创建人 |
| `design_uid` / `child_uid` | string | 版本关联 |
| `soft_ver` | string | 软件版本 |
| `logs` | string | 版本日志 |
| `fuid` | string | 文件存储编号 |

更新接口为**平铺字段**，不使用 `set`；只接受资源白名单字段：

- 文件：`project_uid`、`organ_uid`、`flow_uid`、`type_uid`、`name`、`description`、`cover`、`create_by`
- 类型：`type_name`、`icon`
- 版本：`cover`、`design_uid`、`child_uid`、`soft_ver`、`name`、`logs`、`fuid`、`create_by`
- 标签：`name`、`kind`、`color`、`sort`、`source`

未知字段返回 `400104`；空更新返回 `400103`。

### 3.4 删除（`DeleteReq`）

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `id` / `uuid` | int / string | 至少一个 |
| `hard` | bool | 默认 `false`（软删除）；`true` 物理删除 |

### 3.5 标签（`TagListReq` / `TagCreateReq`）

列表：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `page` / `page_size` | int | 分页 |
| `kind` | string | `general` / `color` / `style` / `scene` / `element` / `usage` / `format` |
| `keyword` | string | 名称模糊匹配 |
| `include_deleted` | bool | 包含已删除 |

创建：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `name` | string | **必填** |
| `kind` | string | 默认 `general`，非法值 `400104` |
| `source` | string | `manual` / `ai`，默认 `manual` |
| `color` | string | 颜色 |
| `sort` | int | 排序 |
| `uuid` | string | 可选 |

### 3.6 内容搜索（`FileSearchReq`）

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `query` | string | **必填** |
| `top_k` | int | 默认 5，范围 1–50 |
| `min_score` | number | 默认 0.2 |
| `type_uid` / `tag_uuid` | string | 结果过滤 |

返回 `{ list, total }`，每项为素材记录（含 `score`、`tags`）。

### 3.7 素材-标签关联

`file_tag_list`：

```json
{ "file_uuid": "..." }
```

`file_tag_set`：

```json
{
  "file_uuid": "...",
  "replace": true,
  "tags": [
    { "tag_uuid": "...", "source": "manual", "confidence": 0.9 }
  ]
}
```

- `file_uuid`、每个 `tag_uuid` 必填；
- `replace` 默认 `true`，先清空再写入；
- `source` 默认 `manual`；`confidence` 默认 1.0，范围 0–1；
- 返回该素材的最新标签列表。

---

## 4. 工具清单（24）

| 工具 | FFI-Symbol | 说明 | 输入 |
| --- | --- | --- | --- |
| `material_setup` | `setup` | 建 5 张表与 17 个索引（幂等） | 空 / `null` |
| `material_file_list` | `file_list` | 素材分页列表 | `ListReq` |
| `material_file_get` | `file_get` | 素材详情 | `LocateReq` |
| `material_file_create` | `file_create` | 新建素材（`name` 必填） | `CreateReq` |
| `material_file_update` | `file_update` | 更新素材（平铺字段） | `LocateReq` + 文件字段 |
| `material_file_delete` | `file_delete` | 软 / 硬删除素材 | `DeleteReq` |
| `material_file_search` | `file_search` | 语义搜索（调 chatllm） | `FileSearchReq` |
| `material_type_list` | `type_list` | 类型分页列表 | `ListReq` |
| `material_type_get` | `type_get` | 类型详情 | `LocateReq` |
| `material_type_create` | `type_create` | 新建类型（`type_name` 必填） | `CreateReq` |
| `material_type_update` | `type_update` | 更新类型 | `LocateReq` + `type_name` / `icon` |
| `material_type_delete` | `type_delete` | 删除类型 | `DeleteReq` |
| `material_version_list` | `version_list` | 版本分页列表 | `ListReq` |
| `material_version_get` | `version_get` | 版本详情 | `LocateReq` |
| `material_version_create` | `version_create` | 新建版本 | `CreateReq` |
| `material_version_update` | `version_update` | 更新版本 | `LocateReq` + 版本字段 |
| `material_version_delete` | `version_delete` | 删除版本 | `DeleteReq` |
| `material_tag_list` | `tag_list` | 标签分页列表 | `TagListReq` |
| `material_tag_get` | `tag_get` | 标签详情 | `LocateReq` |
| `material_tag_create` | `tag_create` | 新建标签（`name` 必填） | `TagCreateReq` |
| `material_tag_update` | `tag_update` | 更新标签 | `LocateReq` + 标签字段 |
| `material_tag_delete` | `tag_delete` | 删除标签 | `DeleteReq` |
| `material_file_tag_list` | `file_tag_list` | 读取素材标签 | `{file_uuid}` |
| `material_file_tag_set` | `file_tag_set` | 覆盖 / 追加素材标签 | `FileTagSetReq` |

---

## 5. MCP 工具 Schema 示例

### 5.1 列表工具

```json
{
  "name": "material_file_list",
  "description": "分页查询素材文件，支持类型、项目、标签、关键词过滤。",
  "inputSchema": {
    "type": "object",
    "properties": {
      "page": { "type": "integer", "minimum": 1, "default": 1 },
      "page_size": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 },
      "type_uid": { "type": "string" },
      "project_uid": { "type": "string" },
      "tag_uuid": { "type": "string" },
      "keyword": { "type": "string" },
      "include_deleted": { "type": "boolean", "default": false }
    },
    "additionalProperties": false
  }
}
```

### 5.2 详情 / 删除工具

```json
{
  "name": "material_file_delete",
  "description": "删除素材文件；默认软删除，hard=true 时物理删除。",
  "inputSchema": {
    "type": "object",
    "properties": {
      "id": { "type": "integer" },
      "uuid": { "type": "string" },
      "hard": { "type": "boolean", "default": false }
    },
    "anyOf": [
      { "required": ["id"] },
      { "required": ["uuid"] }
    ],
    "additionalProperties": false
  }
}
```

### 5.3 搜索工具

```json
{
  "name": "material_file_search",
  "description": "按语义搜索素材内容，返回带 score 的素材记录。",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query": { "type": "string", "minLength": 1 },
      "top_k": { "type": "integer", "minimum": 1, "maximum": 50, "default": 5 },
      "min_score": { "type": "number", "default": 0.2 },
      "type_uid": { "type": "string" },
      "tag_uuid": { "type": "string" }
    },
    "required": ["query"],
    "additionalProperties": false
  }
}
```

### 5.4 标签关联

```json
{
  "name": "material_file_tag_set",
  "description": "设置素材标签；replace=true 时先清空再写入。",
  "inputSchema": {
    "type": "object",
    "properties": {
      "file_uuid": { "type": "string", "minLength": 1 },
      "replace": { "type": "boolean", "default": true },
      "tags": {
        "type": "array",
        "items": {
          "type": "object",
          "properties": {
            "tag_uuid": { "type": "string", "minLength": 1 },
            "source": { "type": "string", "enum": ["manual", "ai"], "default": "manual" },
            "confidence": { "type": "number", "minimum": 0, "maximum": 1, "default": 1.0 }
          },
          "required": ["tag_uuid"],
          "additionalProperties": false
        }
      }
    },
    "required": ["file_uuid"],
    "additionalProperties": false
  }
}
```

---

## 6. 输出

统一外壳：

```json
{ "code": 200, "data": {}, "msg": "success" }
```

| 工具类型 | `data` |
| --- | --- |
| `setup` | `{ "tables": 5, "indexes": 17 }` |
| `*_list` | `{ "list": [], "page": 1, "page_size": 20, "total": 0 }` |
| `*_get` / `*_update` | 单条记录 |
| `*_create` | `{ "id": 1, "uuid": "..." }` |
| `*_delete` | `{ "deleted": true, "hard": false }` |
| `file_search` | `{ "list": [], "total": 0 }` |
| `file_tag_list` / `file_tag_set` | 标签数组 |

---

## 7. 实现指南

### 7.1 FFI 调用

```c
int post_file_list(const char *json_in, char *json_out);
```

Rust（`libloading`）示例：

```rust
let lib = unsafe { Library::new("material_lib.dll")? };
let f: Symbol<unsafe extern "C" fn(*const c_char, *mut c_char) -> i32> =
    unsafe { lib.get(b"post_file_list\0")? };
let input = CString::new(r#"{"page":1,"page_size":20}"#)?;
let mut out = vec![0 as c_char; 64 * 1024];
let ret = unsafe { f(input.as_ptr(), out.as_mut_ptr()) };
assert_eq!(ret, 0);
```

也可以直接依赖本 crate 的 `rlib`，调用导出的 `post_*` 函数。

### 7.2 缓冲区

- 固定 **64 KiB**（含 NUL）；
- `-2` 时不保证内容完整，应扩容重试或报内部错误；
- `-1` 表示 `json_out` 为空，属于 server 编码错误。

### 7.3 并发与事务

- 本库通过 `database.dll` 执行同步 SQL，调用会阻塞；
- MCP server 建议串行化写操作，或按素材 / 项目分片；
- `setup` 幂等，可在启动时调用一次；
- `file_tag_set` 的 `replace=true` 会先删除再插入，避免并发覆盖同一素材标签。

### 7.4 安全

- 调用方不接触 SQL；过滤字段与更新字段都在库内白名单校验；
- `file_search` 会把 `query` 交给 `chatllm` 做向量检索，不拼接 SQL；
- 记录内容（名称、描述、日志）可能包含用户数据，工具输出应遵循宿主的脱敏策略。

---

## 8. 资源与提示词（可选）

建议暴露：

| URI | 内容 |
| --- | --- |
| `material://schema` | 5 张表与字段说明（来自 `ui/material/db.md`） |
| `material://tags` | 标签字典（`tag_list` 的缓存） |

提示词示例：

```json
{
  "name": "find-material",
  "description": "按语义查找素材并返回可用标签",
  "arguments": [
    { "name": "query", "required": true },
    { "name": "top_k", "required": false }
  ]
}
```

---

## 9. 版本与兼容

| 项 | 值 |
| --- | --- |
| 包名 / 版本 | `material_lib` / 1.0.1 |
| 发布产物 | `material_lib.dll` |
| 端点数 | 24 |
| 平台 | Windows（`platform-exports.windows`） |
| 输出缓冲 | 64 KiB |

升级时：

- 新增工具保持向后兼容；
- 错误码只新增，不复用；
- 工具名与 FFI-Symbol 的映射保持稳定。

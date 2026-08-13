# Tusk 界面完善实施计划（参考 Navicat 及主流布局）

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** 把 Tusk 从单页查询工具完善为 Navicat 风格的 PostgreSQL 管理客户端（三栏布局 + 对象树 + 多标签工作区 + 表管理 + 连接管理）。

**Architecture:** Svelte 5 前端（布局/交互）+ Rust 后端（核心逻辑与 Tauri command 解耦，`src-tauri/src/lib.rs`）。后端持续暴露 `list_databases` / `list_tables` / `list_columns` / `paginate` 等 command；前端按 Navicat 主流布局组织：顶部工具栏 / 左侧对象树 / 中央多标签工作区 / 底部状态栏。

**Tech Stack:** Tauri 2.11 + Rust (tokio-postgres) + Svelte 5 + TypeScript + Vite。测试：`cargo test --lib`（Rust 核心）、`npm run check`（前端类型）、`npm run tauri dev`（手工验证）。

**当前状态（阶段 0 已完成并实测）：** 连接表单 + SQL 编辑器 + 结果表格，单页布局。Rust 侧有 `connect` / `disconnect` / `query` 三个 command，`open_connection` / `run_query` 为可测试核心。本地 PG 17 + 测试库 `tusk_demo`（products 表，7 列含 numeric/jsonb/timestamptz）。

---

## 阶段 1：Navicat 风格三栏布局骨架

目标：重构为「顶部工具栏 + 左侧对象树 + 中央标签页 + 底部状态栏」四区布局，连接后左侧树展示 库 → 表 → 字段。

### Task 1.1: Rust 侧新增 schema 查询 command

**Objective:** 提供查询库/表/字段的 command（基于 pg_catalog 系统表）

**Files:**
- Modify: `src-tauri/src/lib.rs`（新增 `DatabaseInfo`、`TableInfo`、`ColumnInfo` 复用、`list_databases`、`list_tables`、`list_columns` 三个核心函数 + command）

**Step 1: 先写失败测试**（追加到 `tests` 模块）

```rust
#[tokio::test]
async fn test_schema_queries() {
    let (client, _) = open_connection(&test_cfg()).await.expect("连接失败");
    let dbs = list_databases(&client).await.expect("list_databases 失败");
    assert!(dbs.iter().any(|d| d.name == "tusk_demo"), "应列出 tusk_demo");

    let tables = list_tables(&client, "tusk_demo").await.expect("list_tables 失败");
    assert!(tables.iter().any(|t| t.name == "products"));

    let cols = list_columns(&client, "tusk_demo", "products").await.expect("list_columns 失败");
    assert!(cols.iter().any(|c| c.name == "price" && c.type_name.contains("numeric")));
}
```

**Step 2: 运行验证失败** — `cargo test --lib test_schema_queries`，预期 FAIL（函数未定义）

**Step 3: 实现核心函数**

```rust
// 查数据库列表（排除系统库可选）
async fn list_databases(client: &Client) -> Result<Vec<DatabaseInfo>, String> {
    let rows = client.query(
        "SELECT datname FROM pg_database WHERE datistemplate = false ORDER BY datname", &[],
    ).await.map_err(|e| format!("查询数据库失败: {e}"))?;
    Ok(rows.iter().map(|r| DatabaseInfo { name: r.get(0) }).collect())
}
// list_tables: SELECT tablename FROM pg_tables WHERE schemaname='public' ORDER BY tablename
// list_columns: SELECT column_name, data_type, is_nullable, column_default
//   FROM information_schema.columns WHERE table_schema='public' AND table_name=$1 ORDER BY ordinal_position
```

**Step 4: 运行验证通过** — `cargo test --lib`，预期全部 PASS

**Step 5: 注册 command** — 三组 `#[tauri::command]` 薄封装（从 state 取 client），加入 `generate_handler!`

### Task 1.2: 前端四区布局 + 对象树

**Objective:** 重构 `+page.svelte` 为顶部工具栏 / 左侧树 / 中央标签页 / 底部状态栏；连接后自动加载对象树

**Files:**
- Modify: `src/routes/+page.svelte`（整体重写为四区布局）
- 新建组件建议：`src/lib/components/Toolbar.svelte`、`src/lib/components/Sidebar.svelte`、`src/lib/components/StatusBar.svelte`（如组件化拆分，需建 `src/lib/` 目录）

**关键行为：**
1. 顶部工具栏：连接下拉（显示当前连接）/ 新建查询 / 刷新对象树 / 断开
2. 左侧树：连接节点 → 数据库 → 表（点击展开字段），支持点击刷新；`invoke('list_databases')` → 点库 `invoke('list_tables')` → 点表 `invoke('list_columns')`
3. 中央工作区：保留现有 SQL 编辑器 + 结果表格，放进第一个标签页；标签栏可关闭
4. 底部状态栏：当前连接 + 服务器版本 + 结果行数/耗时
5. 连接成功后自动加载对象树；断开清空

**验证：** `npm run check` 0 错误；`npm run tauri dev` 连接本地库后左侧出现 tusk_demo → products → 7 个字段

### Task 1.3: 提交

```bash
git add -A && git commit -m "feat: Navicat 风格三栏布局 + 对象树（库/表/字段）"
```

---

## 阶段 2：表页签（数据 / 结构 / SQL 预览）

目标：双击左侧表 → 中央打开该表标签页，内含「数据 / 结构 / SQL预览」子标签。

### Task 2.1: Rust 分页查询 command

- 新增 `query_page(client, conn_id, sql, limit, offset) -> QueryResult`：在用户 SQL 后追加 `LIMIT $n OFFSET $m`（安全做法：prepare 后基于 columns 判断；简单做法：包装 `SELECT * FROM (sql) AS _t LIMIT x OFFSET y`，只对单 SELECT 生效）
- 或更简单：`paginate_table(conn_id, schema, table, limit, offset)` 专用 command，自动生成 `SELECT * FROM table LIMIT/OFFSET` + `SELECT count(*)` 返回总行数
- 测试：`test_pagination` 断言 limit/offset 生效、count 正确

### Task 2.2: 表标签页 UI

- 双击表打开标签：三个子标签
  - 数据：分页表格（上一页/下一页/共 N 行），列类型小字
  - 结构：字段名 / 类型 / 可空 / 默认值 / 主键标记（来自 list_columns 扩展）
  - SQL 预览：`SELECT * FROM "public"."products";` 只读展示 + 「在编辑器中打开」按钮
- 标签栏：活动标签高亮、× 关闭、中键关闭

**验证：** 双击 products → 数据页显示 4 行可翻页；结构页显示 7 列属性；SQL 预览正确

---

## 阶段 3：查询编辑器完善

目标：多标签查询 + 结果网格体验 + 消息面板。

### Task 3.1: 结果增强

- 显示执行耗时（前端计时 invoke 前后）
- 消息面板：非查询语句显示影响行数（已有）+ DDL 成功提示
- 结果行数上限提示（如 >1000 行提示缩小范围）

### Task 3.2: 多语句 + 历史

- Rust：`query` 支持按分号拆分逐条执行（简单 split 需防字符串内分号——用简单状态机或先支持「拆分 + 忽略空语句」，注明限制）
- 前端：SQL 历史下拉（本地 localStorage，最近 50 条），Ctrl+↑ 回看
- 快捷键：Cmd+Enter 执行、Cmd+N 新查询标签、Cmd+R 刷新

### Task 3.3: 提交

```bash
git commit -m "feat: 查询编辑器增强（耗时/消息/历史/多语句）"
```

---

## 阶段 4：连接管理

目标：保存连接配置，密码入 Keychain。

### Task 4.1: Rust 连接存储

- 连接配置 JSON 存 `~/Library/Application Support/com.tusk.app/connections.json`（名称/host/port/user/dbname），**密码不落盘**
- 新增 command：`save_connection` / `list_connections` / `delete_connection`
- 密码：`security add-generic-password -a <name> -s tusk -w <pw>`（macOS Keychain CLI），`security find-generic-password` 读取。Rust 侧用 `std::process::Command` 调 security（避免引额外 crate；后续可换 keyring crate）
- 测试：save/list/delete 往返（用临时目录覆盖路径，通过环境变量注入配置目录）

### Task 4.2: 连接管理 UI

- 工具栏连接按钮 → 弹出连接管理面板：已保存连接列表（点击即连）+ 新建/编辑/删除
- 连接表单增加「保存此连接」勾选
- 启动时加载已保存连接

### Task 4.3: 提交

```bash
git commit -m "feat: 连接管理（配置持久化 + Keychain 密码）"
```

---

## 阶段 5：数据编辑与导出

目标：网格内编辑、行级增删改、CSV 导出。

### Task 5.1: Rust 数据操作

- `update_cell(conn_id, schema, table, pk_cols, pk_vals, col, value)`：生成 `UPDATE ... SET col=$n WHERE pk=...`
- `insert_row` / `delete_row` 同理（基于主键；无主键表仅删除按行号回退限制，先支持有主键表）
- 值序列化：所有值走 `text` 转换 + `stmt.types()` 强转（复用类型映射思路）；先支持 text/numeric/int/bool/时间，jsonb 暂以字符串输入
- 测试：插入→更新→删除往返，断言行数变化与数据正确

### Task 5.2: 网格编辑 UI + 导出

- 数据页单元格双击进入编辑，回车提交（调用 update_cell），Esc 取消
- 行右键菜单：删除行 / 复制行
- 工具栏「导出 CSV」：前端把当前结果集拼 CSV 下载（tauri dialog save + fs 写入，或先复制到剪贴板）

### Task 5.3: 提交

```bash
git commit -m "feat: 数据编辑 + CSV 导出"
```

---

## 阶段 6：打磨

- 暗色/亮色主题切换、设置面板（默认 LIMIT、字体大小）
- SQL 格式化（引入轻量格式化 crate 或前端纯 JS 实现，评估后定）
- Explain 分析（`EXPLAIN (ANALYZE, BUFFERS) <sql>` 结果显示为文本/树）
- 应用图标（象牙 logo）、`npm run tauri build` 出 dmg 安装包
- 全局快捷键、窗口大小记忆

---

## 验证总纲

每个阶段完成标准：
1. `cargo test --lib` 全绿（新增逻辑必须有测试）
2. `npm run check` 0 errors 0 warnings
3. `npm run tauri dev` 手工验证关键交互（连接 → 树 → 表页签 → 查询 → 编辑）
4. `git commit`（每任务一次）

## 风险与权衡

- **多语句拆分**（阶段 3）：简单 split 会误伤字符串内分号，先实现「按语句类型探测 + 文档注明限制」，后续可换 `pgsql-parser`（pg_query crate，体积大）——评估后决定
- **Keychain CLI**（阶段 4）：`security` 命令交互式弹窗需 `-w` 静默写入，读取无需弹窗；若体验差换 `keyring` crate
- **无主键表编辑**（阶段 5）：不支持行级删除/定位更新，UI 置灰提示
- **jsonb 编辑**：先以 JSON 字符串输入并校验合法性，复杂编辑器后置
- **numeric 已实现精度无损**（自解码 wire format），数据编辑回写时同样走 text 转换

## 开放问题

1. 是否需要 MySQL/SQLite 等多数据库支持？（影响后端抽象，暂不引入，保持 PostgreSQL 单库聚焦）
2. 图标与品牌：象牙 logo 何时做？（阶段 6 或提前）
3. 是否需要设置面板？（阶段 6，若用户不需要可砍）

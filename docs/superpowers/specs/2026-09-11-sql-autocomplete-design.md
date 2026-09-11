# SQL 编辑器代码补全 设计文档

日期:2026-09-11
状态:已通过对话评审,待实施计划

## 1. 背景与目标

当前 SQL 编辑器是 `+page.svelte` 里的一个纯 `<textarea>`(`bind:value={activeTab.sql}`,`keydown` 只处理 Cmd+Enter 执行与 Cmd+↑/↓ 历史),没有语法高亮,也没有任何补全:写 `SELECT * FROM <table>` 时表名和列名全靠记忆。

目标:输入 SQL 时给出**上下文感知**的候选——`FROM/JOIN` 后提示表名,`SELECT/WHERE/ON` 等处提示字段,写了别名(`u.`)就提示该表的字段,同时覆盖 SQL 关键字与常用函数。

成功标准:

- 输入 `SELECT * FROM bo|` 能提示出当前库的表名。
- 输入 `SELECT u.| FROM book_pages u` 只提示 `book_pages` 的字段。
- 补全不打断既有输入习惯:Cmd+Enter 执行、Cmd+↑/↓ 历史、Enter 换行全部照旧。
- 补全失败或 schema 未加载时**静默降级**,永不阻塞输入、不弹错误。

### 非目标(本次明确不做)

- 语法高亮、括号自动闭合、多光标(需要 CodeMirror 级别的编辑器重构,单独作为后续需求)。
- 接受候选时自动补函数括号(如插入 `count()` 并把光标放进括号内)。
- 跨库/跨连接的 schema 提示(只提示当前查询页签所属连接的默认库)。
- SQL 语义分析级别的别名解析(如子查询、CTE、`JOIN ... USING` 的隐式别名推导)。

## 2. 架构与组件边界

新增三个源文件(测试文件见第 7 节),职责单一、可独立理解:

```
src/lib/sql-dialect.ts               纯常量:PG / SQLite 关键字 + 常用函数
src/lib/sql-complete.ts              纯函数补全引擎(无 DOM、无 IO)
src/lib/components/SqlEditor.svelte  textarea + 候选浮层 + 键位接管
```

### SqlEditor.svelte 对外契约

```ts
{
  value: string;                       // $bindable,双向绑定
  schema: {
    tables: string[];
    columns: Record<string, { name: string; type_name?: string }[]>;
  };
  dialect: 'postgresql' | 'sqlite' | 'merged';
  onkeydown?: (e: KeyboardEvent) => void;
}
```

- `+page.svelte` 只把第 1890 行的 `<textarea>` 替换成该组件,执行 / 历史 / 格式化 / Explain 链路**一行不改**。
- `keydown` 的 Cmd+Enter、Cmd+N、Cmd+↑/↓ 仍由 `+page.svelte` 处理;组件只在自己浮层打开时接管 ↑/↓/Tab/Esc,其余按键原样透传(无论是否接管,都继续调用 `props.onkeydown`)。
- 组件内部不调用任何 Tauri command;`complete()` 是同步纯函数,因此没有 loading 态、没有异步竞态。

### 数据流

```
+page.svelte
  ├─ tables[ck(conn,db)]        (现有:展开库时 list_tables)
  ├─ columnIndex[ck(conn,db)]   (新增:list_columns_bulk)
  └─ $derived schema ──┐
                       └→ <SqlEditor {schema} {dialect} bind:value={activeTab.sql} onkeydown={keydown} />
                              └─ 内部调 complete({text, caret, dialect, schema})
```

## 3. 数据来源

### 3.1 方言与目标库(不需要改 Rust)

前端已从 `list_connections` 拿到 `savedConns: { db_type, name, host, port, user, dbname }[]`,且 `connNodes[i].name === savedConns[i].name`。因此:

- **方言**:按活动查询页签的 `connId` → `connNodes` 找到 `name` → `savedConns` 的 `db_type` 映射为 `'postgresql'` / `'sqlite'`。
- **目标库**:同上取 `dbname`(PG 的连接绑定默认库,也是 `query` 命令实际执行的库)。取不到时(连接未保存)兜底用 `activeDb`。
- **兜底**:`db_type` 取不到时用 `'merged'`(PG ∪ SQLite 关键字并集),`dbname` 取不到且 `activeDb` 也为空时,表/字段候选为空,只提示关键字。

### 3.2 表名

复用现有 `tables[ck(conn, db)]`(`list_tables` 的结果,含 `kind: 'table' | 'view'`)。若目标库尚未加载,由 3.4 的懒加载补齐。

### 3.3 字段名:新增后端命令 `list_columns_bulk`(本次唯一的 Rust 改动)

现有 `list_columns` 每张表都要 `open_connection` 一次(`pg.rs:596` 的 `list_columns_core`),按表循环等于每表一次连接,不能用于整库预热。新增:

```rust
// lib.rs invoke_handler 注册
pub async fn list_columns_bulk(
    state: State<'_, AppState>,
    conn_id: String,
    dbname: String,
) -> Result<HashMap<String, Vec<SchemaColumn>>, String>
```

- 分发模式与现有 9 个通用 command 一致:`if entry.cfg.is_sqlite() { return sqlite::list_columns_bulk(&entry, &dbname).await; }`。
- **PG**:对目标库开**一次**连接,跑**一条**查询拿全库字段(类型表达式与 `list_columns_core` 保持一致):

  ```sql
  SELECT c.relname, a.attname,
         CASE WHEN a.atttypmod > 0 THEN
           CASE t.typname
             WHEN 'varchar' THEN 'varchar(' || (a.atttypmod - 4) || ')'
             WHEN 'numeric' THEN 'numeric(' || ((a.atttypmod - 4) >> 16) || ',' || ((a.atttypmod - 4) & 65535) || ')'
             ELSE t.typname END
         ELSE t.typname END,
         CASE WHEN NOT a.attnotnull THEN 'YES' ELSE 'NO' END,
         pg_get_expr(d.adbin, d.adrelid),
         col_description(a.attrelid, a.attnum)
  FROM pg_class c
  JOIN pg_namespace n ON n.oid = c.relnamespace
  JOIN pg_attribute a ON a.attrelid = c.oid
  JOIN pg_type t ON t.oid = a.atttypid
  LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
  WHERE n.nspname = 'public' AND c.relkind IN ('r','p','v')
    AND a.attnum > 0 AND NOT a.attisdropped
  ORDER BY c.relname, a.attnum
  ```

  对象集合刻意与 `list_tables_core`(`pg.rs:424`)完全一致——同样只取 `public` 下的 `r/p/v`、**不含物化视图**——否则字段索引里会出现 `tables[]` 里没有的表,补全就会给出点不开的表名。

  主键另跑一条 `pg_constraint`(`contype = 'p'`)查询,按 `relname` 分组后在内存里标记 `is_pk`,避免每表一条 PK 查询。
- **SQLite**:在同一条连接上循环 `pragma_table_info`,复用现有 `sqlite::list_columns` 的取值逻辑,`pk > 0` 即 `is_pk`。
- 返回值:`HashMap<String, Vec<SchemaColumn>>`,Tauri 序列化为 `{ [table]: SchemaColumn[] }`。`SchemaColumn` 模型直接复用,不新增 DTO。

### 3.4 加载时机与缓存

前端新增:

```ts
let columnIndex = $state<Record<string, Record<string, SchemaColumn[]>>>({}); // key: ck(conn, db)
let schemaLoading = $state<Record<string, boolean>>({});                      // 去重,防并发重复请求
```

`ensureSchema(conn, db)`:当某查询页签成为活动页签、且其目标库变化时触发。

1. `tables[ck(conn,db)]` 不存在 → 调 `list_tables` 补齐(与侧栏展开共用同一份缓存,不会重复请求)。
2. `columnIndex[ck(conn,db)]` 不存在且不在加载中 → 后台调 `list_columns_bulk`。
3. 任一步失败:静默,不写 `status`、不弹错误;补全自然降级(见第 6 节)。

两个请求都**不阻塞 UI**,`SqlEditor` 也不需要感知加载状态——schema 到了 `$derived` 会重算,候选自然变丰富。

副作用说明:目标库即使没在侧栏展开过,也会因为打开查询页签而加载表与字段,于是侧栏该库的行会顺带显示出表数量徽标。这是期望行为,不是缺陷。

### 3.5 缓存失效

在既有的 4 处 schema 变更点统一改为调用新增的 `invalidateSchema(conn, db)`:

- `+page.svelte:431` `refreshTables()`——刷新库表列表(建表/改表结构后走这里)
- `+page.svelte:667` `reloadDb()`——重新加载指定库的表列表
- `+page.svelte:767` `drop_database`——删库后清缓存(连同 `treeOpen`)
- `+page.svelte:955`——结构同步完成后清目标库缓存

```ts
function invalidateSchema(conn: string, db: string) {
  delete tables[ck(conn, db)];
  delete columnIndex[ck(conn, db)];
}
```

`disconnectConn`(`+page.svelte:158-161`)清理缓存时,同样要一并清掉 `columnIndex` 中该连接的所有 key。

## 4. 补全引擎规则(`sql-complete.ts`)

```ts
export type CompletionKind = 'keyword' | 'function' | 'table' | 'column';
export type CompletionItem = {
  label: string;            // 浮层里显示的文字
  kind: CompletionKind;
  detail?: string;
  insertText?: string;      // 实际插入的文字,缺省等于 label
};

export function complete(input: {
  text: string;
  caret: number;
  dialect: 'postgresql' | 'sqlite' | 'merged';
  schema: { tables: string[]; columns: Record<string, { name: string }[]> };
}): { items: CompletionItem[]; replaceFrom: number; replaceTo: number } | null;
```

### 4.1 替换范围

- `replaceTo = caret`;`replaceFrom` 从 caret 向左扫 `[A-Za-z0-9_$]`。
- 前缀是带引号的 `"..."` 时,向左扫到配对的双引号(`"` 转义写作 `""`),`replaceFrom` 指向开引号。此时候选的 `label` 仍是原名,但 `insertText` 为带引号且内部 `"` 双写的形式,接受后得到完整的 `"book_pages"`,不会把用户已经敲下的引号吞掉。
- 紧邻左侧是 `.` 时进入 **qualified 模式**:继续向左解析出限定符(`别名` 或 `表名`),`replaceFrom` 只覆盖点号之后的部分,保证 `u.na|` 替换为 `u.name` 而不是吞掉 `u.`。两种模式可叠加:`u."na|` 的限定符是 `u`,替换范围是引号内的 `"na`。

### 4.2 当前语句

从 caret 向左找最近的语句分隔 `;`,但**跳过字符串字面量 `'...'`(含 `''` 转义)与注释 `--` 行注释、`/* */` 块注释**。这是必须的:`WHERE x = 'a;b'` 里的分号不能截断语句。

### 4.3 上下文判定

对语句前缀自右向左扫 token,取遇到的第一个分类关键字:

| 触发 | 候选 |
| --- | --- |
| `FROM` `JOIN` `UPDATE` `INTO` `TABLE` | 表名 |
| `SELECT` `WHERE` `AND` `OR` `ON` `GROUP BY` `ORDER BY` `HAVING` `SET` `BY` `USING` `RETURNING` | 字段名(本语句涉及的表优先,其余表其次) |
| `xxx.`(qualified) | 该表的字段 |
| 其他(语句开头、`(` 或 `)` 之后等) | 关键字 + 函数 + 表名混排 |

qualified 模式的别名解析:在**当前语句**中用 `FROM|JOIN <表名> [AS] <别名>` 建立 别名→表 映射(大小写不敏感);限定符不是已知别名时,按表名直接匹配。解析不到就返回该位置可见的字段全集。

「本语句涉及的表」的含义同样来自当前语句 `FROM|JOIN` 后出现的表名:这些表的字段排在其余表的字段之前,但**两种都会出现**——写多表 JOIN 时不会因为解析漏了某张表就提示不出来。

### 4.4 过滤、排序与大小写

- case-insensitive 前缀匹配。
- 排序权重:字段/表 > 函数 > 关键字;同权重按「前缀匹配更靠前」再按字典序。
- 关键字与函数**一律大写**输出;表名与字段名按数据库中的原名输出。
- 触发门槛:普通位置前缀长度 ≥ 1 才弹;`xxx.` 后长度为 0 也弹。
- 候选上限 50 条。

### 4.5 不补全的情形

- 光标位于字符串字面量或注释内部 → 返回 `null`。
- 输入法组合期间(`compositionstart` 到 `compositionend` 之间)→ 不计算、不弹、不接管按键。

## 5. 交互与浮层

- 浮层绝对定位在 `.editor` 容器内(`position: relative`),`max-height: 200px` 可滚动,每个候选左侧用一个小标签区分类型(表 / 字段 / 函数 / 关键字),选中项用 `#1d2a44` 底 + `#4fc3f7` 文字。
- **光标坐标**:一个与 textarea 同字体、同字号、同行高、同 padding、同宽、同 `white-space: pre-wrap` 的隐藏镜像 `div`(绝对定位、`visibility: hidden`),内容为「caret 前文本 + 测量 `<span>` + caret 后文本」,取 span 的 `offsetLeft/offsetTop`,再减去 textarea 的 `scrollTop/scrollLeft`,浮层放在该行的下一行(`top + lineHeight`)。
- **翻转**:若浮层底部超出 textarea 可视高度,则改为向上弹出。
- **键位**(仅浮层打开时接管,均 `preventDefault`):

  | 键 | 行为 |
  | --- | --- |
  | ↑ / ↓ | 移动选中项 |
  | Tab | 接受当前候选 |
  | Esc | 关闭浮层 |
  | Enter | 关闭浮层,**不拦截**,照常换行 |
  | 鼠标点击候选 | 接受 |

- **接受**动作:`const ins = item.insertText ?? item.label`,然后 `value = value.slice(0, replaceFrom) + ins + value.slice(replaceTo)`,随后把 `selectionStart/selectionEnd` 设为插入文本末尾,并关闭浮层。
- 继续输入、移动光标、切换页签都重新计算或关闭浮层。
- 浮层配色沿用现有深色变量:底 `#1b1e25`、边 `#2c303a`、次要文字 `#6b7484`。

## 6. 降级与错误处理

按能力从强到弱逐级降级,任何一级失败都不产生用户可见错误:

1. 全量 schema 可用 → 关键字 + 函数 + 表名 + 字段名(含别名限定)。
2. `columnIndex` 未加载或加载失败 → 没有字段候选,只剩关键字 + 函数 + 表名。
3. `tables` 也未加载 → 只剩关键字 + 函数。
4. `complete()` 抛异常 → 组件 `try/catch` 后不弹浮层,输入不受影响。

组件不写 `status`、不弹 toast、不打断输入焦点。

## 7. 测试策略

前端目前没有任何 JS 测试框架,本次引入 `vitest`(devDependency)+ `"test": "vitest run"`,**只测纯函数**,不测 DOM 与浮层像素。

`src/lib/sql-complete.test.ts` 覆盖:

- 上下文:光标在 `FROM ` 后 → 表名;`SELECT | FROM users` → 字段;`SELECT u.| FROM users u` → `users` 的字段;`UPDATE |` / `JOIN |` → 表名。
- 字符串与注释:`WHERE x = 'a;b'` 后面的位置按**同一条语句**处理,不被 `;` 截断;光标在 `'...'` 或 `-- ...` 内 → 返回 `null`。
- 替换范围:`u.na|` 的 `replaceFrom` 只覆盖 `na`;带引号的 `"book_pa|` 的 `replaceFrom` 指向开引号且候选的 `insertText` 是 `"book_pages"`。
- 排序与大小写:前缀精确项排在前面;关键字输出大写、表名字段名保留原名;超过 50 条被截断。
- 降级:`schema.columns` 为空对象 → 不产生 `column` 类型候选。

Rust 侧:`list_columns_bulk` 按现有测试风格补测试(复用 `pg.rs` 测试模块的 `test_cfg()`,`tusk_demo` 库),断言返回的 map 覆盖预期表、字段数与 `is_pk` 正确;SQLite 侧用临时库断言(参考 `sqlite.rs:433` 现有 `list_columns` 测试)。与现有 30 个测试一起跑。

## 8. 影响面与验证步骤

改动文件:

| 文件 | 改动 |
| --- | --- |
| `src/lib/sql-dialect.ts` | 新增 |
| `src/lib/sql-complete.ts` | 新增 |
| `src/lib/sql-complete.test.ts` | 新增 |
| `src/lib/components/SqlEditor.svelte` | 新增 |
| `src/routes/+page.svelte` | 替换编辑器为组件;新增 `columnIndex`/`ensureSchema`/`invalidateSchema`/`schema` 派生;4 处失效点改调用;`disconnectConn` 清理新缓存 |
| `src-tauri/src/models.rs` | 不新增 DTO(复用 `SchemaColumn`) |
| `src-tauri/src/db/pg.rs` | 新增 `list_columns_bulk` + `list_columns_bulk_core` + 测试 |
| `src-tauri/src/db/sqlite.rs` | 新增 `list_columns_bulk` + 测试 |
| `src-tauri/src/lib.rs` | 注册 command |
| `package.json` | 新增 `vitest` devDependency 与 `test` 脚本 |

验证顺序(每步通过才进下一步):

1. `cargo test`(在 `src-tauri`)→ 全绿,含新增的 bulk 测试。
2. `npm test` → 引擎纯函数用例全绿。
3. `npm run check` → 0 error 0 warning。
4. `npm run build` → 构建通过。
5. 手动实测(`npm run tauri dev`):`SELECT * FROM bo|` 提示表;`SELECT u.| FROM book_pages u` 只提示该表字段;字符串里的 `;` 不干扰;Tab 接受、Enter 换行、Esc 关闭、Cmd+Enter 执行、Cmd+↑/↓ 历史均正常;断开连接后重新连接,补全仍可用。
6. 断网/无权限场景:字段加载失败时只剩关键字与表名候选,无错误弹窗。

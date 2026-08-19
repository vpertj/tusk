# Tusk 架构升级 + SQLite 支持 执行计划

> 原则:每阶段多次验证(编译/测试/check/build/浏览器实测),确认无误才进下一阶段。
> 每次变更 → 验证 → git commit + push。

## ✅ 阶段 1: Rust 模块化拆分(已完成 v1.1.0, commit 9f676bf)
lib.rs(4185 行)→ 薄壳(57 行)+ models.rs + state.rs + db/pg.rs。
- ✅ models.rs(DTO 16 个, ConnConfig/SavedConn 加 db_type/path)
- ✅ db/mod.rs + db/pg.rs(全部 PG 实现原样搬移,零逻辑改动)
- ✅ 验证:cargo test 26/26 全绿 + 零警告 + dev 实测

## ✅ 阶段 2: 前端组件化拆分(已完成, commit 4972275)
+page.svelte(5275 行)→ 容器(4516 行)+ components/。
- ✅ Header / Footer / ConnDialog / Sidebar 四个组件(props 契约)
- ✅ 每拆一个 svelte-check + 浏览器实测
- ✅ 全量回归 + 推送

## ✅ 阶段 3: 多库驱动架构 + SQLite 支持(已完成, commit 6600a5c)
- ✅ ConnEntry = client(Option<Arc<Client>>) + sqlite(Option<Arc<Mutex<Connection>>>) + cfg;PG 取连接用 entry.pg_client()?
- ✅ 9 个通用 command 分发:query/list_databases/list_tables/list_columns/paginate_table/update_cell/delete_row/insert_row/export_csv(模式:`if entry.cfg.is_sqlite() { return sqlite::xxx(...) }`)
- ✅ db/sqlite.rs:连接/查询/表/列/分页/编辑/导出 + TDD 1 test
- ✅ 前端:连接弹窗类型切换(SQLite 文件路径表单)+ 状态栏/徽标适配
- ✅ 27 tests 全绿(26 PG + 1 SQLite)

## ✅ 阶段 4: UI 优化(已完成, commit 1144d48)
- ✅ 欢迎引导页(数据库大图标 + 三步引导 + 新建连接 CTA)
- ✅ 图标统一:树/弹窗/页签/右键菜单 emoji → SVG 线性图标 + CSS 圆点状态灯 + PK 徽标

## ✅ 阶段 5: 发布(已完成, tag v1.1.0)
- ✅ 版本四处同步(package.json / Cargo.toml / tauri.conf.json / 前端 APP_VERSION)
- ✅ 27 tests + check 0/0 + 本地 dmg(hdiutil verify VALID)+ CI 构建成功 + Release 发布

---

# P1 功能路线(下一步)

## P1-1 数据导入 CSV(导出闭环,复用 COPY 流式)
- Rust:`import_csv_core`(解析 CSV → COPY 流式灌入目标表,列名映射,类型宽松)
- command `import_csv(connId, dbname, table, path)`
- 前端:表页签工具栏「导入」按钮 → 文件选择 → 预览前几行 → 确认导入
- TDD:临时表导入 → 断言行数/值
- SQLite 同步支持(简单 INSERT 循环)

## P1-2 查询历史(低成本高感知)
- localStorage 存最近 N 条,查询页签下拉历史(↑/↓ 已有部分)
- 设置面板可清空

## P1-3 SSH 隧道(远程刚需)
- russh 依赖,连接配置加 SSH 选项(host/port/user/key/pass)

## P1-4 多结果集页签(PG 多语句)
- MultiResult 已返回多结果,前端分页签展示

## P2(差异化):ER 图 / EXPLAIN 可视化 / 备份恢复(pg_dump)

## 验证清单(每阶段末必跑)
- cargo test 全绿(PG 26 + SQLite 1 + 新增)
- npm run check 0 errors 0 warnings
- npm run build 通过
- dev 实例浏览器实测(预览面板读 DOM/文本)
- git commit + push

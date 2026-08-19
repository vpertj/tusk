# Tusk 架构升级 + SQLite 支持 执行计划

> 原则:每阶段多次验证(编译/测试/check/build/浏览器实测),确认无误才进下一阶段。
> 每次变更 → 验证 → git commit + push。

## 阶段 1: Rust 模块化拆分(纯搬移,零逻辑改动)
目标:lib.rs(4185 行)→ 薄壳 + state/models + db/pg。
- [ ] 1.1 建 state.rs(models 的 DTO + AppState/ConnEntry/ConnConfig/SavedConn 等)搬移
- [ ] 1.2 建 db/mod.rs(DbKind 枚举占位) + db/pg.rs(全部 PG 实现原样搬移)
- [ ] 1.3 lib.rs 剩 tauri 入口 + command 注册 + invoke_handler
- [ ] 1.4 验证:`cargo test` 26/26 全绿 + `cargo build` 无警告 + dev 实例浏览器实测
- [ ] 1.5 git commit + push

## 阶段 2: 前端组件化拆分(纯搬移,零逻辑改动)
目标:+page.svelte(5275 行)→ 容器 + components/。
- [ ] 2.1 建 stores.js(共享 $state:conns/tabs/树缓存/弹窗开关)+ api.js(invoke 封装)
- [ ] 2.2 拆分简单弹窗(Confirm/NewDb/Row/Duplicate/Search/Settings/View/Sync/Designer)→ 每拆一个立即 svelte-check + 浏览器实测
- [ ] 2.3 拆分 Header/Footer/Sidebar
- [ ] 2.4 拆分 QueryTab/TableTab(最复杂,最后拆)
- [ ] 2.5 全量回归:cargo test + check 0/0 + build + dev 实测全部功能入口
- [ ] 2.6 git commit + push

## 阶段 3: 多库驱动架构 + SQLite 支持(P0 功能)
目标:db_type 真正生效,SQLite 可用。
- [ ] 3.1 db/mod.rs 分发层:连接/库列表/表列表/查询/分页/编辑/导出按类型分发
- [ ] 3.2 sqlite.rs:rusqlite 实现(连接文件、列表、查询、浏览、编辑、DDL 子集)
- [ ] 3.3 前端:连接面板类型选择生效(SQLite 选文件路径,隐藏端口/用户等无关字段)
- [ ] 3.4 SQLite 专属测试(TDD)+ PG 26 tests 回归不受影响
- [ ] 3.5 验证:两种类型完整链路实测(建库/建表/插入/查询/导出)
- [ ] 3.6 git commit + push

## 阶段 4: UI 高价值优化(视觉专业感)
- [ ] 4.1 图标统一:树/弹窗/状态点 emoji(🔌🗄📋👁🐘●○)→ SVG 线性图标 + CSS 圆点
- [ ] 4.2 空状态引导页:大图标 + 三步引导(新建连接 → 双击展开 → 新建查询)
- [ ] 4.3 细节:树 hover/缩进引导线、页签中键关闭、NULL 值样式
- [ ] 4.4 验证:浏览器实测截图 + 像素检查,确认无回归
- [ ] 4.5 git commit + push

## 阶段 5: 发布(版本四处同步)
- [ ] 5.1 版本号同步:package.json / Cargo.toml / tauri.conf.json / 前端 APP_VERSION
- [ ] 5.2 全量回归 + 本地 dmg 打包 + hdiutil verify + 打 tag 触发 CI
- [ ] 5.3 应用内更新实测

## 验证清单(每阶段末必跑)
- cargo test 全绿(阶段 3 后 = PG 26 + SQLite N)
- npm run check 0 errors 0 warnings
- npm run build 通过
- dev 实例浏览器实测(预览面板读 DOM/文本)
- git commit + push

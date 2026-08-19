<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
	import Header from '../lib/components/Header.svelte';
	import Footer from '../lib/components/Footer.svelte';
	import ConnDialog from '../lib/components/ConnDialog.svelte';
	import Sidebar from '../lib/components/Sidebar.svelte';
  import { listen } from '@tauri-apps/api/event';
  import { format as formatSql } from 'sql-formatter';
  import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window';

  // ===== 窗口大小记忆：启动恢复、resize 防抖保存 =====
  try {
    const saved = localStorage.getItem('tusk.winSize');
    if (saved) {
      const [w, h] = JSON.parse(saved);
      if (w > 400 && h > 300) {
        getCurrentWindow().setSize(new LogicalSize(w, h));
      }
    }
  } catch {
    // 恢复失败忽略
  }
  let resizeTimer: ReturnType<typeof setTimeout> | undefined;
  function onWinResize() {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(async () => {
      try {
        const size = await getCurrentWindow().innerSize();
        localStorage.setItem('tusk.winSize', JSON.stringify([size.width, size.height]));
      } catch {
        // 保存失败忽略
      }
    }, 500);
  }
  window.addEventListener('resize', onWinResize);
  window.addEventListener('keydown', onGlobalKeydown);

  // ================= 连接配置 =================
  let host = $state('localhost');
  let port = $state<number>(5432);
  let user = $state('');
  let password = $state('');
  let dbname = $state('postgres');
  let dbType = $state('postgres');
  let showConnPanel = $state(true);
  let connName = $state('');
  let saveConn = $state(true);
  let savedConns = $state<
    { db_type: string; name: string; host: string; port: number; user: string; dbname: string }[]
  >([]);

  async function loadSavedConns() {
    try {
      savedConns = await invoke('list_connections');
    } catch {
      savedConns = [];
    }
    syncConnNodes();
  }

  /** 用 savedConns + 当前连接重建连接树节点（保活已连接节点的状态） */
  function syncConnNodes() {
    const next: { id: string; name: string; host: string; port: number; connected: boolean; expanded: boolean }[] = [];
    for (const sc of savedConns) {
      const exist = connNodes.find((n) => n.name === sc.name);
      next.push({
        id: exist?.id ?? '',
        name: sc.name,
        host: sc.host,
        port: sc.port,
        connected: !!exist?.connected,
        expanded: exist?.expanded ?? false,
      });
    }
    // 当前活动连接若不在保存列表（手动连接），也挂一个节点
    if (connId && !next.some((n) => n.id === connId)) {
      next.push({
        id: connId,
        name: connMeta.user ? `${connMeta.user}@${connMeta.host}:${connMeta.port}` : '当前连接',
        host: connMeta.host,
        port: connMeta.port,
        connected: true,
        expanded: true,
      });
    }
    connNodes = next;
  }

  /** 连接树行点击：未连→连接；已连→展开/收起并聚焦 */
  async function connRowClick(c: { id: string; name: string }) {
    if (!c.id) {
      await connectByName(c.name);
      return;
    }
    connId = c.id;
    const node = connNodes.find((n) => n.id === c.id);
    if (node) node.expanded = !node.expanded;
    if (!connDbs[c.id]) {
      await loadDbsFor(c.id);
    }
  }

  /** 按保存连接名一键连接并挂入连接树 */
  async function connectByName(name: string): Promise<string> {
    try {
      const info = await invoke<{ id: string; version: string; user: string; host: string; port: number }>(
        'connect_saved',
        { name },
      );
      connId = info.id;
      version = info.version;
      connMeta = { user: info.user, host: info.host, port: info.port, version: info.version };
      status = `已连接 · ${info.user}@${info.host}:${info.port}`;
      const node = connNodes.find((n) => n.name === name);
      if (node) {
        node.id = info.id;
        node.connected = true;
        node.expanded = true;
      } else {
        syncConnNodes();
      }
      await loadDbsFor(info.id);
      return info.id;
    } catch (e) {
      status = `连接失败: ${e}`;
      throw e;
    }
  }

  /** 加载指定连接的库列表 */
  async function loadDbsFor(conn: string) {
    try {
      connDbs[conn] = await invoke<DatabaseInfo[]>('list_databases', { connId: conn });
    } catch (e) {
      status = `加载数据库失败: ${e}`;
    }
  }

  /** 断开连接：节点置灰 + 清缓存 + 关闭其页签 */
  async function disconnectConn(conn: string) {
    try {
      await invoke('disconnect', { connId: conn });
    } catch {
      /* 忽略 */
    }
    const node = connNodes.find((n) => n.id === conn);
    if (node) {
      node.connected = false;
      node.expanded = false;
      node.id = '';
    }
    delete connDbs[conn];
    for (const k of Object.keys(tables)) if (k.startsWith(conn + '::')) delete tables[k];
    for (const k of Object.keys(treeOpen)) if (k.startsWith(conn + '::')) delete treeOpen[k];
    for (const k of Object.keys(columns)) if (k.startsWith(conn + '::')) delete columns[k];
    // 关闭属于该连接的页签
    for (const t of tabs.filter((x) => x.connId === conn)) closeTab(t.id);
    if (connId === conn) {
      connId = '';
      connMeta = { user: '', host: '', port: 0, version: '' };
      status = '已断开连接';
    }
  }

  /** 连接右键菜单 */
  let connMenu = $state<{ x: number; y: number; name: string; id: string; connected: boolean } | null>(null);

  function openConnMenu(e: MouseEvent, c: { id: string; name: string; connected: boolean }) {
    e.preventDefault();
    e.stopPropagation();
    connMenu = { x: e.clientX, y: e.clientY, name: c.name, id: c.id, connected: c.connected };
  }

  // ================= 连接状态 =================
  let connId = $state('');
  let version = $state('');
  /** 连接元信息（底部状态栏绿色显示） */
  let connMeta = $state<{ user: string; host: string; port: number; version: string }>({
    user: '',
    host: '',
    port: 0,
    version: '',
  });
  let connecting = $state(false);
  let status = $state('未连接');

  // ================= 对象树 =================
  // 左侧树宽度（默认 260，可拖拽，记忆到本地）
  const savedW =
    typeof localStorage !== 'undefined'
      ? Number(localStorage.getItem('tusk.sidebarWidth'))
      : NaN;
  let sidebarWidth = $state(
    Number.isFinite(savedW) && savedW >= 160 && savedW <= 480 ? savedW : 260,
  );
  $effect(() => {
    localStorage.setItem('tusk.sidebarWidth', String(sidebarWidth));
  });

  function startSidebarResize(e: MouseEvent) {
    e.preventDefault();
    const startX = e.clientX;
    const startW = sidebarWidth;
    const onMove = (ev: MouseEvent) => {
      sidebarWidth = Math.max(160, Math.min(480, startW + (ev.clientX - startX)));
    };
    const onUp = () => {
      document.removeEventListener('mousemove', onMove);
      document.removeEventListener('mouseup', onUp);
      document.body.style.cursor = '';
    };
    document.body.style.cursor = 'col-resize';
    document.addEventListener('mousemove', onMove);
    document.addEventListener('mouseup', onUp);
  }

  interface DatabaseInfo {
    name: string;
  }

  interface TableInfo {
    name: string;
    kind: string; // "table" | "view"
  }
  interface SchemaColumn {
    name: string;
    type_name: string;
    is_nullable: string;
    default: string | null;
    comment?: string | null;
    is_pk: boolean;
  }
  interface IndexInfo {
    name: string;
    columns: string;
    is_unique: boolean;
  }
  let dbs = $state<DatabaseInfo[]>([]);

  // ===== 表设计器（建表） =====
  const DESIGNER_TYPES = [
    'serial',
    'bigserial',
    'int4',
    'int8',
    'text',
    'varchar',
    'numeric',
    'float8',
    'bool',
    'date',
    'timestamp',
    'timestamptz',
    'time',
    'jsonb',
    'uuid',
    'bytea',
  ];
  let showDesigner = $state(false);
  let editingTable = $state<{ conn: string; db: string; table: string } | null>(null);
  let designerDb = $state('');
  /** 当前活动库：展开库/打开表时记录，新建表/视图默认目标 */
  let activeDb = $state('');
  let designerName = $state('');
  let designerComment = $state('');
  let designerError = $state('');
  let designerSeq = 0;
  interface DesignerCol {
    id: number;
    name: string;
    baseType: string;
    length: string;
    nullable: boolean;
    default: string;
    comment: string;
    isPk: boolean;
    isSerial: boolean;
  }
  let designerCols = $state<DesignerCol[]>([
    { id: 1, name: 'id', baseType: 'serial', length: '', nullable: false, default: '', isPk: true, isSerial: true, comment: '' },
    { id: 2, name: 'name', baseType: 'text', length: '', nullable: false, default: '', isPk: false, isSerial: false, comment: '' },
  ]);

  function openDesigner() {
    editingTable = null;
    designerDb = activeDb || (connDbs[connId]?.[0]?.name ?? dbname);
    designerName = '';
    designerError = '';
    designerCols = [
      { id: ++designerSeq, name: 'id', baseType: 'serial', length: '', nullable: false, default: '', isPk: true, isSerial: true, comment: '' },
      { id: ++designerSeq, name: 'name', baseType: 'text', length: '', nullable: false, default: '', isPk: false, isSerial: false, comment: '' },
    ];
    showDesigner = true;
  }

  // 打开已有表的设计器（预填当前结构）
  async function openDesignerForEdit(conn: string, db: string, table: string) {
    try {
      const cols = await invoke<SchemaColumn[]>('list_columns', { connId: conn, dbname: db, table });
      editingTable = { conn, db, table };
      designerDb = db;
      designerName = table;
      designerComment = '';
      designerError = '';
      const typeMap: Record<string, string> = {
        int4: 'int4', int8: 'int8', text: 'text', varchar: 'varchar', numeric: 'numeric',
        float8: 'float8', bool: 'bool', date: 'date', timestamp: 'timestamp',
        timestamptz: 'timestamptz', time: 'time', jsonb: 'jsonb', uuid: 'uuid', bytea: 'bytea',
      };
      designerCols = cols.map((c) => {
        const m = c.type_name.match(/^(\w+)\((.+)\)$/);
        let baseType = c.type_name;
        let length = '';
        if (m) {
          baseType = m[1];
          length = m[2];
        }
        const isSerial = (c.default ?? '').includes('nextval(');
        if (isSerial) {
          baseType = c.default!.includes('bigint') || c.default!.includes('bigserial') ? 'bigserial' : 'serial';
        } else {
          baseType = typeMap[baseType] ?? baseType;
        }
        return {
          id: ++designerSeq,
          name: c.name,
          baseType,
          length,
          nullable: c.is_nullable === 'YES',
          default: isSerial ? '' : (c.default ?? ''),
          comment: c.comment ?? '',
          isPk: c.is_pk,
          isSerial,
        };
      });
      showDesigner = true;
    } catch (e) {
      status = `加载表结构失败: ${e}`;
    }
  }

  function addDesignerCol() {
    designerCols = [
      ...designerCols,
      { id: ++designerSeq, name: '', baseType: 'text', length: '', nullable: true, default: '', isPk: false, isSerial: false, comment: '' },
    ];
  }

  function delDesignerCol(id: number) {
    if (designerCols.length <= 1) return;
    designerCols = designerCols.filter((c) => c.id !== id);
  }

  function buildColType(c: DesignerCol): string {
    if (c.baseType === 'varchar') return c.length.trim() ? `varchar(${c.length.trim()})` : 'varchar';
    if (c.baseType === 'numeric') return c.length.trim() ? `numeric(${c.length.trim()})` : 'numeric';
    return c.baseType;
  }

  async function doCreateTable() {
    if (!connId || !designerName.trim()) {
      designerError = '请填写表名';
      return;
    }
    const cols = designerCols.map((c) => ({
      name: c.name.trim(),
      col_type: buildColType(c),
      nullable: c.nullable,
      default: c.default.trim() === '' ? null : c.default.trim(),
      comment: c.comment.trim() === '' ? null : c.comment.trim(),
      is_pk: c.isPk,
      is_serial: c.isSerial || c.baseType === 'serial' || c.baseType === 'bigserial',
    }));
    if (cols.some((c) => !c.name)) {
      designerError = '字段名不能为空';
      return;
    }
    try {
      if (editingTable) {
        await invoke('alter_table', {
          connId,
          dbname: editingTable.db,
          table: editingTable.table,
          columns: cols,
          tableComment: designerComment.trim() === '' ? null : designerComment.trim(),
        });
        // 刷新已打开的表页签（结构 + 数据）
        const openTab = tabs.find(
          (t) => t.kind === 'table' && t.dbname === editingTable!.db && t.table === editingTable!.table,
        );
        if (openTab) {
          loadStructure(openTab);
          loadTablePage(openTab);
        }
        await refreshTables(editingTable.conn, editingTable.db);
        const ck = `${editingTable.db}.${editingTable.table}`;
        if (columns[ck]) {
          columns[ck] = await invoke<SchemaColumn[]>('list_columns', {
            connId,
            dbname: editingTable.db,
            table: editingTable.table,
          });
        }
      } else {
        await invoke('create_table', {
          connId,
          dbname: designerDb,
          table: designerName.trim(),
          columns: cols,
          tableComment: designerComment.trim() === '' ? null : designerComment.trim(),
        });
        await refreshTables(connId, designerDb);
        openTableTab(connId, designerDb, designerName.trim());
      }
      showDesigner = false;
      editingTable = null;
    } catch (e) {
      designerError = String(e);
    }
  }

  // 刷新库的表列表（保持展开状态）
  async function refreshTables(conn: string, db: string) {
    try {
      tables[ck(conn, db)] = await invoke<TableInfo[]>('list_tables', { connId: conn, dbname: db });
    } catch (e) {
      status = `刷新表失败: ${e}`;
    }
  }

  // ===== 数据筛选 =====
  const FILTER_OPS = ['=', '!=', '>', '<', '>=', '<=', 'LIKE', 'ILIKE', 'IS NULL', 'IS NOT NULL'];
  let filterSeq = 0;

  function addFilter(raw: QueryTab) {
    const t = resolveTab(raw);
    if (!t.filters) t.filters = [];
    const firstCol = (t.structure ?? [])[0]?.name ?? '';
    t.filters.push({ id: ++filterSeq, column: firstCol, op: '=', value: '' });
  }

  function removeFilter(raw: QueryTab, id: number) {
    const t = resolveTab(raw);
    t.filters = (t.filters ?? []).filter((f) => f.id !== id);
  }

  async function applyFilters(raw: QueryTab) {
    const t = resolveTab(raw);
    const valid = (t.filters ?? []).filter(
      (f) =>
        f.column &&
        (f.op === 'IS NULL' || f.op === 'IS NOT NULL' || f.value.trim() !== ''),
    );
    t.filters = valid;
    t.filterActive = valid.length > 0;
    t.page = 1;
    await loadTablePage(t);
  }

  function clearFilters(raw: QueryTab) {
    const t = resolveTab(raw);
    t.filters = [];
    t.filterActive = false;
    t.page = 1;
    loadTablePage(t);
  }

  // 导出表数据为 SQL（INSERT 语句）到 ~/Downloads
  async function exportTableSql(raw: QueryTab) {
    const t = resolveTab(raw);
    if (!t.dbname || !t.table) return;
    t.loading = true;
    try {
      const path = await invoke<string>('export_sql', {
        connId,
        dbname: t.dbname,
        table: t.table,
      });
      t.message = `已导出 SQL → ${path}`;
    } catch (e) {
      t.error = String(e);
    }
    t.loading = false;
  }

  // 查询结果导出 CSV 到 ~/Downloads
  async function exportQueryCsv(raw: QueryTab) {
    const t = resolveTab(raw);
    const res = t.results?.find((r) => r.rows.length > 0) ?? t.results?.[0];
    if (!res || !res.columns.length) {
      t.error = '没有可导出的结果';
      return;
    }
    const esc = (v: unknown) => {
      if (v === null || v === undefined) return '';
      const s = String(v);
      return /[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
    };
    const lines = [
      res.columns.map((c) => esc(c.name)).join(','),
      ...res.rows.map((r) => r.map(esc).join(',')),
    ];
    const ts = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
    const path = `~/Downloads/tusk-query-${ts}.csv`;
    try {
      await invoke('write_text_file', { path, content: lines.join('\n') });
      t.message = `已导出 CSV → ${path}`;
    } catch (e) {
      t.error = String(e);
    }
  }

  // ===== 新增行弹窗（填值插入） =====
  let insertDialog = $state<{
    tabId: number;
    cols: SchemaColumn[];
    values: Record<string, string>;
    err: string;
  } | null>(null);

  function openInsertDialog(raw: QueryTab) {
    const t = resolveTab(raw);
    const cols = t.structure ?? [];
    if (!cols.length) {
      t.error = '无字段信息，请先刷新';
      return;
    }
    const values: Record<string, string> = {};
    for (const c of cols) {
      if (c.is_pk && (c.default ?? '').includes('nextval(')) {
        values[c.name] = ''; // serial 主键自动生成
      } else if (c.default) {
        values[c.name] = c.default.includes('nextval(') ? '' : (c.default ?? '');
      } else {
        values[c.name] = '';
      }
    }
    insertDialog = { tabId: t.id, cols, values, err: '' };
  }

  async function doInsertRow() {
    const d = insertDialog;
    if (!d) return;
    // 用弹窗打开时的表（用户可能已切换标签）
    const t = resolveTab({ id: d.tabId } as QueryTab);
    if (!t || t.kind !== 'table' || !t.dbname || !t.table) {
      d.err = '目标表已关闭，请重新打开后重试';
      return;
    }
    const vals = d.cols
      .filter((c) => (d.values[c.name] ?? '').trim() !== '')
      .map((c) => ({ name: c.name, value: d.values[c.name].trim() }));
    if (!vals.length) {
      d.err = '请至少填写一个字段值';
      return;
    }
    try {
      const newId = await invoke<number>('insert_row_vals', {
        connId,
        dbname: t.dbname,
        table: t.table,
        values: vals,
      });
      insertDialog = null;
      // 跳到最后一页（新行在主键排序末尾）再刷新
      await loadTablePage(t);
      const lastPage = Math.max(1, Math.ceil((t.total ?? 1) / t.pageSize!));
      t.page = lastPage;
      await loadTablePage(t);
      t.message = `已插入第 ${newId} 行`;
    } catch (e) {
      d.err = String(e);
    }
  }

  // 删除表确认弹窗（WKWebView 不支持 window.confirm，必须自定义）
  let confirmDrop = $state<{ conn: string; db: string; table: string; kind?: string } | null>(null);

  // ===== 表右键菜单 + 复制表 =====
  let tableMenu = $state<{ x: number; y: number; conn: string; db: string; table: string; kind: string } | null>(
    null,
  );
  /** 库级右键菜单 */
  let dbMenu = $state<{ x: number; y: number; conn: string; db: string } | null>(null);
  let dupDialog = $state<{
    conn: string;
    db: string;
    table: string;
    withData: boolean;
    name: string;
    err: string;
  } | null>(null);

  function openTableMenu(e: MouseEvent, conn: string, db: string, table: string, kind = 'table') {
    e.preventDefault();
    e.stopPropagation();
    tableMenu = { x: e.clientX, y: e.clientY, conn, db, table, kind };
  }

  /** 树空白区右键：新建数据库 / 刷新全部 */
  let blankMenu = $state<{ x: number; y: number } | null>(null);

  function openBlankMenu(e: MouseEvent) {
    if (!connId) return;
    e.preventDefault();
    e.stopPropagation();
    blankMenu = { x: e.clientX, y: e.clientY };
  }

  /** 在指定库新建查询编辑器 */
  function newQueryIn(conn: string, db: string) {
    dbMenu = null;
    activeDb = db;
    connId = conn;
    const t = newTab('');
    t.dbname = db;
    tabs.push(t);
    activeTabId = t.id;
  }

  /** 从树导出表 SQL */
  async function exportTableSqlFromTree(conn: string, db: string, table: string) {
    tableMenu = null;
    try {
      const path = await invoke<string>('export_sql', { connId: conn, dbname: db, table });
      status = `已导出 SQL → ${path}`;
    } catch (e) {
      status = `导出失败: ${e}`;
    }
  }

  function openDbMenu(e: MouseEvent, conn: string, db: string) {
    e.preventDefault();
    e.stopPropagation();
    dbMenu = { x: e.clientX, y: e.clientY, conn, db };
  }

  /** 在指定库新建表（右键库 → 在此库新建表） */
  function createTableIn(conn: string, db: string) {
    dbMenu = null;
    activeDb = db;
    connId = conn;
    openDesigner();
    designerDb = db;
  }

  /** 在指定库新建视图 */
  function createViewIn(conn: string, db: string) {
    dbMenu = null;
    activeDb = db;
    connId = conn;
    openViewDialog();
    viewDialog = { conn: connId, db, name: '', sql: 'SELECT\n  *\nFROM\n  "public"."表名"', err: '' };
  }

  /** 重新加载指定库的表列表 */
  async function reloadDb(conn: string, db: string) {
    dbMenu = null;
    loadingKey = ck(conn, db);
    try {
      tables[ck(conn, db)] = await invoke<TableInfo[]>('list_tables', { connId: conn, dbname: db });
    } catch (e) {
      status = `加载表失败: ${e}`;
    }
    loadingKey = '';
  }

  async function doDuplicate() {
    const d = dupDialog;
    if (!d) return;
    if (!d.name.trim()) {
      d.err = '新表名不能为空';
      return;
    }
    try {
      await invoke('duplicate_table', {
        connId: d.conn,
        dbname: d.db,
        srcTable: d.table,
        newTable: d.name.trim(),
        withData: d.withData,
      });
      dupDialog = null;
      await refreshTables(d.conn, d.db);
      openTableTab(d.conn, d.db, d.name.trim());
    } catch (e) {
      d.err = String(e);
    }
  }

  // 删除视图（对象树入口）
  function dropViewFromTree(conn: string, db: string, view: string) {
    confirmDrop = { conn, db, table: view, kind: 'view' };
  }

  // ===== 新建视图 =====
  let showViewDialog = $state(false);
  let viewDialog = $state<{ conn: string; db: string; name: string; sql: string; err: string } | null>(null);

  function openViewDialog() {
    viewDialog = { conn: connId, db: activeDb || (connDbs[connId]?.[0]?.name ?? dbname), name: '', sql: 'SELECT\n  *\nFROM\n  "public"."表名"', err: '' };
    showViewDialog = true;
  }

  async function doCreateView() {
    const d = viewDialog;
    if (!d) return;
    if (!d.name.trim()) {
      d.err = '视图名不能为空';
      return;
    }
    try {
      await invoke('create_view', {
        connId: d.conn,
        dbname: d.db,
        viewName: d.name.trim(),
        selectSql: d.sql,
      });
      showViewDialog = false;
      await refreshTables(d.conn, d.db);
      openTableTab(d.conn, d.db, d.name.trim());
    } catch (e) {
      d.err = String(e);
    }
  }

  // 删除表（对象树入口）
  function dropTableFromTree(conn: string, db: string, table: string) {
    confirmDrop = { conn, db, table };
  }

  // 删除表（表页签工具栏）
  function dropTable(raw: QueryTab) {
    const t = resolveTab(raw);
    confirmDrop = { conn: t.connId, db: t.dbname!, table: t.table! };
  }

  async function doDropConfirm() {
    const cd = confirmDrop;
    if (!cd) return;
    confirmDrop = null;
    try {
      if (cd.kind === 'view') {
        await invoke('drop_view', { connId: cd.conn, dbname: cd.db, viewName: cd.table });
      } else if (cd.kind === 'connection') {
        await invoke('delete_connection', { name: cd.db });
        // 若已连接先断开
        if (cd.conn && connNodes.some((n) => n.id === cd.conn)) {
          await disconnectConn(cd.conn);
        } else {
          connNodes = connNodes.filter((n) => n.name !== cd.db);
        }
        await loadSavedConns();
        return;
      } else if (cd.kind === 'database') {
        await invoke('drop_database', { connId: cd.conn, name: cd.db });
        // 关闭该库的所有页签 + 清缓存
        for (const t of [...tabs]) {
          if (t.connId === cd.conn && t.dbname === cd.db) closeTab(t.id);
        }
        delete tables[ck(cd.conn, cd.db)];
        delete treeOpen[ck(cd.conn, cd.db)];
        if (connDbs[cd.conn]) connDbs[cd.conn] = connDbs[cd.conn].filter((d) => d.name !== cd.db);
        return;
      } else {
        await invoke('drop_table', { connId: cd.conn, dbname: cd.db, table: cd.table });
      }
      const openTab = tabs.find(
        (t) => t.kind === 'table' && t.connId === cd.conn && t.dbname === cd.db && t.table === cd.table,
      );
      if (openTab) closeTab(openTab.id);
      await refreshTables(cd.conn, cd.db);
    } catch (e) {
      status = `删除失败: ${e}`;
    }
  }
  let treeOpen = $state<Record<string, boolean>>({}); // key: connId::db / connId::db.table
  let tables = $state<Record<string, TableInfo[]>>({});
  /** 连接树节点（Navicat 式多连接） */
  let connNodes = $state<
    { id: string; name: string; host: string; port: number; connected: boolean; expanded: boolean }[]
  >([]);
  /** 每个连接的库列表 */
  let connDbs = $state<Record<string, DatabaseInfo[]>>({});
  /** 缓存 key 前缀（连接隔离） */
  function ck(conn: string, key: string) {
    return `${conn}::${key}`;
  }
  let columns = $state<Record<string, SchemaColumn[]>>({});
  let loadingKey = $state('');

  // ===== 对象搜索（Cmd+F） =====
  let searchOpen = $state(false);
  let searchQuery = $state('');

  function searchResults() {
    const q = searchQuery.trim().toLowerCase();
    if (!q) return [];
    const out: { conn: string; db: string; table: string; kind: string }[] = [];
    for (const c of connNodes) {
      if (!c.connected) continue;
      for (const db of connDbs[c.id] ?? []) {
        const dbs_match = db.name.toLowerCase().includes(q);
        for (const t of tables[ck(c.id, db.name)] ?? []) {
          if (dbs_match || t.name.toLowerCase().includes(q)) {
            out.push({ conn: c.name, db: db.name, table: t.name, kind: t.kind });
            if (out.length >= 50) return out;
          }
        }
      }
    }
    return out;
  }

  function openSearchResult(r: { conn: string; db: string; table: string }) {
    openTableTab(r.conn, r.db, r.table);
    searchOpen = false;
    searchQuery = '';
  }

  // 全局快捷键：Cmd/Ctrl+F 打开搜索
  function onGlobalKeydown(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'f') {
      if (!connId) return;
      e.preventDefault();
      searchOpen = true;
    }
  }

  // ===== 设置（每页行数等，localStorage 持久化） =====
  let showSettings = $state(false);
  let settingsTab = $state<'general' | 'update' | 'about'>('general');
  let settingsPageSize = $state(50);
  try {
    settingsPageSize = Number(localStorage.getItem('tusk.pageSize')) || 50;
  } catch {
    // 忽略
  }

  function openSettings() {
    settingsPageSize = Number(localStorage.getItem('tusk.pageSize')) || 50;
    settingsTab = 'general';
    showSettings = true;
  }

  function saveSettings() {
    const n = Math.max(10, Math.min(500, Math.floor(settingsPageSize) || 50));
    settingsPageSize = n;
    localStorage.setItem('tusk.pageSize', String(n));
    // 已打开的表页签分页大小同步
    for (const t of tabs) {
      if (t.kind === 'table') t.pageSize = n;
    }
    showSettings = false;
    status = `每页行数已设为 ${n}`;
  }

  // ===== 结构/数据同步（Navicat） =====
  let showSync = $state(false);
  let syncMode = $state<'schema' | 'schema_data'>('schema');
  let syncSrcConn = $state('__current__');
  let syncDstConn = $state('__current__');
  let syncSrc = $state('');
  let syncDst = $state('');
  let syncDiffs = $state<{ table: string; action: string; sql: string; checked: boolean }[]>([]);
  let syncError = $state('');
  let syncMsg = $state('');
  let syncBusy = $state(false);

  function openSyncDialog() {
    if (dbs.length < 2) {
      status = '至少需要 2 个数据库才能同步';
      return;
    }
    syncSrc = connDbs[connId]?.[0]?.name ?? '';
    syncDst = connDbs[connId]?.[1]?.name ?? '';
    syncSrcConn = '__current__';
    syncDstConn = '__current__';
    syncDiffs = [];
    syncError = '';
    syncMsg = '';
    syncMode = 'schema';
    showSync = true;
  }

  /** 解析连接选择：__current__ 用当前连接，否则按保存连接名一键连接并返回 connId */
  async function resolveConn(sel: string): Promise<string> {
    if (sel === '__current__') return connId;
    const info = await invoke<{ id: string }>('connect_saved', { name: sel });
    return info.id;
  }

  async function doCompare() {
    if (!syncSrc || !syncDst) return;
    if (syncSrc === syncDst && syncSrcConn === syncDstConn) {
      syncError = '源库与目标库不能相同';
      return;
    }
    syncBusy = true;
    syncError = '';
    syncMsg = '';
    try {
      const srcId = await resolveConn(syncSrcConn);
      const dstId = await resolveConn(syncDstConn);
      const diffs = await invoke<{ table: string; action: string; sql: string }[]>('compare_schemas', {
        srcConnId: srcId,
        dstConnId: dstId,
        srcDb: syncSrc,
        dstDb: syncDst,
      });
      syncDiffs = diffs.map((d) => ({ ...d, checked: true }));
      if (!diffs.length) syncMsg = '两库结构完全一致 ✅';
    } catch (e) {
      syncError = String(e);
    }
    syncBusy = false;
  }

  async function doSyncExecute() {
    const sel = syncDiffs.filter((d) => d.checked);
    if (!sel.length) return;
    syncBusy = true;
    syncError = '';
    syncMsg = '';
    try {
      const srcId = await resolveConn(syncSrcConn);
      const dstId = await resolveConn(syncDstConn);
      let ok = 0;
      for (const d of sel) {
        await invoke('execute_sql', { connId: dstId, dbname: syncDst, sql: d.sql });
        ok++;
      }
      if (syncMode === 'schema_data') {
        // 结构+数据：COPY 流式同步勾选表的数据（全量覆盖目标表）
        const tables = sel.map((d) => d.table);
        const [synced, rows] = await invoke<[number, number]>('sync_data', {
          srcConnId: srcId,
          dstConnId: dstId,
          srcDb: syncSrc,
          dstDb: syncDst,
          tables,
        });
        syncMsg = `✅ 结构 ${ok} 项 + 数据 ${synced} 表 ${rows} 行已同步`;
      } else {
        syncMsg = `✅ 已执行 ${ok} 项差异，目标库结构已同步`;
      }
      syncDiffs = [];
      // 清目标库表缓存，强制树重新加载（否则显示旧数据）
      delete tables[syncDst];
      delete treeOpen[syncDst];
      await loadDbs();
    } catch (e) {
      syncError = String(e);
    }
    syncBusy = false;
  }

  // ===== 检查更新（GitHub Releases + 自动下载安装） =====
  const APP_VERSION = '1.0.4';
  let updateInfo = $state<{
    version: string;
    notes: string;
    url: string;
    assetUrl: string;
  } | null>(null);
  let updateCheckMsg = $state('');
  let updateBusy = $state(false);
  let updatePercent = $state(0);

  function verCmp(a: string, b: string): number {
    const pa = a.replace(/^v/i, '').split('.').map(Number);
    const pb = b.replace(/^v/i, '').split('.').map(Number);
    for (let i = 0; i < 3; i++) {
      const x = pa[i] ?? 0;
      const y = pb[i] ?? 0;
      if (x !== y) return x - y;
    }
    return 0;
  }

  async function checkUpdate(silent = false) {
    updateCheckMsg = '';
    try {
      const rel = await invoke<{
        tag_name: string;
        body: string;
        html_url: string;
        asset_url: string;
      }>('check_update');
      const latest = (rel.tag_name ?? '').replace(/^v/i, '');
      if (verCmp(latest, APP_VERSION) > 0) {
        updateInfo = {
          version: latest,
          notes: rel.body ?? '',
          url: rel.html_url || `https://github.com/vpertj/tusk/releases/tag/v${latest}`,
          assetUrl: rel.asset_url ?? '',
        };
      } else if (!silent) {
        updateCheckMsg = `已是最新版本 v${APP_VERSION} ✅`;
      }
    } catch (e) {
      if (!silent) updateCheckMsg = String(e);
    }
  }

  /** 下载并安装更新：下载 → 进度 → 安装 → 自动重启 */
  async function doUpdate() {
    const info = updateInfo;
    if (!info || updateBusy) return;
    if (!info.assetUrl) {
      updateCheckMsg = '更新包地址不可用，请手动前往 GitHub 下载';
      return;
    }
    updateBusy = true;
    updatePercent = 0;
    updateCheckMsg = '';
    const unlisten = await listen<{ percent: number }>('update-progress', (e) => {
      updatePercent = e.payload.percent ?? 0;
    });
    try {
      const target = `${await invoke<string>('get_download_dir')}/tusk-update.dmg`;
      await invoke('download_update', { url: info.assetUrl, target });
      updateCheckMsg = '下载完成，正在安装…';
      // 安装后应用会自动重启
      await invoke('install_update', { dmgPath: target });
    } catch (e) {
      updateCheckMsg = `更新失败: ${e}`;
      updateBusy = false;
    }
    unlisten();
  }

  // 启动 5 秒后静默检查一次
  setTimeout(() => {
    try {
      checkUpdate(true);
    } catch {
      // 静默失败
    }
  }, 5000);

  // ===== 新建数据库 =====
  let showDbDialog = $state(false);
  let newDbName = $state('');
  let newDbEncoding = $state('UTF8');

  function openDbDialog() {
    newDbName = '';
    newDbEncoding = 'UTF8';
    showDbDialog = true;
  }

  async function doCreateDb() {
    if (!newDbName.trim()) {
      status = '数据库名不能为空';
      return;
    }
    try {
      await invoke('create_database', {
        connId,
        name: newDbName.trim(),
        encoding: newDbEncoding || null,
      });
      showDbDialog = false;
      status = `已创建数据库 ${newDbName.trim()}`;
      await loadDbs();
    } catch (e) {
      status = `创建失败: ${e}`;
    }
  }

  // ================= 标签页工作区 =================
  interface QueryResultView {
    columns: { name: string; type_name: string }[];
    rows: unknown[][];
    affected: number | null;
    error: string;
  }

  interface QueryTab {
    id: number;
    kind: 'query' | 'table';
    title: string;
    // 查询标签字段
    sql: string;
    results: QueryResultView[];
    columns: { name: string; type_name: string }[];
    rows: unknown[][];
    affected: number | null;
    error: string;
    running: boolean;
    elapsed: number | null;
    explainText?: string;
    message?: string;
    colWidths: Record<string, string>;
    // 所属连接（多连接）
    connId: string;
    // 表标签字段
    dbname?: string;
    table?: string;
    subTab?: 'data' | 'structure' | 'sql';
    page?: number;
    pageSize?: number;
    total?: number;
    filters?: { id: number; column: string; op: string; value: string }[];
    filterActive?: boolean;
    loading?: boolean;
    structure?: SchemaColumn[];
    indexes?: IndexInfo[];
    // 数据编辑状态
    selectedRowIdx?: number | null;
    editingCell?: { rowIdx: number; colIdx: number } | null;
    editValue?: string;
    exportMsg?: string;
  }
  let tabs = $state<QueryTab[]>([]);
  let activeTabId = $state(0);
  let tabSeq = $state(1);

  function newTab(sql = ''): QueryTab {
    return {
      id: tabSeq++,
      kind: 'query',
      title: `查询 ${tabSeq - 1}`,
      connId,
      sql,
      results: [],
      columns: [],
      rows: [],
      affected: null,
      error: '',
      running: false,
      elapsed: null,
      colWidths: {},
    };
  }

  function ensureTab(): QueryTab {
    if (tabs.length === 0) {
      tabs.push(newTab('SELECT version() AS pg_version;'));
      activeTabId = tabs[0].id;
    }
    return tabs.find((t) => t.id === activeTabId) ?? tabs[0];
  }

  // 打开 SQL 文件加载到编辑器
  async function onSqlFilePicked(e: Event) {
    const input = e.target as HTMLInputElement;
    const file = input.files?.[0];
    if (!file) return;
    try {
      const text = await file.text();
      let t: QueryTab;
      if (activeTab && activeTab.kind === 'query') {
        t = resolveTab(activeTab);
      } else {
        openNewTab();
        t = activeTab;
      }
      t.sql = text;
      t.title = `📄 ${file.name}`;
      t.error = '';
    } catch (err) {
      status = `读取文件失败: ${err}`;
    }
    input.value = '';
  }

  function openNewTab() {
    const t = newTab('');
    tabs.push(t);
    activeTabId = t.id;
  }

  function closeTab(id: number) {
    const idx = tabs.findIndex((t) => t.id === id);
    if (idx < 0) return;
    tabs.splice(idx, 1);
    if (activeTabId === id) {
      activeTabId = tabs[Math.min(idx, tabs.length - 1)]?.id ?? 0;
    }
  }

  const activeTab = $derived(tabs.find((t) => t.id === activeTabId) ?? tabs[0]);

  // ================= 连接 =================
  async function doConnect() {
    connecting = true;
    try {
      const info = await invoke<{
        id: string;
        version: string;
        user: string;
        host: string;
        port: number;
      }>('connect', {
        host,
        port,
        user,
        password,
        dbname,
      });
      connId = info.id;
      version = info.version;
      connMeta = { user: info.user, host: info.host, port: info.port, version: info.version };
      status = `已连接 · ${user}@${host}:${port}`;
      showConnPanel = false;
      ensureTab();
      await loadDbs();
      syncConnNodes();
      // 勾选保存时写入连接管理
      if (saveConn && connName.trim()) {
        try {
          await invoke('save_connection', {
            dbType,
            name: connName.trim(),
            host,
            port,
            user,
            password,
            dbname,
          });
          await loadSavedConns();
        } catch {
          // 保存失败不影响连接
        }
      }
    } catch (e) {
      status = '连接失败';
      const t = ensureTab();
      t.error = String(e);
      // 本机 PG 未运行/未安装 → 环境引导
      const msg = String(e);
      pgHelp =
        msg.includes('Connection refused') &&
        (host === 'localhost' || host === '127.0.0.1' || host.trim() === '');
    }
    connecting = false;
  }

  // ===== PostgreSQL 环境引导（本机未装/未启动时） =====
  let pgHelp = $state(false);
  const PG_INSTALL_CMD = 'brew install postgresql@17';
  const PG_START_CMD = 'brew services start postgresql@17';

  async function copyCmd(cmd: string) {
    try {
      await navigator.clipboard.writeText(cmd);
      status = '命令已复制到剪贴板';
    } catch {
      status = '复制失败';
    }
  }

  // 用已保存的连接一键连接
  async function connectSaved(name: string) {
    connecting = true;
    try {
      const info = await invoke<{
        id: string;
        version: string;
        user: string;
        host: string;
        port: number;
      }>('connect_saved', { name });
      connId = info.id;
      version = info.version;
      connMeta = { user: info.user, host: info.host, port: info.port, version: info.version };
      status = `已连接 · ${name}`;
      showConnPanel = false;
      ensureTab();
      await loadDbs();
    } catch (e) {
      status = '连接失败';
      const t = ensureTab();
      t.error = String(e);
    }
    connecting = false;
  }

  async function deleteSaved(name: string) {
    try {
      await invoke('delete_connection', { name });
      await loadSavedConns();
    } catch {
      // 忽略
    }
  }

  async function doDisconnect() {
    await disconnectConn(connId);
    showConnPanel = true;
  }

  // ================= 对象树加载 =================
  async function loadDbs() {
    if (!connId) return;
    await loadDbsFor(connId);
  }

  async function toggleDb(conn: string, db: string) {
    const key = ck(conn, db);
    treeOpen[key] = !treeOpen[key];
    activeDb = db;
    connId = conn;
    if (treeOpen[key] && !tables[key]) {
      loadingKey = key;
      try {
        tables[key] = await invoke<TableInfo[]>('list_tables', { connId: conn, dbname: db });
      } catch (e) {
        status = `加载表失败: ${e}`;
      }
      loadingKey = '';
    }
  }

  async function toggleTable(conn: string, db: string, table: string) {
    const key = ck(conn, `${db}.${table}`);
    treeOpen[key] = !treeOpen[key];
    connId = conn;
    if (treeOpen[key] && !columns[key]) {
      loadingKey = key;
      try {
        columns[key] = await invoke<SchemaColumn[]>('list_columns', {
          connId: conn,
          dbname: db,
          table,
        });
      } catch (e) {
        status = `加载字段失败: ${e}`;
      }
      loadingKey = '';
    }
  }

  // 双击表：打开表专属页签（数据/结构/SQL预览）
  function openTableTab(conn: string, db: string, table: string) {
    connId = conn;
    activeDb = db;
    const exist = tabs.find((t) => t.kind === 'table' && t.connId === conn && t.dbname === db && t.table === table);
    if (exist) {
      activeTabId = exist.id;
      return;
    }
    const t: QueryTab = {
      id: tabSeq++,
      kind: 'table',
      title: table,
      connId: conn,
      sql: '',
      results: [],
      columns: [],
      rows: [],
      affected: null,
      error: '',
      running: false,
      elapsed: null,
      colWidths: {},
      dbname: db,
      table,
      subTab: 'data',
      page: 1,
      pageSize: settingsPageSize,
      total: 0,
      loading: false,
      structure: undefined,
      selectedRowIdx: null,
      editingCell: null,
      editValue: '',
      exportMsg: '',
    };
    tabs.push(t);
    activeTabId = t.id;
    loadTablePage(t);
    loadStructure(t);
  }

  // 从 tabs 取响应式 proxy 引用（push 进 $state 数组的元素会被深代理，
  // 外部保留的原始对象引用不触发响应式，必须重新取）
  function resolveTab(raw: QueryTab): QueryTab {
    return tabs.find((x) => x.id === raw.id) ?? raw;
  }

  // 加载表数据页
  async function loadTablePage(raw: QueryTab) {
    const t = resolveTab(raw);
    if (!t.connId || !t.dbname || !t.table) return;
    t.loading = true;
    t.error = '';
    try {
      const res = await invoke<{
        columns: { name: string; type_name: string }[];
        rows: unknown[][];
        total: number | null;
      }>('paginate_table', {
        connId: t.connId,
        dbname: t.dbname,
        table: t.table,
        limit: t.pageSize,
        offset: (t.page! - 1) * t.pageSize!,
        filters: t.filterActive
          ? (t.filters ?? []).map((f) => ({
              column: f.column,
              op: f.op,
              value: f.op.startsWith('IS') ? null : f.value.trim() || null,
            }))
          : [],
      });
      t.columns = res.columns;
      t.rows = res.rows;
      t.total = res.total ?? 0;
      t.colWidths = {};
    } catch (e) {
      t.error = String(e);
      t.loading = false;
      // 失败时保留原有数据，绝不清空表格
      return;
    }
    t.loading = false;
  }

  // 加载表结构
  async function loadStructure(raw: QueryTab) {
    const t = resolveTab(raw);
    if (!t.connId || !t.dbname || !t.table) return;
    if (t.structure) return;
    try {
      t.structure = await invoke<SchemaColumn[]>('list_columns', {
        connId: t.connId,
        dbname: t.dbname,
        table: t.table,
      });
      t.indexes = await invoke<IndexInfo[]>('list_indexes', {
        connId: t.connId,
        dbname: t.dbname,
        table: t.table,
      });
    } catch (e) {
      t.error = String(e);
    }
  }

  // 切换表页签子标签
  function setSubTab(raw: QueryTab, sub: 'data' | 'structure' | 'sql') {
    const t = resolveTab(raw);
    t.subTab = sub;
    if (sub === 'structure') loadStructure(t);
    if (sub === 'data' && t.rows.length === 0 && !t.loading) loadTablePage(t);
  }

  function tablePrev(raw: QueryTab) {
    const t = resolveTab(raw);
    if (t.page! > 1) {
      t.page!--;
      loadTablePage(t);
    }
  }

  function tableNext(raw: QueryTab) {
    const t = resolveTab(raw);
    if (t.total! > t.page! * t.pageSize!) {
      t.page!++;
      loadTablePage(t);
    }
  }

  // ================= 数据编辑 =================
  let editInput: HTMLInputElement | null = $state(null);

  $effect(() => {
    if (activeTab?.editingCell && editInput) {
      editInput.focus();
      editInput.select();
    }
  });

  // 主键列名（编辑/删除依赖主键定位行）
  function pkNames(t: QueryTab): string[] {
    return (t.structure ?? []).filter((c) => c.is_pk).map((c) => c.name);
  }

  function canEdit(t: QueryTab): boolean {
    return pkNames(t).length > 0;
  }

  function selectRow(t: QueryTab, idx: number) {
    t.selectedRowIdx = idx;
  }

  function startEdit(t: QueryTab, rowIdx: number, colIdx: number) {
    if (!canEdit(t)) return;
    t.editingCell = { rowIdx, colIdx };
    t.editValue = cellText(t.rows[rowIdx][colIdx]);
    t.selectedRowIdx = rowIdx;
  }

  async function commitEdit(raw: QueryTab) {
    const t = resolveTab(raw);
    const cell = t.editingCell;
    if (!cell) return;
    t.editingCell = null;
    const col = t.columns[cell.colIdx];
    const pkCols = pkNames(t);
    if (!col || pkCols.length === 0) return;
    const pkVals = pkCols.map((c) => {
      const ci = t.columns.findIndex((x) => x.name === c);
      return cellText(t.rows[cell.rowIdx][ci]);
    });
    const value = t.editValue?.trim() === '' ? null : t.editValue;
    try {
      await invoke('update_cell', {
        connId: t.connId,
        dbname: t.dbname,
        table: t.table,
        pkCols,
        pkVals,
        col: col.name,
        colType: col.type_name,
        value,
      });
      await loadTablePage(t);
    } catch (e) {
      t.error = String(e);
    }
  }

  async function addRow(raw: QueryTab) {
    const t = resolveTab(raw);
    try {
      await invoke('insert_row', { connId: t.connId, dbname: t.dbname, table: t.table });
      await loadTablePage(t);
    } catch (e) {
      t.error = String(e);
    }
  }

  async function removeRow(raw: QueryTab) {
    const t = resolveTab(raw);
    if (t.selectedRowIdx === null || t.selectedRowIdx === undefined) return;
    const pkCols = pkNames(t);
    if (pkCols.length === 0) return;
    const pkVals = pkCols.map((c) => {
      const ci = t.columns.findIndex((x) => x.name === c);
      return cellText(t.rows[t.selectedRowIdx!][ci]);
    });
    try {
      await invoke('delete_row', {
        connId: t.connId,
        dbname: t.dbname,
        table: t.table,
        pkCols,
        pkVals,
      });
      t.selectedRowIdx = null;
      await loadTablePage(t);
    } catch (e) {
      t.error = String(e);
    }
  }

  async function exportTable(raw: QueryTab) {
    const t = resolveTab(raw);
    try {
      const path = await invoke<string>('export_csv', {
        connId: t.connId,
        dbname: t.dbname,
        table: t.table,
      });
      t.exportMsg = `已导出 CSV：${path}`;
    } catch (e) {
      t.error = String(e);
    }
  }

  // 表 SQL 预览 → 在查询编辑器打开
  // 注意：dbname 是数据库名不是 schema，表在 public schema 下
  function tablePreviewSql(t: QueryTab): string {
    return `SELECT * FROM "public"."${t.table}";`;
  }

  function openTableInEditor(t: QueryTab) {
    const sql = tablePreviewSql(t);
    const q = newTab(sql);
    tabs.push(q);
    activeTabId = q.id;
  }

  // ================= SQL 历史（localStorage，最近 50 条） =================
  let sqlHistory = $state<string[]>([]);
  let histIdx = $state(-1);

  try {
    const raw = localStorage.getItem('tusk.sqlHistory');
    if (raw) sqlHistory = JSON.parse(raw);
  } catch {
    sqlHistory = [];
  }

  function saveHistory(sqlText: string) {
    const s = sqlText.trim();
    if (!s) return;
    sqlHistory = [s, ...sqlHistory.filter((x) => x !== s)].slice(0, 50);
    localStorage.setItem('tusk.sqlHistory', JSON.stringify(sqlHistory));
    histIdx = -1;
  }

  async function runExplain(tab?: QueryTab) {
    const t = tab ?? activeTab;
    if (!t || !t.connId || !t.sql.trim()) return;
    t.running = true;
    try {
      t.explainText = await invoke<string>('explain_query', { connId: t.connId, sql: t.sql });
      t.error = '';
    } catch (e) {
      t.error = String(e);
    }
    t.running = false;
  }

  // ================= 查询 =================
  // 格式化当前查询 SQL（sql-formatter，PostgreSQL 方言）
  function formatQuery() {
    const t = activeTab;
    if (!t || t.kind !== 'query' || !t.sql.trim()) return;
    try {
      t.sql = formatSql(t.sql, { language: 'postgresql' });
      status = 'SQL 已格式化';
    } catch (e) {
      status = `格式化失败: ${e}`;
    }
  }

  async function runQuery(tab?: QueryTab) {
    const t = tab ?? activeTab;
    if (!t || !t.connId || !t.sql.trim()) return;
    t.running = true;
    t.error = '';
    t.explainText = undefined; // 执行新查询时清掉旧执行计划
    const t0 = performance.now();
    try {
      const res = await invoke<{ results: QueryResultView[] }>('query', {
        connId: t.connId,
        sql: t.sql,
      });
      t.results = res.results;
      saveHistory(t.sql);
    } catch (e) {
      t.results = [{ columns: [], rows: [], affected: null, error: String(e) }];
    }
    t.elapsed = performance.now() - t0;
    t.running = false;
  }

  // 表头拖拽调整列宽
  function startResize(e: MouseEvent, tab: QueryTab, colName: string) {
    e.preventDefault();
    const th = (e.target as HTMLElement).closest('th') as HTMLElement;
    if (!th) return;
    const startX = e.clientX;
    const startW = th.offsetWidth;
    const onMove = (ev: MouseEvent) => {
      const w = Math.max(48, startW + (ev.clientX - startX));
      th.style.width = `${w}px`;
      tab.colWidths[colName] = `${w}px`;
    };
    const onUp = () => {
      document.removeEventListener('mousemove', onMove);
      document.removeEventListener('mouseup', onUp);
      document.body.style.cursor = '';
    };
    document.body.style.cursor = 'col-resize';
    document.addEventListener('mousemove', onMove);
    document.addEventListener('mouseup', onUp);
  }

  function cellText(v: unknown): string {
    if (v === null || v === undefined) return 'NULL';
    if (typeof v === 'object') return JSON.stringify(v);
    return String(v);
  }

  function isNull(v: unknown): boolean {
    return v === null || v === undefined;
  }

  function keydown(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
      e.preventDefault();
      runQuery();
    } else if ((e.metaKey || e.ctrlKey) && e.key === 'n') {
      e.preventDefault();
      openNewTab();
    } else if ((e.metaKey || e.ctrlKey) && e.key === 'ArrowUp') {
      e.preventDefault();
      if (histIdx < sqlHistory.length - 1) {
        histIdx++;
        if (activeTab) activeTab.sql = sqlHistory[histIdx];
      }
    } else if ((e.metaKey || e.ctrlKey) && e.key === 'ArrowDown') {
      e.preventDefault();
      if (histIdx > 0) {
        histIdx--;
        if (activeTab) activeTab.sql = sqlHistory[histIdx];
      } else if (histIdx === 0) {
        histIdx = -1;
        if (activeTab) activeTab.sql = '';
      }
    }
  }
</script>

<div class="app">
  <!-- ============ 顶部工具栏 ============ -->
    <Header
      {connId}
      {openNewTab}
      {onSqlFilePicked}
      {openDesigner}
      {openViewDialog}
      {openSyncDialog}
      {loadDbs}
      openSearch={() => (searchOpen = true)}
      toggleConnPanel={() => (showConnPanel = !showConnPanel)}
      {doDisconnect}
    />

  <main>
    <ConnDialog
      {showConnPanel}
      {savedConns}
      {connecting}
      {dbType}
      {host}
      {port}
      {user}
      {password}
      {dbname}
      {connName}
      {saveConn}
      {pgHelp}
      PG_INSTALL_CMD={PG_INSTALL_CMD}
      PG_START_CMD={PG_START_CMD}
      close={() => (showConnPanel = false)}
      {connectSaved}
      {deleteSaved}
      {doConnect}
      {copyCmd}
    />

    <!-- ============ 左侧对象树 ============ -->
    <Sidebar
      {sidebarWidth}
      {connNodes}
      {connDbs}
      {tables}
      {treeOpen}
      {columns}
      {loadingKey}
      {connId}
      {openBlankMenu}
      openConnPanel={() => (showConnPanel = true)}
      {connRowClick}
      {openConnMenu}
      {openDbDialog}
      {toggleDb}
      {openDbMenu}
      {toggleTable}
      {openTableTab}
      {openTableMenu}
      {openDesignerForEdit}
      {dropViewFromTree}
      {dropTableFromTree}
      {ck}
      {startSidebarResize}
    />

    <!-- ============ 中央标签页工作区 ============ -->
    <section class="workspace">
      {#if tabs.length > 0}
        <div class="tabbar">
          {#each tabs as t}
            <div
              class="tab"
              class:active={t.id === activeTabId}
              role="button"
              tabindex="0"
              onclick={() => (activeTabId = t.id)}
              onkeydown={(e) => e.key === 'Enter' && (activeTabId = t.id)}
            >
              <span class="tab-ico">{t.kind === 'table' ? '📋' : '❯'}</span>
              <span class="tab-title">{t.title}</span>
              <span
                class="tab-close"
                role="button"
                tabindex="0"
                onclick={(e) => {
                  e.stopPropagation();
                  closeTab(t.id);
                }}
                onkeydown={(e) => e.key === 'Enter' && closeTab(t.id)}
                >×</span
              >
            </div>
          {/each}
        </div>
      {/if}

      {#if activeTab}
        {#if activeTab.kind === 'query'}
          <div class="tab-content">
            <div class="editor">
              <textarea
                bind:value={activeTab.sql}
                placeholder="输入 SQL…（Cmd/Ctrl + Enter 执行）"
                onkeydown={keydown}
              ></textarea>
              <div class="editor-bar">
                <button onclick={() => runQuery(activeTab)} disabled={!connId || activeTab.running}>
                  ▶ 执行
                </button>
                <button onclick={formatQuery} disabled={!activeTab.sql.trim()} title="格式化 SQL">
                  ✨ 格式化
                </button>
                <button
                  onclick={() => runExplain(activeTab)}
                  disabled={!connId || activeTab.running}
                  title="EXPLAIN (ANALYZE, BUFFERS)，仅支持单条 SELECT"
                  >🧠 Explain</button
                >
                <span class="hint">Cmd+Enter 执行 · Cmd+N 新查询</span>
              </div>
            </div>

            <div class="result">
              {#if activeTab.kind === 'query' && activeTab.results?.length && activeTab.message}
                <div class="query-msg">✓ {activeTab.message}</div>
              {/if}
              {#if activeTab.kind === 'query' && activeTab.results?.some((r) => r.rows.length > 0)}
                <div class="query-export-bar">
                  <button onclick={() => exportQueryCsv(activeTab)}>⬇ 导出 CSV</button>
                </div>
              {/if}
              {#if activeTab.explainText}
                <div class="explain-box">
                  <div class="explain-title">📊 执行计划（EXPLAIN ANALYZE）</div>
                  <pre>{activeTab.explainText}</pre>
                </div>
              {/if}
              {#each activeTab.results as res, ri}
                <div class="result-block">
                  {#if res.error}
                    <div class="error">⚠ {res.error}</div>
                  {/if}
                  {#if res.affected !== null}
                    <div class="ok">✓ 第 {ri + 1} 条：影响 {res.affected} 行</div>
                  {/if}
                  {#if res.columns.length > 0}
                    <div class="table-wrap">
                      <table>
                        <thead>
                          <tr>
                            {#each res.columns as col}
                              <th
                                style={activeTab.colWidths[col.name] ? `width:${activeTab.colWidths[col.name]}` : ''}
                              >
                                {col.name}<small>{col.type_name}</small>
                                <span
                                  class="resizer"
                                  role="presentation"
                                  onmousedown={(e) => startResize(e, activeTab, col.name)}
                                ></span>
                              </th>
                            {/each}
                          </tr>
                        </thead>
                        <tbody>
                          {#each res.rows as row}
                            <tr>
                              {#each row as cell}
                                <td class:null={isNull(cell)}>{cellText(cell)}</td>
                              {/each}
                            </tr>
                          {/each}
                        </tbody>
                      </table>
                    </div>
                    <div class="count">
                      第 {ri + 1} 条 · 共 {res.rows.length} 行
                      {#if ri === activeTab.results.length - 1 && activeTab.elapsed != null}
                        · 耗时 {activeTab.elapsed.toFixed(0)} ms
                      {/if}
                    </div>
                  {/if}
                </div>
              {/each}
              {#if activeTab.results.length === 0 && !activeTab.running}
                <div class="empty">连接后输入 SQL，点执行查看结果（支持多语句）；双击左侧表直接浏览数据</div>
              {/if}
            </div>
          </div>
        {:else}
          <!-- 表页签：数据 / 结构 / SQL 预览 -->
          <div class="tab-content">
            <div class="subtabbar">
              <span
                class="subtab"
                class:active={activeTab.subTab === 'data'}
                role="button"
                tabindex="0"
                onclick={() => setSubTab(activeTab, 'data')}
                onkeydown={(e) => e.key === 'Enter' && setSubTab(activeTab, 'data')}
                >数据</span
              >
              <span
                class="subtab"
                class:active={activeTab.subTab === 'structure'}
                role="button"
                tabindex="0"
                onclick={() => setSubTab(activeTab, 'structure')}
                onkeydown={(e) => e.key === 'Enter' && setSubTab(activeTab, 'structure')}
                >结构</span
              >
              <span
                class="subtab"
                class:active={activeTab.subTab === 'sql'}
                role="button"
                tabindex="0"
                onclick={() => setSubTab(activeTab, 'sql')}
                onkeydown={(e) => e.key === 'Enter' && setSubTab(activeTab, 'sql')}
                >SQL 预览</span
              >
              <span class="subtab-spacer"></span>
              <span class="subtab-hint"
                >{activeTab.dbname}.{activeTab.table} · 每页 {activeTab.pageSize} 行
                {#if activeTab.message}
                  <span class="ok-msg">✓ {activeTab.message}</span>
                {/if}</span
              >
            </div>

            {#if activeTab.error}
              <div class="error">⚠ {activeTab.error}</div>
            {/if}

            {#if activeTab.subTab === 'data'}
              <div class="filter-bar">
                {#each activeTab.filters ?? [] as f}
                  <select bind:value={f.column} title="筛选字段">
                    <option value="">字段…</option>
                    {#each activeTab.structure ?? [] as c}
                      <option value={c.name}>{c.name}</option>
                    {/each}
                  </select>
                  <select bind:value={f.op} title="运算符">
                    {#each FILTER_OPS as op}
                      <option value={op}>{op}</option>
                    {/each}
                  </select>
                  <input
                    bind:value={f.value}
                    placeholder="值（LIKE 用 % 通配）"
                    disabled={f.op === 'IS NULL' || f.op === 'IS NOT NULL'}
                  />
                  <button
                    class="filter-del"
                    onclick={() => removeFilter(activeTab, f.id)}
                    title="删除条件"
                    >×</button
                  >
                {/each}
                <button onclick={() => addFilter(activeTab)} class="filter-add">＋ 条件</button>
                <button onclick={() => applyFilters(activeTab)} class="primary filter-apply">
                  🔍 应用
                </button>
                {#if activeTab.filterActive}
                  <button onclick={() => clearFilters(activeTab)} class="filter-clear">✕ 清除</button>
                  <span class="filter-on">筛选生效</span>
                {/if}
              </div>
              <div class="data-toolbar">
                <button onclick={() => openInsertDialog(activeTab)} disabled={activeTab.loading}>
                  ＋ 新增行
                </button>
                <button
                  onclick={() => removeRow(activeTab)}
                  disabled={!canEdit(activeTab) || activeTab.selectedRowIdx === null || activeTab.loading}
                  >🗑 删除行</button
                >
                <button onclick={() => exportTable(activeTab)} disabled={activeTab.loading}>
                  ⬇ 导出 CSV
                </button>
                <button onclick={() => exportTableSql(activeTab)} disabled={activeTab.loading}>
                  ⬇ 导出 SQL
                </button>
                <button
                  class="danger"
                  onclick={() => dropTable(activeTab)}
                  disabled={activeTab.loading}
                  title="删除整张表（不可恢复）"
                  >🗑 删除表</button
                >
                <button onclick={() => loadTablePage(activeTab)} disabled={activeTab.loading}>
                  ⟳ 刷新
                </button>
                <span class="toolbar-hint">
                  {canEdit(activeTab) ? '双击单元格编辑 · 点击行选中' : '无主键表仅可新增/导出'}
                </span>
              </div>
              <div class="result">
                {#if activeTab.exportMsg}
                  <div class="ok">✓ {activeTab.exportMsg}</div>
                {/if}
                {#if activeTab.columns.length > 0}
                  <div class="table-wrap">
                    <table>
                      <thead>
                        <tr>
                          {#each activeTab.columns as col}
                            <th
                              style={activeTab.colWidths[col.name] ? `width:${activeTab.colWidths[col.name]}` : ''}
                            >
                              {col.name}<small>{col.type_name}</small>
                              <span
                                class="resizer"
                                role="presentation"
                                onmousedown={(e) => startResize(e, activeTab, col.name)}
                              ></span>
                            </th>
                          {/each}
                        </tr>
                      </thead>
                      <tbody>
                        {#each activeTab.rows as row, rowIdx}
                          <tr
                            class:selected={activeTab.selectedRowIdx === rowIdx}
                            onclick={() => selectRow(activeTab, rowIdx)}
                          >
                            {#each row as cell, colIdx}
                              <td
                                class:null={isNull(cell)}
                                ondblclick={() => startEdit(activeTab, rowIdx, colIdx)}
                              >
                                {#if activeTab.editingCell?.rowIdx === rowIdx && activeTab.editingCell?.colIdx === colIdx}
                                  <input
                                    bind:this={editInput}
                                    bind:value={activeTab.editValue}
                                    class="cell-input"
                                    onkeydown={(e) => {
                                      if (e.key === 'Enter') {
                                        e.stopPropagation();
                                        commitEdit(activeTab);
                                      } else if (e.key === 'Escape') {
                                        activeTab.editingCell = null;
                                      }
                                    }}
                                  />
                                {:else}
                                  {cellText(cell)}
                                {/if}
                              </td>
                            {/each}
                          </tr>
                        {/each}
                      </tbody>
                    </table>
                  </div>
                  <div class="count">
                    <span>
                      {#if activeTab.loading}加载中…{:else}第 {activeTab.page} 页 · 共 {activeTab.total} 行{/if}
                    </span>
                    <span class="pager">
                      <button onclick={() => tablePrev(activeTab)} disabled={activeTab.page! <= 1 || activeTab.loading}
                        >‹ 上一页</button
                      >
                      <button
                        onclick={() => tableNext(activeTab)}
                        disabled={activeTab.total! <= activeTab.page! * activeTab.pageSize! || activeTab.loading}
                        >下一页 ›</button
                      >
                    </span>
                  </div>
                {:else if !activeTab.loading}
                  <div class="empty">表暂无数据</div>
                {/if}
              </div>
            {:else if activeTab.subTab === 'structure'}
              <div class="result">
                <div class="struct-toolbar">
                  <button
                    onclick={() => openDesignerForEdit(activeTab.connId, activeTab.dbname!, activeTab.table!)}
                    title="修改表结构（增删字段/改类型/改默认值）"
                    >✎ 编辑结构</button
                  >
                </div>
                <div class="table-wrap">
                  <table class="struct-table">
                    <thead>
                      <tr>
                        <th>字段名</th>
                        <th>类型</th>
                        <th>可空</th>
                        <th>默认值</th>
                      </tr>
                    </thead>
                    <tbody>
                      {#each activeTab.structure ?? [] as col}
                        <tr>
                          <td>{col.is_pk ? '🔑 ' : ''}{col.name}</td>
                          <td>{col.type_name}</td>
                          <td>{col.is_nullable === 'YES' ? '是' : '否'}</td>
                          <td>{col.default ?? '—'}</td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                </div>
                <div class="count">共 {(activeTab.structure ?? []).length} 个字段</div>
                <div class="idx-title">索引（{(activeTab.indexes ?? []).length}）</div>
                {#if (activeTab.indexes ?? []).length === 0}
                  <div class="idx-empty">无索引（主键除外）</div>
                {:else}
                  <div class="table-wrap">
                    <table class="struct-table">
                      <thead>
                        <tr>
                          <th>索引名</th>
                          <th>列</th>
                          <th>唯一</th>
                        </tr>
                      </thead>
                      <tbody>
                        {#each activeTab.indexes ?? [] as ix}
                          <tr>
                            <td>{ix.name}</td>
                            <td>{ix.columns}</td>
                            <td>{ix.is_unique ? '✅' : '—'}</td>
                          </tr>
                        {/each}
                      </tbody>
                    </table>
                  </div>
                {/if}
              </div>
            {:else}
              <div class="result">
                <div class="sql-preview">
                  <pre>{tablePreviewSql(activeTab)}</pre>
                  <button onclick={() => openTableInEditor(activeTab)}>在编辑器中打开</button>
                </div>
              </div>
            {/if}
          </div>
        {/if}
      {:else}
        <div class="empty">连接后点击「＋ 新建查询」开始</div>
      {/if}
    </section>
  </main>

  <!-- ============ 表设计器（新建表） ============ -->
  {#if showDesigner && connId}
    <div class="overlay" role="presentation" onclick={() => (showDesigner = false)}>
      <div
        class="conn-dialog designer-dialog"
        role="dialog"
        aria-label="新建表"
        tabindex="-1"
        onclick={(e) => e.stopPropagation()}
        onkeydown={(e) => e.key === 'Escape' && (showDesigner = false)}
      >
        <div class="dialog-head">
          <span class="dialog-title">📋 {editingTable ? `编辑表：${editingTable.table}` : '新建表'}</span>
          <button class="dialog-close" onclick={() => (showDesigner = false)}>×</button>
        </div>
        <div class="dialog-body">
          {#if designerError}
            <div class="error">⚠ {designerError}</div>
          {/if}
          <div class="field">
            <label for="d-tname">表名</label>
            <input
              id="d-tname"
              bind:value={designerName}
              placeholder="表名"
              disabled={!!editingTable}
            />
          </div>
          <div class="field">
            <label for="d-tcmt">表注释</label>
            <input
              id="d-tcmt"
              bind:value={designerComment}
              placeholder={editingTable ? '留空则不修改现有注释' : '表注释（可选）'}
            />
          </div>
          <div class="field">
            <label for="d-db">数据库</label>
            <select id="d-db" bind:value={designerDb} disabled={!!editingTable}>
              {#each (connDbs[connId] ?? []) as db}
                <option value={db.name}>{db.name}</option>
              {/each}
            </select>
          </div>
          <div class="saved-title" style="margin-top:12px">字段定义</div>
          <div class="designer-grid">
            <div class="dg-head">
              <span class="dg-pk">主键</span>
              <span class="dg-serial">自增</span>
              <span class="dg-name">字段名</span>
              <span class="dg-type">类型</span>
              <span class="dg-len">长度/精度</span>
              <span class="dg-null">可空</span>
              <span class="dg-def">默认值</span>
              <span class="dg-cmt">注释</span>
              <span class="dg-del"></span>
            </div>
            {#each designerCols as c}
              <div class="dg-row">
                <span class="dg-pk">
                  <input
                    type="radio"
                    name="dg-pk"
                    checked={c.isPk}
                    onclick={() => designerCols.forEach((x) => (x.isPk = x.id === c.id))}
                  />
                </span>
                <span class="dg-serial">
                  <input
                    type="checkbox"
                    checked={c.baseType === 'serial' || c.baseType === 'bigserial'}
                    onclick={(e) => {
                      const want = (e.target as HTMLInputElement).checked;
                      if (want && c.baseType !== 'serial' && c.baseType !== 'bigserial') {
                        c.baseType = 'serial';
                      }
                      if (!want && (c.baseType === 'serial' || c.baseType === 'bigserial')) {
                        c.baseType = 'int4';
                      }
                    }}
                  />
                </span>
                <span class="dg-name">
                  <input bind:value={c.name} placeholder="字段名" />
                </span>
                <span class="dg-type">
                  <select bind:value={c.baseType}>
                    {#if !DESIGNER_TYPES.includes(c.baseType)}
                      <option value={c.baseType} disabled>{c.baseType}</option>
                    {/if}
                    {#each DESIGNER_TYPES as t}
                      <option value={t}>{t}</option>
                    {/each}
                  </select>
                </span>
                <span class="dg-len">
                  <input
                    bind:value={c.length}
                    placeholder={c.baseType === 'numeric' ? '10,2' : '255'}
                  />
                </span>
                <span class="dg-null">
                  <input
                    type="checkbox"
                    bind:checked={c.nullable}
                    disabled={c.baseType === 'serial' || c.baseType === 'bigserial'}
                  />
                </span>
                <span class="dg-def">
                  <input bind:value={c.default} placeholder="now() / 0 / 'x'" />
                </span>
                <span class="dg-cmt">
                  <input bind:value={c.comment} placeholder="字段注释" />
                </span>
                <span class="dg-del">
                  <button onclick={() => delDesignerCol(c.id)}>×</button>
                </span>
              </div>
            {/each}
          </div>
          <button onclick={addDesignerCol} class="add-col">＋ 添加字段</button>
          <div class="field-actions" style="margin-top:16px">
            <button onclick={() => (showDesigner = false)}>取消</button>
            <button onclick={doCreateTable} class="primary">
              {editingTable ? '保存修改' : '创建表'}
            </button>
          </div>
        </div>
      </div>
    </div>
  {/if}

  <!-- ============ 新增行弹窗 ============ -->
  {#if insertDialog}
    <div class="overlay" role="presentation" onclick={() => (insertDialog = null)}>
      <div
        class="conn-dialog insert-dialog"
        role="dialog"
        aria-label="新增行"
        tabindex="-1"
        onclick={(e) => e.stopPropagation()}
        onkeydown={(e) => e.key === 'Escape' && (insertDialog = null)}
      >
        <div class="dialog-head">
          <span class="dialog-title">＋ 新增行</span>
          <button class="dialog-close" onclick={() => (insertDialog = null)}>×</button>
        </div>
        <div class="dialog-body">
          {#if insertDialog.err}
            <div class="error">⚠ {insertDialog.err}</div>
          {/if}
          {#each insertDialog.cols as c}
            <div class="field">
              <label for={`ins-${c.name}`}>
                {c.name}
                <span class="field-hint">
                  {c.type_name}
                  {#if c.is_nullable === 'NO'} · 必填{/if}
                </span>
              </label>
              {#if c.is_pk && (c.default ?? '').includes('nextval(')}
                <input id={`ins-${c.name}`} value="自动生成" disabled />
              {:else}
                <input
                  id={`ins-${c.name}`}
                  bind:value={insertDialog.values[c.name]}
                  placeholder={
                    c.default ? `默认: ${c.default}` : c.is_nullable === 'NO' ? '必填' : '可留空(NULL)'
                  }
                />
              {/if}
            </div>
          {/each}
          <div class="field-actions" style="margin-top:16px">
            <button onclick={() => (insertDialog = null)}>取消</button>
            <button onclick={doInsertRow} class="primary">插入</button>
          </div>
        </div>
      </div>
    </div>
  {/if}

  <!-- ============ 结构同步弹窗 ============ -->
  {#if showSync}
    <div class="overlay" role="presentation" onclick={() => (showSync = false)}>
      <div
        class="conn-dialog sync-dialog"
        role="dialog"
        aria-label="结构同步"
        tabindex="-1"
        onclick={(e) => e.stopPropagation()}
        onkeydown={(e) => e.key === 'Escape' && (showSync = false)}
      >
        <div class="dialog-head">
          <span class="dialog-title">⇄ 同步{syncMode === 'schema_data' ? '结构和数据' : '结构'}</span>
          <button class="dialog-close" onclick={() => (showSync = false)}>×</button>
        </div>
        <div class="dialog-body">
          <!-- 同步模式 -->
          <div class="sync-mode">
            <button class="sync-mode-btn" class:active={syncMode === 'schema'} onclick={() => (syncMode = 'schema')}>
              仅同步结构
            </button>
            <button class="sync-mode-btn" class:active={syncMode === 'schema_data'} onclick={() => (syncMode = 'schema_data')}>
              结构和数据
            </button>
          </div>
          <div class="sync-pair">
            <div class="field">
              <label for="s-src-conn">源连接</label>
              <select id="s-src-conn" bind:value={syncSrcConn}>
                <option value="__current__">当前连接</option>
                {#each savedConns as sc (sc.name)}
                  <option value={sc.name}>{sc.name}</option>
                {/each}
              </select>
            </div>
            <div class="sync-arrow">→</div>
            <div class="field">
              <label for="s-dst-conn">目标连接</label>
              <select id="s-dst-conn" bind:value={syncDstConn}>
                <option value="__current__">当前连接</option>
                {#each savedConns as sc (sc.name)}
                  <option value={sc.name}>{sc.name}</option>
                {/each}
              </select>
            </div>
          </div>
          <div class="sync-pair">
            <div class="field">
              <label for="s-src">源数据库</label>
              <select id="s-src" bind:value={syncSrc}>
                {#each (connDbs[connId] ?? []) as db}
                  <option value={db.name}>{db.name}</option>
                {/each}
              </select>
            </div>
            <div class="sync-arrow">→</div>
            <div class="field">
              <label for="s-dst">目标数据库</label>
              <select id="s-dst" bind:value={syncDst}>
                {#each (connDbs[connId] ?? []) as db}
                  <option value={db.name}>{db.name}</option>
                {/each}
              </select>
            </div>
          </div>
          {#if syncMode === 'schema_data'}
            <div class="sync-hint">结构和数据：目标表数据将被源表全量覆盖（清空后灌入源数据）</div>
          {/if}
          <div class="field-actions" style="margin: 10px 0">
            <button onclick={doCompare} disabled={syncBusy}>
              {syncBusy ? '比较中…' : `🔍 比较${syncMode === 'schema_data' ? '结构和数据' : '结构'}`}
            </button>
          </div>
          {#if syncError}
            <div class="sync-err">⚠ {syncError}</div>
          {/if}
          {#if syncMsg}
            <div class="sync-ok">{syncMsg}</div>
          {/if}
          {#if syncDiffs.length > 0}
            <div class="sync-list">
              <div class="sync-head">
                <span>表</span>
                <span>操作</span>
                <span>SQL 预览</span>
              </div>
              {#each syncDiffs as d, i (d.table)}
                <div class="sync-item">
                  <label class="sync-check">
                    <input type="checkbox" bind:checked={d.checked} />
                    <span class="sync-tbl">{d.table}</span>
                  </label>
                  <span class="sync-act {d.action}">
                    {d.action === 'create' ? '＋ 新建' : d.action === 'alter' ? '✎ 修改' : '🗑 删除'}
                  </span>
                  <span class="sync-sql">{d.sql.split('\n')[0]}{d.sql.split('\n').length > 1 ? ' …' : ''}</span>
                </div>
              {/each}
            </div>
            <div class="field-actions" style="margin-top: 12px">
              <button
                onclick={() =>
                  (syncDiffs = syncDiffs.map((d) => ({ ...d, checked: !d.checked })))}
              >
                全选/全不选
              </button>
              <button
                onclick={doSyncExecute}
                class="primary danger"
                disabled={syncBusy || !syncDiffs.some((d) => d.checked)}
              >
                {syncBusy ? '执行中…' : `⚡ 执行同步（${syncDiffs.filter((d) => d.checked).length} 项）`}
              </button>
            </div>
          {/if}
        </div>
      </div>
    </div>
  {/if}

  <!-- ============ 发现新版本弹窗 ============ -->
  {#if updateInfo}
    <div class="overlay" role="presentation" onclick={() => (updateInfo = null)}>
      <div
        class="conn-dialog update-dialog"
        role="dialog"
        aria-label="发现新版本"
        tabindex="-1"
        onclick={(e) => e.stopPropagation()}
        onkeydown={(e) => e.key === 'Escape' && (updateInfo = null)}
      >
        <div class="dialog-head">
          <span class="dialog-title">🚀 发现新版本 v{updateInfo.version}</span>
          <button class="dialog-close" onclick={() => (updateInfo = null)}>×</button>
        </div>
        <div class="dialog-body">
          <p class="update-desc">
            当前版本 <b>v{APP_VERSION}</b>，最新版本 <b>v{updateInfo.version}</b>，建议更新。
          </p>
          {#if updateInfo.notes.trim()}
            <div class="update-notes">
              {#each updateInfo.notes.split('\n') as line}
                <div>{line}</div>
              {/each}
            </div>
          {/if}
          <div class="field-actions" style="margin-top: 18px">
            {#if updateBusy}
              <div class="update-progress">
                <div class="update-bar">
                  <div class="update-bar-fill" style={`width:${updatePercent}%`}></div>
                </div>
                <span class="update-pct">{updatePercent}%</span>
              </div>
              <div class="update-msg">{updateCheckMsg || '正在下载更新包…'}</div>
            {:else}
              <button onclick={() => (updateInfo = null)}>稍后再说</button>
              <button onclick={doUpdate} class="primary">⬇ 下载并安装</button>
            {/if}
          </div>
          {#if !updateBusy && updateCheckMsg}
            <div class="update-msg">{updateCheckMsg}</div>
          {/if}
        </div>
      </div>
    </div>
  {/if}

  <!-- ============ 新建数据库弹窗 ============ -->
  {#if showDbDialog}
    <div class="overlay" role="presentation" onclick={() => (showDbDialog = false)}>
      <div
        class="conn-dialog db-dialog"
        role="dialog"
        aria-label="新建数据库"
        tabindex="-1"
        onclick={(e) => e.stopPropagation()}
        onkeydown={(e) => e.key === 'Escape' && (showDbDialog = false)}
      >
        <div class="dialog-head">
          <span class="dialog-title">🗄 新建数据库</span>
          <button class="dialog-close" onclick={() => (showDbDialog = false)}>×</button>
        </div>
        <div class="dialog-body">
          <div class="field">
            <label for="ndb-name">数据库名</label>
            <input id="ndb-name" bind:value={newDbName} placeholder="如 my_database" />
          </div>
          <div class="field" style="margin-top: 12px">
            <label for="ndb-enc">字符集</label>
            <select id="ndb-enc" bind:value={newDbEncoding}>
              <option value="UTF8">UTF8（默认）</option>
              <option value="GBK">GBK（中文）</option>
              <option value="LATIN1">LATIN1</option>
              <option value="">跟随模板库</option>
            </select>
          </div>
          <div class="field-actions" style="margin-top: 18px">
            <button onclick={() => (showDbDialog = false)}>取消</button>
            <button onclick={doCreateDb} class="primary">创建</button>
          </div>
        </div>
      </div>
    </div>
  {/if}

  <!-- ============ 设置弹窗（左右分栏） ============ -->
  {#if showSettings}
    <div class="overlay" role="presentation" onclick={() => (showSettings = false)}>
      <div
        class="conn-dialog settings-dialog"
        role="dialog"
        aria-label="设置"
        tabindex="-1"
        onclick={(e) => e.stopPropagation()}
        onkeydown={(e) => e.key === 'Escape' && (showSettings = false)}
      >
        <div class="dialog-head">
          <span class="dialog-title">⚙ 设置</span>
          <button class="dialog-close" onclick={() => (showSettings = false)}>×</button>
        </div>
        <div class="settings-body">
          <!-- 左栏：分类导航 -->
          <nav class="settings-nav" aria-label="设置分类">
            <button
              class="settings-item"
              class:active={settingsTab === 'general'}
              onclick={() => (settingsTab = 'general')}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <circle cx="12" cy="12" r="3" />
                <path d="M12 1v3M12 20v3M4.22 4.22l2.12 2.12M17.66 17.66l2.12 2.12M1 12h3M20 12h3M4.22 19.78l2.12-2.12M17.66 6.34l2.12-2.12" />
              </svg>
              常规
            </button>
            <button
              class="settings-item"
              class:active={settingsTab === 'update'}
              onclick={() => (settingsTab = 'update')}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <polyline points="23 4 23 10 17 10" />
                <polyline points="1 20 1 14 7 14" />
                <path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15" />
              </svg>
              更新
            </button>
            <button
              class="settings-item"
              class:active={settingsTab === 'about'}
              onclick={() => (settingsTab = 'about')}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <circle cx="12" cy="12" r="10" />
                <line x1="12" y1="16" x2="12" y2="12" />
                <line x1="12" y1="8" x2="12.01" y2="8" />
              </svg>
              关于
            </button>
          </nav>
          <!-- 右栏：内容区 -->
          <div class="settings-content">
            {#if settingsTab === 'general'}
              <div class="settings-title">常规</div>
              <div class="field">
                <label for="s-ps">数据页每页行数（10-500）</label>
                <input id="s-ps" type="number" min="10" max="500" bind:value={settingsPageSize} />
              </div>
              <div class="field-actions" style="margin-top:24px">
                <button onclick={saveSettings} class="primary">保存</button>
              </div>
            {:else if settingsTab === 'update'}
              <div class="settings-title">更新</div>
              <div class="field">
                <label for="s-update">检查更新</label>
                <div class="update-row">
                  <span class="ver-tag">当前 v{APP_VERSION}</span>
                  <button onclick={() => checkUpdate(false)}>🔍 检查更新</button>
                </div>
                {#if updateCheckMsg}
                  <div class="update-msg">{updateCheckMsg}</div>
                {/if}
              </div>
            {:else}
              <div class="settings-title">关于</div>
              <div class="about-box">
                <img src="/tusk-icon.png" class="about-logo" alt="Tusk" />
                <div class="about-name">Tusk v{APP_VERSION}</div>
                <div class="about-desc">PostgreSQL 管理客户端</div>
                <div class="about-stack">Tauri 2 · Rust · Svelte 5</div>
                <button onclick={() => invoke('open_url', { url: 'https://github.com/vpertj/tusk' })}>
                  GitHub 仓库 ↗
                </button>
              </div>
            {/if}
          </div>
        </div>
      </div>
    </div>
  {/if}

  <!-- ============ 对象搜索弹窗（Cmd+F） ============ -->
  {#if searchOpen}
    <div class="overlay" role="presentation" onclick={() => (searchOpen = false)}>
      <div
        class="conn-dialog search-dialog"
        role="dialog"
        aria-label="搜索对象"
        tabindex="-1"
        onclick={(e) => e.stopPropagation()}
        onkeydown={(e) => e.key === 'Escape' && (searchOpen = false)}
      >
        <div class="dialog-head">
          <span class="dialog-title">🔍 搜索对象 <kbd>⌘F</kbd></span>
          <button class="dialog-close" onclick={() => (searchOpen = false)}>×</button>
        </div>
        <div class="dialog-body">
          <div class="field">
            <input
              bind:value={searchQuery}
              placeholder="输入表名 / 视图名 / 数据库名…"
            />
          </div>
          <div class="search-list">
            {#if !searchQuery.trim()}
              <div class="search-empty">输入关键字开始搜索</div>
            {:else if searchResults().length === 0}
              <div class="search-empty">无匹配对象</div>
            {:else}
              {#each searchResults() as r (r.db + '.' + r.table)}
                <button
                  class="search-item"
                  onclick={() => openSearchResult(r)}
                  title={`打开 ${r.db}.${r.table}`}
                >
                  <span class="ico">{r.kind === 'view' ? '👁' : '📋'}</span>
                  <span class="s-db">{r.db}</span>
                  <span class="s-sep">.</span>
                  <span class="s-tbl">{r.table}</span>
                </button>
              {/each}
            {/if}
          </div>
        </div>
      </div>
    </div>
  {/if}

  <!-- ============ 新建视图弹窗 ============ -->
  {#if showViewDialog && viewDialog}
    <div class="overlay" role="presentation" onclick={() => (showViewDialog = false)}>
      <div
        class="conn-dialog view-dialog"
        role="dialog"
        aria-label="新建视图"
        tabindex="-1"
        onclick={(e) => e.stopPropagation()}
        onkeydown={(e) => e.key === 'Escape' && (showViewDialog = false)}
      >
        <div class="dialog-head">
          <span class="dialog-title">👁 新建视图</span>
          <button class="dialog-close" onclick={() => (showViewDialog = false)}>×</button>
        </div>
        <div class="dialog-body">
          {#if viewDialog.err}
            <div class="error">⚠ {viewDialog.err}</div>
          {/if}
          <div class="field">
            <label for="v-name">视图名</label>
            <input id="v-name" bind:value={viewDialog.name} placeholder="视图名" />
          </div>
          <div class="field">
            <label for="v-db">数据库</label>
            <select id="v-db" bind:value={viewDialog.db}>
              {#each (connDbs[connId] ?? []) as db}
                <option value={db.name}>{db.name}</option>
              {/each}
            </select>
          </div>
          <div class="field">
            <label for="v-sql">SELECT 语句</label>
            <textarea
              id="v-sql"
              bind:value={viewDialog.sql}
              rows="8"
              spellcheck="false"
              placeholder="SELECT ...（只允许 SELECT）"
            ></textarea>
          </div>
          <div class="field-actions" style="margin-top:16px">
            <button onclick={() => (showViewDialog = false)}>取消</button>
            <button onclick={doCreateView} class="primary">创建视图</button>
          </div>
        </div>
      </div>
    </div>
  {/if}

  <!-- ============ 连接右键菜单 ============ -->
  {#if connMenu}
    <div
      class="ctx-overlay"
      role="presentation"
      onclick={() => (connMenu = null)}
      oncontextmenu={(e) => {
        e.preventDefault();
        connMenu = null;
      }}
    ></div>
    <div class="ctx-menu" style="left:{connMenu!.x}px; top:{connMenu!.y}px">
      <div class="ctx-title">🔌 {connMenu!.name}</div>
      {#if connMenu!.connected}
        <button
          onclick={() => {
            const m = connMenu!;
            connMenu = null;
            connRowClick(m);
          }}
          >⇅ 展开/收起</button
        >
        <button
          class="ctx-danger"
          onclick={() => {
            const m = connMenu!;
            connMenu = null;
            disconnectConn(m.id);
          }}
          >⏻ 断开连接</button
        >
      {:else}
        <button
          onclick={() => {
            const m = connMenu!;
            connMenu = null;
            connectByName(m.name);
          }}
          >🔗 连接</button
        >
      {/if}
      <div class="ctx-sep"></div>
      <button
        class="ctx-danger"
        onclick={() => {
          const m = connMenu!;
          connMenu = null;
          confirmDrop = { conn: m.id, db: m.name, table: '', kind: 'connection' };
        }}
        >🗑 删除连接</button
      >
    </div>
  {/if}

  <!-- ============ 库右键菜单 ============ -->
  {#if dbMenu}
    <div
      class="ctx-overlay"
      role="presentation"
      onclick={() => (dbMenu = null)}
      oncontextmenu={(e) => {
        e.preventDefault();
        dbMenu = null;
      }}
    ></div>
    <div class="ctx-menu" style="left:{dbMenu!.x}px; top:{dbMenu!.y}px">
      <div class="ctx-title">🗄 {dbMenu!.db}</div>
      <button onclick={() => createTableIn(dbMenu!.conn, dbMenu!.db)}>＋ 在此库新建表</button>
      <button onclick={() => createViewIn(dbMenu!.conn, dbMenu!.db)}>＋ 在此库新建视图</button>
      <button onclick={() => newQueryIn(dbMenu!.conn, dbMenu!.db)}>⌨ 打开查询编辑器</button>
      <div class="ctx-sep"></div>
      <button onclick={() => reloadDb(dbMenu!.conn, dbMenu!.db)}>⟳ 刷新该库</button>
      <button
        class="ctx-danger"
        onclick={() => {
          confirmDrop = { conn: dbMenu!.conn, db: dbMenu!.db, table: '', kind: 'database' };
          dbMenu = null;
        }}
        >🗑 删除数据库</button
      >
    </div>
  {/if}

  <!-- ============ 树空白区右键菜单 ============ -->
  {#if blankMenu}
    <div
      class="ctx-overlay"
      role="presentation"
      onclick={() => (blankMenu = null)}
      oncontextmenu={(e) => {
        e.preventDefault();
        blankMenu = null;
      }}
    ></div>
    <div class="ctx-menu" style="left:{blankMenu!.x}px; top:{blankMenu!.y}px">
      <button onclick={openDbDialog}>＋ 新建数据库</button>
      <button onclick={() => loadDbs()} disabled={!connId}>⟳ 刷新全部</button>
    </div>
  {/if}

  <!-- ============ 表右键菜单 ============ -->
  {#if tableMenu}
    <div
      class="ctx-overlay"
      role="presentation"
      onclick={() => (tableMenu = null)}
      oncontextmenu={(e) => {
        e.preventDefault();
        tableMenu = null;
      }}
    ></div>
    <div class="ctx-menu" style="left:{tableMenu!.x}px; top:{tableMenu!.y}px">
      <button
        onclick={() => {
          openTableTab(tableMenu!.conn, tableMenu!.db, tableMenu!.table);
          tableMenu = null;
        }}
        >📖 打开</button
      >
      {#if tableMenu!.kind === 'table'}
        <button
          onclick={() => {
            openDesignerForEdit(tableMenu!.conn, tableMenu!.db, tableMenu!.table);
            tableMenu = null;
          }}
          >✎ 编辑表结构</button
        >
        <div class="ctx-sep"></div>
        <button
          onclick={() => {
            dupDialog = {
              conn: tableMenu!.conn,
              db: tableMenu!.db,
              table: tableMenu!.table,
              withData: false,
              name: `${tableMenu!.table}_copy`,
              err: '',
            };
            tableMenu = null;
          }}
          >⧉ 复制表结构</button
        >
        <button
          onclick={() => {
            dupDialog = {
              conn: tableMenu!.conn,
              db: tableMenu!.db,
              table: tableMenu!.table,
              withData: true,
              name: `${tableMenu!.table}_copy`,
              err: '',
            };
            tableMenu = null;
          }}
          >⧉ 复制表（含数据）</button
        >
        <div class="ctx-sep"></div>
        <button
          onclick={() => exportTableSqlFromTree(tableMenu!.conn, tableMenu!.db, tableMenu!.table)}
          >⬇ 导出 SQL</button
        >
      {/if}
      <div class="ctx-sep"></div>
      <button
        class="ctx-danger"
        onclick={() => {
          if (tableMenu!.kind === 'view') {
            dropViewFromTree(tableMenu!.conn, tableMenu!.db, tableMenu!.table);
          } else {
            dropTableFromTree(tableMenu!.conn, tableMenu!.db, tableMenu!.table);
          }
          tableMenu = null;
        }}
        >🗑 删除</button
      >
    </div>
  {/if}

  <!-- ============ 复制表（输入新表名） ============ -->
  {#if dupDialog}
    <div class="overlay" role="presentation" onclick={() => (dupDialog = null)}>
      <div
        class="conn-dialog confirm-dialog"
        role="dialog"
        aria-label="复制表"
        tabindex="-1"
        onclick={(e) => e.stopPropagation()}
        onkeydown={(e) => e.key === 'Escape' && (dupDialog = null)}
      >
        <div class="dialog-head">
          <span class="dialog-title">⧉ 复制表：{dupDialog.table}</span>
          <button class="dialog-close" onclick={() => (dupDialog = null)}>×</button>
        </div>
        <div class="dialog-body">
          {#if dupDialog.err}
            <div class="error">⚠ {dupDialog.err}</div>
          {/if}
          <div class="field">
            <label for="dup-name">新表名</label>
            <input id="dup-name" bind:value={dupDialog.name} placeholder="新表名" />
          </div>
          <div class="field-actions" style="margin-top:16px">
            <button onclick={() => (dupDialog = null)}>取消</button>
            <button onclick={doDuplicate} class="primary">
              {dupDialog.withData ? '复制表和数据' : '复制结构'}
            </button>
          </div>
        </div>
      </div>
    </div>
  {/if}

  <!-- ============ 删除表确认弹窗 ============ -->
  {#if confirmDrop}
    <div class="overlay" role="presentation" onclick={() => (confirmDrop = null)}>
      <div
        class="conn-dialog confirm-dialog"
        role="alertdialog"
        aria-label="确认删除"
        tabindex="-1"
        onclick={(e) => e.stopPropagation()}
        onkeydown={(e) => e.key === 'Escape' && (confirmDrop = null)}
      >
        <div class="dialog-head">
          <span class="dialog-title">
            {confirmDrop.kind === 'database'
              ? '🗑 删除数据库'
              : confirmDrop.kind === 'connection'
                ? '🗑 删除连接'
                : '🗑 删除表'}
          </span>
          <button class="dialog-close" onclick={() => (confirmDrop = null)}>×</button>
        </div>
        <div class="dialog-body">
          <p class="confirm-text">
            {#if confirmDrop.kind === 'view'}
              确定删除视图「<b>{confirmDrop.table}</b>」？
            {:else if confirmDrop.kind === 'database'}
              确定删除数据库「<b>{confirmDrop.db}</b>」？<br />
              <span class="confirm-warn">库中所有表和数据将全部删除，此操作不可撤销！</span>
            {:else if confirmDrop.kind === 'connection'}
              确定删除连接「<b>{confirmDrop.db}</b>」？<br />
              <span class="confirm-warn">该连接配置将被移除（不影响服务器上的数据）</span>
            {:else}
              确定删除表「<b>{confirmDrop.table}</b>」？<br />
              <span class="confirm-warn">表中数据将全部丢失，此操作不可撤销！</span>
            {/if}
          </p>
          <div class="field-actions" style="margin-top:18px">
            <button onclick={() => (confirmDrop = null)}>取消</button>
            <button onclick={doDropConfirm} class="danger">确认删除</button>
          </div>
        </div>
      </div>
    </div>
  {/if}

  <!-- ============ 底部状态栏 ============ -->
    <Footer {connId} {connMeta} appVersion={APP_VERSION} {openSettings} />
</div>

<style>
  :global(body) {
    margin: 0;
    background: #16181d;
    color: #d7dae0;
    font-family: -apple-system, 'PingFang SC', 'Microsoft YaHei', sans-serif;
    font-size: 13px;
    overflow: hidden;
  }

  .app {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }

  /* ===== 顶部工具栏 ===== */

  .status {
    color: #8b93a3;
    font-size: 12px;
    white-space: nowrap;
    max-width: 460px;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .status.ok {
    color: #57c98a;
  }

  .status.err {
    color: #e05656;
  }

  .ver {
    margin-left: 8px;
    color: #6b7484;
    font-size: 11px;
  }

  /* ===== 主区 ===== */
  main {
    flex: 1;
    display: flex;
    min-height: 0;
  }

  /* ===== 连接管理弹窗 ===== */
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 100;
    background: rgba(0, 0, 0, 0.55);
    backdrop-filter: blur(3px);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .conn-dialog {
    width: 560px;
    max-width: calc(100vw - 48px);
    max-height: 82vh;
    overflow-y: auto;
    background: #1e2128;
    border: 1px solid #363b47;
    border-radius: 12px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.5);
    display: flex;
    flex-direction: column;
  }

  /* 新建数据库弹窗 */
  .db-dialog {
    width: 400px;
  }

  .dialog-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 14px 18px;
    border-bottom: 1px solid #2c303a;
    position: sticky;
    top: 0;
    background: #1e2128;
    border-radius: 12px 12px 0 0;
  }

  .dialog-title {
    font-size: 14px;
    font-weight: 600;
    color: #d7dae0;
  }

  .dialog-close {
    background: transparent;
    border: none;
    color: #8b93a3;
    font-size: 18px;
    padding: 2px 8px;
    border-radius: 6px;
    cursor: pointer;
    line-height: 1;
  }

  .dialog-close:hover {
    background: #363b47;
    color: #e05656;
  }

  .dialog-body {
    padding: 14px 18px 18px;
  }

  .divider {
    height: 1px;
    background: #2c303a;
    margin: 12px 0;
  }

  /* 新建连接纵向表单 */
  .conn-form-v {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .field {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .field label {
    width: 84px;
    flex-shrink: 0;
    color: #aab2c0;
    font-size: 12px;
    text-align: right;
  }

  .field input,
  .field select {
    flex: 1;
    background: #262a33;
    border: 1px solid #363b47;
    border-radius: 6px;
    color: #d7dae0;
    padding: 7px 12px;
    font-size: 12px;
  }

  .field input:focus,
  .field select:focus {
    outline: none;
    border-color: #4fc3f7;
  }

  .field select {
    appearance: none;
    cursor: pointer;
    background-image: linear-gradient(45deg, transparent 50%, #8b93a3 50%),
      linear-gradient(135deg, #8b93a3 50%, transparent 50%);
    background-position: calc(100% - 16px) 55%, calc(100% - 11px) 55%;
    background-size: 5px 5px;
    background-repeat: no-repeat;
  }

  .field-actions {
    display: flex;
    flex-direction: row;
    justify-content: flex-end;
    align-items: center;
    gap: 10px;
    margin-top: 6px;
    padding-top: 12px;
    border-top: 1px solid #2c303a;
  }

  .field-actions button.primary {
    min-width: 90px;
  }

  .save-label {
    color: #8b93a3;
    font-size: 12px;
    display: inline-flex;
    align-items: center;
    gap: 3px;
    white-space: nowrap;
    cursor: pointer;
  }


  .saved-title {
    font-size: 11px;
    color: #6b7484;
    letter-spacing: 1px;
    margin-bottom: 8px;
  }

  .saved-item {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 6px;
  }

  .saved-connect {
    flex: 1;
    text-align: left;
    background: #262a33;
    border: 1px solid #363b47;
    border-radius: 8px;
    color: #d7dae0;
    padding: 9px 14px;
    font-size: 13px;
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 12px;
    transition: border-color 0.12s, background 0.12s;
  }

  .saved-connect:hover:not(:disabled) {
    border-color: #4fc3f7;
    background: #2a2f3a;
  }

  .saved-connect:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .conn-ico {
    font-size: 16px;
  }

  .conn-main {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .conn-name {
    color: #d7dae0;
    font-weight: 600;
  }

  .conn-sub {
    color: #6b7484;
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .badge {
    background: #2f6fed22;
    border: 1px solid #2f6fed55;
    color: #4fc3f7;
    font-size: 10px;
    padding: 1px 6px;
    border-radius: 4px;
    flex-shrink: 0;
  }

  .saved-del {
    background: transparent;
    border: 1px solid #363b47;
    border-radius: 8px;
    color: #8b93a3;
    padding: 10px 12px;
    font-size: 14px;
    cursor: pointer;
    line-height: 1;
  }

  .saved-del:hover {
    border-color: #d64545;
    color: #e05656;
  }

  input {
    background: #262a33;
    border: 1px solid #363b47;
    border-radius: 6px;
    color: #d7dae0;
    padding: 6px 10px;
    font-size: 12px;
    min-width: 60px;
    flex: 1;
  }

  input:focus {
    outline: none;
    border-color: #4fc3f7;
  }

  .workspace {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .tabbar {
    display: flex;
    background: #171a20;
    border-bottom: 1px solid #2c303a;
    min-height: 32px;
    overflow-x: auto;
  }

  .tab {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 7px 10px 7px 14px;
    border-right: 1px solid #23262e;
    color: #8b93a3;
    cursor: pointer;
    white-space: nowrap;
    font-size: 12px;
  }

  .tab.active {
    background: #1e2128;
    color: #d7dae0;
    border-top: 2px solid #4fc3f7;
  }

  .tab-close {
    color: #5c6472;
    padding: 0 3px;
    border-radius: 3px;
  }

  .tab-close:hover {
    background: #363b47;
    color: #e05656;
  }

  .tab-content {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  .tab-ico {
    font-size: 11px;
    opacity: 0.8;
  }

  /* 查询结果导出栏 + 消息 */
  .query-export-bar {
    display: flex;
    justify-content: flex-end;
    padding: 6px 14px 0;
  }

  .query-export-bar button {
    background: #262a33;
    border: 1px solid #363b47;
    border-radius: 6px;
    color: #aab2c0;
    font-size: 12px;
    padding: 4px 12px;
    cursor: pointer;
  }

  .query-export-bar button:hover {
    border-color: #4fc3f7;
  }

  .query-msg {
    color: #4caf50;
    font-size: 12px;
    padding: 8px 14px 0;
  }

  /* Explain 执行计划 */
  .explain-box {
    margin: 10px 14px 0;
    background: #161a22;
    border: 1px solid #2c303a;
    border-radius: 8px;
    padding: 10px 14px;
  }

  .explain-title {
    font-size: 11px;
    color: #6b7484;
    letter-spacing: 1px;
    margin-bottom: 8px;
  }

  .explain-box pre {
    margin: 0;
    color: #9fe8b0;
    font-family: 'SF Mono', Menlo, monospace;
    font-size: 12px;
    line-height: 1.55;
    white-space: pre-wrap;
    word-break: break-all;
  }

  /* 新增行弹窗 */
  .insert-dialog {
    width: 460px;
    max-height: 82vh;
    overflow-y: auto;
  }

  .field-hint {
    color: #5c6472;
    font-size: 11px;
    font-weight: 400;
    margin-left: 6px;
  }

  /* 检查更新 */
  .update-row {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .ver-tag {
    color: #8b93a3;
    font-size: 12px;
    background: #242833;
    border: 1px solid #363b47;
    border-radius: 10px;
    padding: 2px 10px;
  }

  .update-msg {
    color: #8b93a3;
    font-size: 12px;
    padding-top: 6px;
  }

  .update-dialog {
    width: 440px;
  }

  .update-desc {
    color: #d7dae0;
    font-size: 13px;
    margin: 0 0 10px;
  }

  .update-notes {
    background: #1c2029;
    border: 1px solid #2c303a;
    border-radius: 6px;
    padding: 10px 12px;
    max-height: 180px;
    overflow-y: auto;
    color: #aab2c0;
    font-size: 12px;
    white-space: pre-wrap;
  }

  /* 结构同步弹窗 */
  .sync-dialog {
    width: 620px;
  }

  /* 同步模式切换 */
  .sync-mode {
    display: flex;
    gap: 8px;
    margin-bottom: 12px;
  }

  .sync-mode-btn {
    flex: 1;
    padding: 7px 0;
    border-radius: 6px;
    font-size: 12px;
    border: 1px solid #2c303a;
    background: transparent;
    color: #9aa3b2;
    cursor: pointer;
  }

  .sync-mode-btn:hover {
    background: #262b36;
    color: #f0f2f5;
  }

  .sync-mode-btn.active {
    background: #1d3a6b;
    border-color: #2f6fed;
    color: #8ab4ff;
  }

  .sync-hint {
    margin: 8px 0 0;
    font-size: 11px;
    color: #c9a227;
  }

  .sync-pair {
    display: flex;
    align-items: flex-end;
    gap: 12px;
  }

  .sync-pair .field {
    flex: 1;
  }

  .sync-arrow {
    font-size: 20px;
    color: #6f7a8d;
    padding-bottom: 6px;
  }

  .sync-err {
    color: #e05656;
    font-size: 12px;
    padding: 6px 0;
  }

  .sync-ok {
    color: #4caf50;
    font-size: 12px;
    padding: 6px 0;
  }

  .sync-list {
    border: 1px solid #2c303a;
    border-radius: 6px;
    background: #1c2029;
    max-height: 300px;
    overflow-y: auto;
  }

  .sync-head,
  .sync-item {
    display: grid;
    grid-template-columns: 1.2fr 0.7fr 2.2fr;
    gap: 8px;
    padding: 6px 10px;
    align-items: center;
  }

  .sync-head {
    background: #242833;
    color: #8b93a3;
    font-size: 11px;
    position: sticky;
    top: 0;
  }

  .sync-item {
    border-top: 1px solid #242833;
    font-size: 12px;
  }

  .sync-check {
    display: flex;
    align-items: center;
    gap: 6px;
    color: #d7dae0;
  }

  .sync-act {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 10px;
    text-align: center;
    width: fit-content;
  }

  .sync-act.create {
    background: #1e3a2a;
    color: #4caf50;
  }

  .sync-act.alter {
    background: #3a3420;
    color: #e0b34c;
  }

  .sync-act.drop {
    background: #3a2020;
    color: #e05656;
  }

  .sync-sql {
    color: #6f7a8d;
    font-family: ui-monospace, monospace;
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* 设置弹窗（左右分栏） */
  .settings-dialog {
    width: 560px;
  }

  .settings-body {
    display: flex;
    min-height: 300px;
  }

  .settings-nav {
    width: 150px;
    flex-shrink: 0;
    border-right: 1px solid #2c303a;
    padding: 10px 8px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .settings-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 10px;
    border-radius: 6px;
    border: none;
    background: transparent;
    color: #aab2c0;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
  }

  .settings-item svg {
    width: 15px;
    height: 15px;
    flex-shrink: 0;
  }

  .settings-item:hover {
    background: #232833;
    color: #e8ebf0;
  }

  .settings-item.active {
    background: #2a3345;
    color: #4fc3f7;
    font-weight: 600;
  }

  .settings-content {
    flex: 1;
    padding: 18px 22px;
    min-width: 0;
  }

  .settings-title {
    font-size: 14px;
    font-weight: 700;
    color: #e8ebf0;
    margin-bottom: 18px;
  }

  /* 关于页 */
  .about-box {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 10px 0;
  }

  .about-logo {
    width: 64px;
    height: 64px;
    border-radius: 14px;
    margin-bottom: 6px;
  }

  .about-name {
    font-size: 16px;
    font-weight: 700;
    color: #e8ebf0;
  }

  .about-desc {
    font-size: 13px;
    color: #8b93a3;
  }

  .about-stack {
    font-size: 11px;
    color: #5c6472;
    margin-bottom: 10px;
  }

  /* 更新进度条 */
  .update-progress {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
  }

  .update-bar {
    flex: 1;
    height: 6px;
    background: #14171d;
    border: 1px solid #2c303a;
    border-radius: 3px;
    overflow: hidden;
  }

  .update-bar-fill {
    height: 100%;
    background: #2f6fed;
    border-radius: 3px;
    transition: width 0.15s ease;
  }

  .update-pct {
    color: #8b93a3;
    font-size: 12px;
    min-width: 38px;
    text-align: right;
  }

  /* 对象搜索弹窗 */
  .search-dialog {
    width: 420px;
  }

  .search-list {
    max-height: 320px;
    overflow-y: auto;
    margin-top: 8px;
    border: 1px solid #2c303a;
    border-radius: 6px;
    background: #1c2029;
  }

  .search-empty {
    color: #5c6472;
    font-size: 12px;
    padding: 18px;
    text-align: center;
  }

  .search-item {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 7px 10px;
    background: transparent;
    border: none;
    border-bottom: 1px solid #242833;
    color: #d7dae0;
    font-size: 12px;
    text-align: left;
    cursor: pointer;
  }

  .search-item:last-child {
    border-bottom: none;
  }

  .search-item:hover {
    background: #242833;
  }

  .search-item .s-db {
    color: #6f7a8d;
  }

  .search-item .s-sep {
    color: #4a5264;
  }

  .search-item .s-tbl {
    color: #e8ebf0;
    font-weight: 600;
  }

  /* 新建视图弹窗 */
  .view-dialog {
    width: 560px;
  }

  .view-dialog textarea {
    width: 100%;
    background: #1c2029;
    border: 1px solid #363b47;
    border-radius: 6px;
    color: #d7dae0;
    font-size: 12px;
    font-family: 'SF Mono', Menlo, monospace;
    padding: 8px 10px;
    resize: vertical;
  }

  .view-dialog textarea:focus {
    outline: none;
    border-color: #4fc3f7;
  }

  /* PG 环境引导 */
  .pg-help {
    margin-top: 14px;
    background: #1c2029;
    border: 1px solid #3a3420;
    border-radius: 8px;
    padding: 12px 14px;
  }

  .pg-help-title {
    color: #e0b34c;
    font-size: 13px;
    font-weight: 600;
    margin-bottom: 6px;
  }

  .pg-help-desc {
    color: #aab2c0;
    font-size: 12px;
    margin: 0 0 10px;
    line-height: 1.6;
  }

  .pg-help-cmd {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 8px;
  }



  .pg-help-note {
    color: #5c6472;
    font-size: 11px;
    margin: 4px 0 0;
  }

  /* 库右键菜单标题 */
  .ctx-title {
    padding: 6px 12px 8px;
    font-size: 11px;
    color: #8b93a3;
    border-bottom: 1px solid #23262e;
    margin-bottom: 4px;
    white-space: nowrap;
  }

  /* 右键菜单 */
  .ctx-overlay {
    position: fixed;
    inset: 0;
    z-index: 200;
  }

  .ctx-menu {
    position: fixed;
    z-index: 201;
    min-width: 180px;
    background: #22262f;
    border: 1px solid #363b47;
    border-radius: 8px;
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.5);
    padding: 5px;
    display: flex;
    flex-direction: column;
  }

  .ctx-menu button {
    background: transparent;
    border: none;
    border-radius: 5px;
    color: #d7dae0;
    font-size: 12px;
    text-align: left;
    padding: 7px 10px;
    cursor: pointer;
  }

  .ctx-menu button:hover {
    background: #2f6fed;
    color: #fff;
  }

  .ctx-sep {
    height: 1px;
    background: #363b47;
    margin: 4px 6px;
  }

  .ctx-menu button.ctx-danger:hover {
    background: #d64545;
  }

  /* 删除表确认弹窗 */
  .confirm-dialog {
    width: 420px;
  }

  .confirm-text {
    color: #d7dae0;
    font-size: 13px;
    line-height: 1.7;
    margin: 4px 0 0;
  }

  .confirm-text b {
    color: #e05656;
  }

  .confirm-warn {
    color: #8b93a3;
    font-size: 12px;
  }

  button.danger {
    background: #d64545;
  }

  button.danger:hover:not(:disabled) {
    background: #e05656;
  }

  /* ===== 表设计器 ===== */
  .designer-dialog {
    width: 860px;
  }

  .designer-grid {
    border: 1px solid #2c303a;
    border-radius: 8px;
    overflow: hidden;
  }

  .dg-head,
  .dg-row {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px;
  }

  .dg-head {
    background: #242833;
    color: #8b93a3;
    font-size: 11px;
    padding-top: 6px;
    padding-bottom: 6px;
  }

  .dg-row {
    border-top: 1px solid #2c303a;
    background: #1b1e25;
  }

  .dg-row:hover {
    background: #20242e;
  }

  .dg-pk {
    width: 46px;
    flex-shrink: 0;
    text-align: center;
  }

  .dg-serial {
    width: 44px;
    flex-shrink: 0;
    text-align: center;
  }

  .dg-name {
    flex: 1.3;
    min-width: 90px;
  }

  .dg-type {
    flex: 1;
    min-width: 90px;
  }

  .dg-len {
    flex: 0.8;
    min-width: 70px;
  }

  .dg-null {
    width: 42px;
    flex-shrink: 0;
    text-align: center;
  }

  .dg-def {
    flex: 1.1;
    min-width: 80px;
  }

  .dg-cmt {
    flex: 1.2;
    min-width: 90px;
  }

  .dg-del {
    width: 28px;
    flex-shrink: 0;
    text-align: center;
  }

  .dg-row input:not([type]) {
    width: 100%;
    padding: 4px 8px;
    font-size: 12px;
  }

  .dg-row select {
    width: 100%;
    padding: 4px 6px;
    font-size: 12px;
  }

  .dg-del button {
    background: transparent;
    border: none;
    color: #8b93a3;
    font-size: 15px;
    cursor: pointer;
    padding: 0 4px;
    line-height: 1;
  }

  .dg-del button:hover {
    color: #e05656;
  }

  .add-col {
    margin-top: 8px;
    background: #262a33;
    border: 1px dashed #4a5264;
    border-radius: 6px;
    color: #aab2c0;
    padding: 5px 14px;
    font-size: 12px;
    cursor: pointer;
  }

  .add-col:hover {
    border-color: #4fc3f7;
    color: #d7dae0;
  }

  /* ===== 表设计器 ===== */

  /* 结构子标签工具栏 */
  .struct-toolbar {
    display: flex;
    align-items: center;
    padding: 8px 14px;
    background: #171a20;
    border-bottom: 1px solid #2c303a;
  }

  .struct-toolbar button {
    background: #262a33;
    border: 1px solid #363b47;
    border-radius: 6px;
    color: #aab2c0;
    padding: 4px 12px;
    font-size: 12px;
    cursor: pointer;
  }

  .struct-toolbar button:hover {
    border-color: #4fc3f7;
    color: #d7dae0;
  }

  .ok-msg {
    color: #4caf50;
    font-size: 12px;
    margin-left: 10px;
  }

  /* 筛选栏 */
  .filter-bar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 7px 14px;
    background: #161a22;
    border-bottom: 1px solid #2c303a;
    flex-wrap: wrap;
  }

  .filter-bar select {
    background: #22262f;
    border: 1px solid #363b47;
    border-radius: 6px;
    color: #aab2c0;
    font-size: 12px;
    padding: 4px 8px;
  }

  .filter-bar input {
    background: #1c2029;
    border: 1px solid #363b47;
    border-radius: 6px;
    color: #d7dae0;
    font-size: 12px;
    padding: 4px 8px;
    width: 140px;
  }

  .filter-bar input:disabled {
    opacity: 0.4;
  }

  .filter-bar button {
    background: #262a33;
    border: 1px solid #363b47;
    border-radius: 6px;
    color: #aab2c0;
    font-size: 12px;
    padding: 4px 10px;
    cursor: pointer;
  }

  .filter-bar button:hover {
    border-color: #4fc3f7;
  }

  .filter-bar button.filter-del {
    padding: 4px 7px;
    color: #e05656;
  }

  .filter-bar button.filter-apply {
    background: #2f6fed;
    border-color: #2f6fed;
    color: #fff;
  }

  .filter-bar button.filter-clear:hover {
    border-color: #d64545;
    color: #e05656;
  }

  .filter-on {
    color: #4fc3f7;
    font-size: 12px;
  }

  .idx-title {
    color: #aab2c0;
    font-size: 12px;
    padding: 14px 0 6px;
    border-top: 1px solid #2c303a;
    margin-top: 14px;
  }

  .idx-empty {
    color: #5c6472;
    font-size: 12px;
    padding: 4px 0;
  }

  /* 数据页工具栏 */
  .data-toolbar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 14px;
    background: #171a20;
    border-bottom: 1px solid #2c303a;
  }

  .data-toolbar button {
    background: #262a33;
    border: 1px solid #363b47;
    border-radius: 6px;
    color: #aab2c0;
    padding: 4px 12px;
    font-size: 12px;
    cursor: pointer;
  }

  .data-toolbar button:hover:not(:disabled) {
    border-color: #4fc3f7;
    color: #d7dae0;
  }

  .data-toolbar button:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .data-toolbar button.danger {
    border-color: #7a2e2e;
    color: #e05656;
  }

  .data-toolbar button.danger:hover:not(:disabled) {
    border-color: #d64545;
    background: #3a2020;
  }

  .toolbar-hint {
    margin-left: auto;
    color: #5c6472;
    font-size: 11px;
  }

  /* 单元格编辑输入框 */
  .cell-input {
    background: #1b1e25;
    border: 1px solid #4fc3f7;
    border-radius: 4px;
    color: #d7dae0;
    font-family: 'SF Mono', Menlo, monospace;
    font-size: 12px;
    padding: 2px 6px;
    width: 100%;
    box-sizing: border-box;
  }

  .cell-input:focus {
    outline: none;
  }

  /* 选中行 */
  tbody tr.selected {
    background: #24324a;
  }

  tbody tr.selected:hover {
    background: #2a3b58;
  }

  /* 表页签子标签栏 */
  .subtabbar {
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 0 14px;
    background: #171a20;
    border-bottom: 1px solid #2c303a;
    min-height: 34px;
  }

  .subtab {
    padding: 8px 14px;
    color: #8b93a3;
    cursor: pointer;
    font-size: 12px;
    border-bottom: 2px solid transparent;
    user-select: none;
  }

  .subtab:hover {
    color: #d7dae0;
  }

  .subtab.active {
    color: #4fc3f7;
    border-bottom-color: #4fc3f7;
  }

  .subtab-spacer {
    flex: 1;
  }

  .subtab-hint {
    color: #5c6472;
    font-size: 11px;
  }

  .pager {
    display: inline-flex;
    gap: 6px;
    margin-left: auto;
  }

  .pager button {
    background: #262a33;
    border: 1px solid #363b47;
    border-radius: 5px;
    color: #aab2c0;
    padding: 2px 10px;
    font-size: 11px;
    cursor: pointer;
  }

  .pager button:hover:not(:disabled) {
    background: #2f6fed;
    color: #fff;
    border-color: #2f6fed;
  }

  .pager button:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .count {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .sql-preview {
    padding: 16px;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 12px;
  }

  .sql-preview pre {
    margin: 0;
    padding: 14px 16px;
    background: #242833;
    border: 1px solid #363b47;
    border-radius: 8px;
    color: #c9e2b4;
    font-family: 'SF Mono', Menlo, monospace;
    font-size: 13px;
    white-space: pre-wrap;
  }

  .editor {
    padding: 12px 14px 8px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  textarea {
    width: 100%;
    height: 140px;
    background: #1b1e25;
    border: 1px solid #2c303a;
    border-radius: 8px;
    color: #c9e2b4;
    font-family: 'SF Mono', Menlo, Consolas, monospace;
    font-size: 13px;
    padding: 10px 12px;
    resize: vertical;
    box-sizing: border-box;
    line-height: 1.5;
  }

  textarea:focus {
    outline: none;
    border-color: #4fc3f7;
  }

  .editor-bar {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .hint {
    color: #5c6472;
    font-size: 11px;
  }

  .result {
    flex: 1;
    margin: 0 14px 14px;
    background: #1b1e25;
    border: 1px solid #2c303a;
    border-radius: 8px;
    overflow-y: auto;
    min-height: 0;
  }

  .result-block {
    border-bottom: 1px solid #23262e;
  }

  .result-block:last-child {
    border-bottom: none;
  }

  .error {
    background: #3a2226;
    color: #ff8a8a;
    padding: 10px 14px;
    border-bottom: 1px solid #4a2e33;
    font-family: Menlo, monospace;
    font-size: 12px;
    white-space: pre-wrap;
  }

  .ok {
    background: #1d3529;
    color: #7fe0a8;
    padding: 8px 14px;
    border-bottom: 1px solid #2c4a38;
  }

  .table-wrap {
    overflow: auto;
  }

  table {
    border-collapse: collapse;
    width: 100%;
    font-size: 12px;
  }

  th {
    position: sticky;
    top: 0;
    background: #242833;
    color: #aab2c0;
    text-align: left;
    padding: 8px 12px;
    border-bottom: 1px solid #363b47;
    border-right: 1px solid #2e3340;
    white-space: nowrap;
    z-index: 1;
    position: relative;
  }

  th:last-child {
    border-right: none;
  }

  .resizer {
    position: absolute;
    top: 0;
    right: 0;
    width: 7px;
    height: 100%;
    cursor: col-resize;
    user-select: none;
    z-index: 2;
  }

  /* 常驻细线：提示每一列的拖拽边界 */
  .resizer::after {
    content: '';
    position: absolute;
    top: 0;
    right: 3px;
    width: 1px;
    height: 100%;
    background: #454c5c;
  }

  .resizer:hover::after {
    right: 2.5px;
    width: 2px;
    background: #4fc3f7;
  }

  .resizer:hover {
    background: rgba(79, 195, 247, 0.12);
  }

  th small {
    display: block;
    color: #5c6472;
    font-weight: 400;
    font-size: 10px;
  }

  td {
    padding: 6px 12px;
    border-bottom: 1px solid #23262e;
    white-space: nowrap;
    font-family: 'SF Mono', Menlo, monospace;
    font-size: 12px;
  }

  td.null {
    color: #5c6472;
    font-style: italic;
  }

  tbody tr:hover {
    background: #20242c;
  }

  .count {
    padding: 8px 14px;
    color: #6b7484;
    font-size: 11px;
    border-top: 1px solid #2c303a;
  }

  .empty {
    margin: auto;
    color: #4c5462;
    font-size: 13px;
  }

  /* ===== 底部状态栏 ===== */
</style>

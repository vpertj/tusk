<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
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
  }
  loadSavedConns();

  // ================= 连接状态 =================
  let connId = $state('');
  let version = $state('');
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
  let editingTable = $state<{ db: string; table: string } | null>(null);
  let designerDb = $state('');
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
    designerDb = dbs[0]?.name ?? dbname;
    designerName = '';
    designerError = '';
    designerCols = [
      { id: ++designerSeq, name: 'id', baseType: 'serial', length: '', nullable: false, default: '', isPk: true, isSerial: true, comment: '' },
      { id: ++designerSeq, name: 'name', baseType: 'text', length: '', nullable: false, default: '', isPk: false, isSerial: false, comment: '' },
    ];
    showDesigner = true;
  }

  // 打开已有表的设计器（预填当前结构）
  async function openDesignerForEdit(db: string, table: string) {
    try {
      const cols = await invoke<SchemaColumn[]>('list_columns', { connId, dbname: db, table });
      editingTable = { db, table };
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
        refreshTables(editingTable.db);
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
        await refreshTables(designerDb);
        openTableTab(designerDb, designerName.trim());
      }
      showDesigner = false;
      editingTable = null;
    } catch (e) {
      designerError = String(e);
    }
  }

  // 刷新库的表列表（保持展开状态）
  async function refreshTables(db: string) {
    if (!connId) return;
    try {
      tables[db] = await invoke<TableInfo[]>('list_tables', { connId, dbname: db });
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
      const sql = await invoke<string>('export_sql', {
        connId,
        dbname: t.dbname,
        table: t.table,
      });
      const ts = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
      const path = `~/Downloads/tusk-${t.table}-${ts}.sql`;
      await invoke('write_text_file', { path, content: sql });
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
  let confirmDrop = $state<{ db: string; table: string; kind?: string } | null>(null);

  // ===== 表右键菜单 + 复制表 =====
  let tableMenu = $state<{ x: number; y: number; db: string; table: string; kind: string } | null>(
    null,
  );
  let dupDialog = $state<{
    db: string;
    table: string;
    withData: boolean;
    name: string;
    err: string;
  } | null>(null);

  function openTableMenu(e: MouseEvent, db: string, table: string, kind = 'table') {
    e.preventDefault();
    tableMenu = { x: e.clientX, y: e.clientY, db, table, kind };
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
        connId,
        dbname: d.db,
        srcTable: d.table,
        newTable: d.name.trim(),
        withData: d.withData,
      });
      dupDialog = null;
      await refreshTables(d.db);
      openTableTab(d.db, d.name.trim());
    } catch (e) {
      d.err = String(e);
    }
  }

  // 删除视图（对象树入口）
  function dropViewFromTree(db: string, view: string) {
    confirmDrop = { db, table: view, kind: 'view' };
  }

  // ===== 新建视图 =====
  let showViewDialog = $state(false);
  let viewDialog = $state<{ db: string; name: string; sql: string; err: string } | null>(null);

  function openViewDialog() {
    viewDialog = { db: dbs[0]?.name ?? dbname, name: '', sql: 'SELECT\n  *\nFROM\n  "public"."表名"', err: '' };
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
        connId,
        dbname: d.db,
        viewName: d.name.trim(),
        selectSql: d.sql,
      });
      showViewDialog = false;
      await refreshTables(d.db);
      openTableTab(d.db, d.name.trim());
    } catch (e) {
      d.err = String(e);
    }
  }

  // 删除表（对象树入口）
  function dropTableFromTree(db: string, table: string) {
    confirmDrop = { db, table };
  }

  // 删除表（表页签工具栏）
  function dropTable(raw: QueryTab) {
    const t = resolveTab(raw);
    confirmDrop = { db: t.dbname!, table: t.table! };
  }

  async function doDropConfirm() {
    const cd = confirmDrop;
    if (!cd) return;
    confirmDrop = null;
    try {
      if (cd.kind === 'view') {
        await invoke('drop_view', { connId, dbname: cd.db, viewName: cd.table });
      } else {
        await invoke('drop_table', { connId, dbname: cd.db, table: cd.table });
      }
      const openTab = tabs.find((t) => t.kind === 'table' && t.dbname === cd.db && t.table === cd.table);
      if (openTab) closeTab(openTab.id);
      await refreshTables(cd.db);
    } catch (e) {
      status = `删除失败: ${e}`;
    }
  }
  let treeOpen = $state<Record<string, boolean>>({}); // key: db / db.table
  let tables = $state<Record<string, TableInfo[]>>({});
  let columns = $state<Record<string, SchemaColumn[]>>({});
  let loadingKey = $state('');

  // ===== 对象搜索（Cmd+F） =====
  let searchOpen = $state(false);
  let searchQuery = $state('');

  function searchResults() {
    const q = searchQuery.trim().toLowerCase();
    if (!q) return [];
    const out: { db: string; table: string; kind: string }[] = [];
    for (const db of dbs) {
      const dbs_match = db.name.toLowerCase().includes(q);
      for (const t of tables[db.name] ?? []) {
        if (dbs_match || t.name.toLowerCase().includes(q)) {
          out.push({ db: db.name, table: t.name, kind: t.kind });
          if (out.length >= 50) return out;
        }
      }
    }
    return out;
  }

  function openSearchResult(r: { db: string; table: string }) {
    openTableTab(r.db, r.table);
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
  let settingsPageSize = $state(50);
  try {
    settingsPageSize = Number(localStorage.getItem('tusk.pageSize')) || 50;
  } catch {
    // 忽略
  }

  function openSettings() {
    settingsPageSize = Number(localStorage.getItem('tusk.pageSize')) || 50;
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
      const info = await invoke<{ id: string; version: string }>('connect', {
        host,
        port,
        user,
        password,
        dbname,
      });
      connId = info.id;
      version = info.version;
      status = `已连接 · ${user}@${host}:${port}`;
      showConnPanel = false;
      ensureTab();
      await loadDbs();
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
    }
    connecting = false;
  }

  // 用已保存的连接一键连接
  async function connectSaved(name: string) {
    connecting = true;
    try {
      const info = await invoke<{ id: string; version: string }>('connect_saved', { name });
      connId = info.id;
      version = info.version;
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
    try {
      await invoke('disconnect', { connId });
    } catch {
      // 忽略
    }
    connId = '';
    version = '';
    status = '未连接';
    dbs = [];
    treeOpen = {};
    tables = {};
    columns = {};
    tabs = [];
    activeTabId = 0;
    showConnPanel = true;
  }

  // ================= 对象树加载 =================
  async function loadDbs() {
    if (!connId) return;
    try {
      dbs = await invoke<DatabaseInfo[]>('list_databases', { connId });
    } catch (e) {
      status = `加载数据库失败: ${e}`;
    }
  }

  async function toggleDb(db: string) {
    const key = db;
    treeOpen[key] = !treeOpen[key];
    if (treeOpen[key] && !tables[key]) {
      loadingKey = key;
      try {
        tables[key] = await invoke<TableInfo[]>('list_tables', { connId, dbname: db });
      } catch (e) {
        status = `加载表失败: ${e}`;
      }
      loadingKey = '';
    }
  }

  async function toggleTable(db: string, table: string) {
    const key = `${db}.${table}`;
    treeOpen[key] = !treeOpen[key];
    if (treeOpen[key] && !columns[key]) {
      loadingKey = key;
      try {
        columns[key] = await invoke<SchemaColumn[]>('list_columns', {
          connId,
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
  function openTableTab(db: string, table: string) {
    const exist = tabs.find((t) => t.kind === 'table' && t.dbname === db && t.table === table);
    if (exist) {
      activeTabId = exist.id;
      return;
    }
    const t: QueryTab = {
      id: tabSeq++,
      kind: 'table',
      title: table,
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
    if (!connId || !t.dbname || !t.table) return;
    t.loading = true;
    t.error = '';
    try {
      const res = await invoke<{
        columns: { name: string; type_name: string }[];
        rows: unknown[][];
        total: number | null;
      }>('paginate_table', {
        connId,
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
    if (!connId || !t.dbname || !t.table) return;
    if (t.structure) return;
    try {
      t.structure = await invoke<SchemaColumn[]>('list_columns', {
        connId,
        dbname: t.dbname,
        table: t.table,
      });
      t.indexes = await invoke<IndexInfo[]>('list_indexes', {
        connId,
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
        connId,
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
      await invoke('insert_row', { connId, dbname: t.dbname, table: t.table });
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
        connId,
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
        connId,
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
    if (!t || !connId || !t.sql.trim()) return;
    t.running = true;
    try {
      t.explainText = await invoke<string>('explain_query', { connId, sql: t.sql });
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
    if (!t || !connId || !t.sql.trim()) return;
    t.running = true;
    t.error = '';
    t.explainText = undefined; // 执行新查询时清掉旧执行计划
    const t0 = performance.now();
    try {
      const res = await invoke<{ results: QueryResultView[] }>('query', {
        connId,
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
  <header>
    <div class="logo">🐘 Tusk</div>
    <div class="toolbar">
      <button onclick={openNewTab} disabled={!connId} title="新建查询 (Cmd+N)">＋ 新建查询</button>
      <input
        type="file"
        accept=".sql,.txt,text/plain"
        id="sql-file-input"
        hidden
        onchange={onSqlFilePicked}
      />
      <button
        onclick={() => document.getElementById('sql-file-input')?.click()}
        disabled={!connId}
        title="打开 .sql 文件加载到编辑器"
        >📂 打开 SQL 文件</button
      >
      <button onclick={openDesigner} disabled={!connId} title="新建表（表设计器）">＋ 新建表</button>
      <button onclick={openViewDialog} disabled={!connId} title="新建视图">＋ 新建视图</button>
      <button onclick={loadDbs} disabled={!connId} title="刷新对象树">⟳ 刷新</button>
      <button onclick={openSettings} title="设置">⚙ 设置</button>
      {#if connId}
        <button onclick={doDisconnect} class="danger">断开</button>
      {:else}
        <button onclick={() => (showConnPanel = !showConnPanel)}>连接</button>
      {/if}
    </div>
    <div class="status" class:ok={!!connId} class:err={status.startsWith('连接失败') || status.startsWith('加载')}>
      {status}
      {#if version}
        <span class="ver">{version.slice(0, 40)}</span>
      {/if}
    </div>
  </header>

  <main>
    <!-- ============ 连接管理弹窗（未连接时显示） ============ -->
    {#if showConnPanel && !connId}
      <div class="overlay" role="presentation" onclick={() => (showConnPanel = false)}>
        <div
          class="conn-dialog"
          role="dialog"
          aria-label="连接数据库"
          tabindex="-1"
          onclick={(e) => e.stopPropagation()}
          onkeydown={(e) => e.key === 'Escape' && (showConnPanel = false)}
        >
          <div class="dialog-head">
            <span class="dialog-title">🔌 连接数据库</span>
            <button class="dialog-close" onclick={() => (showConnPanel = false)}>×</button>
          </div>
          <div class="dialog-body">
            {#if savedConns.length > 0}
              <div class="saved-title">已保存的连接</div>
              {#each savedConns as sc}
                <div class="saved-item">
                  <button
                    class="saved-connect"
                    onclick={() => connectSaved(sc.name)}
                    disabled={connecting}
                  >
                    <span class="conn-ico">🔌</span>
                    <span class="conn-main">
                      <span class="conn-name">{sc.name}</span>
                      <span class="conn-sub">
                        <span class="badge">{sc.db_type === 'mysql' ? 'MySQL' : 'PG'}</span>
                        {sc.user}@{sc.host}:{sc.port} · {sc.dbname}
                      </span>
                    </span>
                  </button>
                  <button
                    class="saved-del"
                    onclick={() => deleteSaved(sc.name)}
                    title="删除该连接"
                    >×</button
                  >
                </div>
              {/each}
              <div class="divider"></div>
            {/if}
            <div class="saved-title">新建连接</div>
            <div class="conn-form-v">
              <div class="field">
                <label for="f-dbtype">数据库类型</label>
                <select id="f-dbtype" bind:value={dbType}>
                  <option value="postgres">PostgreSQL 🐘</option>
                  <option value="mysql" disabled>MySQL（即将支持）</option>
                </select>
              </div>
              <div class="field">
                <label for="f-host">连接地址</label>
                <input id="f-host" bind:value={host} placeholder="localhost" />
              </div>
              <div class="field">
                <label for="f-port">端口</label>
                <input id="f-port" bind:value={port} type="number" placeholder="5432" />
              </div>
              <div class="field">
                <label for="f-user">用户名</label>
                <input id="f-user" bind:value={user} placeholder="postgres" />
              </div>
              <div class="field">
                <label for="f-pass">密码</label>
                <input id="f-pass" bind:value={password} type="password" placeholder="留空表示免密" />
              </div>
              <div class="field">
                <label for="f-db">数据库名</label>
                <input id="f-db" bind:value={dbname} placeholder="postgres" />
              </div>
              <div class="field">
                <label for="f-cname">连接名</label>
                <input id="f-cname" bind:value={connName} placeholder="保存后一键连接（可选）" />
              </div>
              <div class="field-actions">
                <label class="save-label">
                  保存此连接
                  <input type="checkbox" bind:checked={saveConn} />
                </label>
                <button onclick={doConnect} disabled={connecting} class="primary">
                  {connecting ? '连接中…' : '连接'}
                </button>
              </div>
            </div>
          </div>
        </div>
      </div>
    {/if}

    <!-- ============ 左侧对象树 ============ -->
    <aside class="sidebar" style={`width:${sidebarWidth}px`}>
      <div class="sidebar-title">连接</div>
      {#if connId}
        <div class="tree">
          {#each dbs as db}
            <div class="tree-node">
              <div
                class="tree-row"
                class:open={treeOpen[db.name]}
                role="button"
                tabindex="0"
                onclick={() => toggleDb(db.name)}
                onkeydown={(e) => e.key === 'Enter' && toggleDb(db.name)}
              >
                <span class="arrow">{treeOpen[db.name] ? '▾' : '▸'}</span>
                <span class="ico">🗄</span>
                <span class="label">{db.name}</span>
                {#if loadingKey === db.name}<span class="spin">…</span>{/if}
              </div>
              {#if treeOpen[db.name] && tables[db.name]}
                <div class="tree-children">
                  {#each tables[db.name] as tb}
                    <div class="tree-node">
                      <div
                        class="tree-row"
                        class:open={treeOpen[`${db.name}.${tb.name}`]}
                        role="button"
                        tabindex="0"
                        onclick={() => openTableTab(db.name, tb.name)}
                        onkeydown={(e) => e.key === 'Enter' && openTableTab(db.name, tb.name)}
                        oncontextmenu={(e) => openTableMenu(e, db.name, tb.name, tb.kind)}
                        title="单击打开表 · 右键更多操作"
                      >
                        <span
                          class="arrow"
                          role="button"
                          tabindex="0"
                          onclick={(e) => {
                            e.stopPropagation();
                            toggleTable(db.name, tb.name);
                          }}
                          onkeydown={(e) => {
                            e.stopPropagation();
                            if (e.key === 'Enter') toggleTable(db.name, tb.name);
                          }}
                          >{treeOpen[`${db.name}.${tb.name}`] ? '▾' : '▸'}</span
                        >
                        <span class="ico">{tb.kind === 'view' ? '👁' : '📋'}</span>
                        <span class="label">{tb.name}</span>
                        {#if loadingKey === `${db.name}.${tb.name}`}<span class="spin">…</span>{/if}
                        {#if tb.kind === 'table'}
                          <button
                            class="tree-del"
                            onclick={(e) => {
                              e.stopPropagation();
                              openDesignerForEdit(db.name, tb.name);
                            }}
                            title="编辑表结构"
                            >✎</button
                          >
                        {/if}
                        <button
                          class="tree-del"
                          onclick={(e) => {
                            e.stopPropagation();
                            if (tb.kind === 'view') {
                              dropViewFromTree(db.name, tb.name);
                            } else {
                              dropTableFromTree(db.name, tb.name);
                            }
                          }}
                          title="删除（不可恢复）"
                          >🗑</button
                        >
                      </div>
                      {#if treeOpen[`${db.name}.${tb.name}`] && columns[`${db.name}.${tb.name}`]}
                        <div class="tree-children">
                          {#each columns[`${db.name}.${tb.name}`] as col}
                            <div class="tree-row leaf">
                              <span class="ico">{col.is_pk ? '🔑' : '▫'}</span>
                              <span class="label">{col.name}</span>
                              <span class="type">{col.type_name}</span>
                            </div>
                          {/each}
                        </div>
                      {/if}
                    </div>
                  {/each}
                </div>
              {/if}
            </div>
          {/each}
        </div>
      {:else}
        <div class="empty-tree">连接后显示数据库对象</div>
      {/if}
    </aside>

    <!-- 侧栏拖拽手柄 -->
    <div class="sidebar-resizer" role="presentation" onmousedown={startSidebarResize}></div>

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
                    onclick={() => openDesignerForEdit(activeTab.dbname!, activeTab.table!)}
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
              {#each dbs as db}
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

  <!-- ============ 设置弹窗 ============ -->
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
        <div class="dialog-body">
          <div class="field">
            <label for="s-ps">数据页每页行数（10-500）</label>
            <input id="s-ps" type="number" min="10" max="500" bind:value={settingsPageSize} />
          </div>
          <div class="field-actions" style="margin-top:18px">
            <button onclick={() => (showSettings = false)}>取消</button>
            <button onclick={saveSettings} class="primary">保存</button>
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
              {#each dbs as db}
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
          openTableTab(tableMenu!.db, tableMenu!.table);
          tableMenu = null;
        }}
        >📖 打开</button
      >
      {#if tableMenu!.kind === 'table'}
        <button
          onclick={() => {
            openDesignerForEdit(tableMenu!.db, tableMenu!.table);
            tableMenu = null;
          }}
          >✎ 编辑表结构</button
        >
        <div class="ctx-sep"></div>
        <button
          onclick={() => {
            dupDialog = {
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
      {/if}
      <div class="ctx-sep"></div>
      <button
        class="ctx-danger"
        onclick={() => {
          if (tableMenu!.kind === 'view') {
            dropViewFromTree(tableMenu!.db, tableMenu!.table);
          } else {
            dropTableFromTree(tableMenu!.db, tableMenu!.table);
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
          <span class="dialog-title">🗑 删除表</span>
          <button class="dialog-close" onclick={() => (confirmDrop = null)}>×</button>
        </div>
        <div class="dialog-body">
          <p class="confirm-text">
            {#if confirmDrop.kind === 'view'}
              确定删除视图「<b>{confirmDrop.table}</b>」？
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
  <footer>
    <span>连接：{connId ? '已连接' : '未连接'}</span>
    <span>· 数据库：{dbname}</span>
    <span>· 对象树：{dbs.length} 库</span>
    <span class="spacer"></span>
    <span>Tusk v1.0.0</span>
  </footer>
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
  header {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 8px 14px;
    background: #1e2128;
    border-bottom: 1px solid #2c303a;
  }

  .logo {
    font-size: 15px;
    font-weight: 700;
    color: #4fc3f7;
    white-space: nowrap;
  }

  .toolbar {
    display: flex;
    gap: 6px;
    flex: 1;
  }

  button {
    background: #2f6fed;
    border: none;
    border-radius: 6px;
    color: #fff;
    padding: 5px 14px;
    font-size: 12px;
    cursor: pointer;
    white-space: nowrap;
  }

  button:hover:not(:disabled) {
    background: #4a83f5;
  }

  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  button.danger {
    background: #d64545;
  }

  button.primary {
    padding: 8px 22px;
    font-size: 13px;
  }

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
    flex-direction: column;
    align-items: center;
    gap: 12px;
    margin-top: 6px;
    padding-top: 12px;
    border-top: 1px solid #2c303a;
  }

  .field-actions button.primary {
    width: 180px;
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

  .save-label input[type='checkbox'] {
    margin: 0;
    width: 14px;
    height: 14px;
    flex-shrink: 0;
    accent-color: #4fc3f7;
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

  /* ===== 左侧对象树 ===== */
  .sidebar {
    background: #191c22;
    border-right: 1px solid #2c303a;
    overflow: auto;
    padding-bottom: 12px;
    flex-shrink: 0;
  }

  .sidebar-resizer {
    width: 5px;
    cursor: col-resize;
    flex-shrink: 0;
    user-select: none;
  }

  .sidebar-resizer:hover {
    background: rgba(79, 195, 247, 0.35);
  }

  .sidebar-title {
    padding: 8px 12px;
    font-size: 11px;
    color: #6b7484;
    letter-spacing: 1px;
    border-bottom: 1px solid #23262e;
    position: sticky;
    top: 0;
    background: #191c22;
  }

  .tree {
    padding: 4px 0;
  }

  .tree-node {
    user-select: none;
  }

  .tree-row {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 3px 12px 3px 8px;
    cursor: pointer;
    white-space: nowrap;
  }

  .tree-row:hover {
    background: #242833;
  }

  .tree-row.leaf {
    cursor: default;
    padding-left: 30px;
  }

  .tree-row .arrow {
    width: 12px;
    color: #5c6472;
    font-size: 10px;
  }

  .tree-row .ico {
    width: 16px;
    text-align: center;
  }

  .tree-row .label {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .tree-row .type {
    color: #5c6472;
    font-size: 10px;
    margin-left: auto;
    padding-right: 4px;
  }

  .tree-children {
    margin-left: 14px;
  }

  .spin {
    color: #4fc3f7;
    margin-left: 4px;
  }

  .empty-tree {
    padding: 20px 12px;
    color: #4c5462;
    font-size: 12px;
  }

  /* ===== 中央工作区 ===== */
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

  .tree-row .tree-del {
    margin-left: auto;
    background: transparent;
    border: none;
    color: #5c6472;
    font-size: 11px;
    cursor: pointer;
    padding: 0 4px;
    opacity: 0;
    transition: opacity 0.12s;
    line-height: 1;
  }

  .tree-row:hover .tree-del {
    opacity: 1;
  }

  .tree-row .tree-del:hover {
    color: #e05656;
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
  footer {
    display: flex;
    gap: 6px;
    padding: 4px 14px;
    background: #1e2128;
    border-top: 1px solid #2c303a;
    color: #6b7484;
    font-size: 11px;
    align-items: center;
  }

  footer .spacer {
    flex: 1;
  }
</style>

<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';

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
  }
  interface SchemaColumn {
    name: string;
    type_name: string;
    is_nullable: string;
    default: string | null;
    is_pk: boolean;
  }
  let dbs = $state<DatabaseInfo[]>([]);
  let treeOpen = $state<Record<string, boolean>>({}); // key: db / db.table
  let tables = $state<Record<string, TableInfo[]>>({});
  let columns = $state<Record<string, SchemaColumn[]>>({});
  let loadingKey = $state('');

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
    colWidths: Record<string, string>;
    // 表标签字段
    dbname?: string;
    table?: string;
    subTab?: 'data' | 'structure' | 'sql';
    page?: number;
    pageSize?: number;
    total?: number;
    loading?: boolean;
    structure?: SchemaColumn[];
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
      pageSize: 50,
      total: 0,
      loading: false,
      structure: undefined,
    };
    tabs.push(t);
    activeTabId = t.id;
    loadTablePage(t);
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
      });
      t.columns = res.columns;
      t.rows = res.rows;
      t.total = res.total ?? 0;
      t.colWidths = {};
    } catch (e) {
      t.error = String(e);
      t.rows = [];
      t.columns = [];
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

  // ================= 查询 =================
  async function runQuery(tab?: QueryTab) {
    const t = tab ?? activeTab;
    if (!t || !connId || !t.sql.trim()) return;
    t.running = true;
    t.error = '';
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
      <button onclick={loadDbs} disabled={!connId} title="刷新对象树">⟳ 刷新</button>
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
                  <input type="checkbox" bind:checked={saveConn} />
                  保存此连接
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
                        title="单击打开表（数据/结构/SQL）"
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
                        <span class="ico">📋</span>
                        <span class="label">{tb.name}</span>
                        {#if loadingKey === `${db.name}.${tb.name}`}<span class="spin">…</span>{/if}
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
                <button onclick={() => runQuery()} disabled={!connId || activeTab.running}>
                  {activeTab.running ? '执行中…' : '▶ 执行'}
                </button>
                <span class="hint">Cmd+Enter 执行 · Cmd+N 新查询</span>
              </div>
            </div>

            <div class="result">
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
                      {#if ri === activeTab.results.length - 1 && activeTab.elapsed !== null}
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
                >{activeTab.dbname}.{activeTab.table} · 每页 {activeTab.pageSize} 行</span
              >
            </div>

            {#if activeTab.error}
              <div class="error">⚠ {activeTab.error}</div>
            {/if}

            {#if activeTab.subTab === 'data'}
              <div class="result">
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
                        {#each activeTab.rows as row}
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

  <!-- ============ 底部状态栏 ============ -->
  <footer>
    <span>连接：{connId ? '已连接' : '未连接'}</span>
    <span>· 数据库：{dbname}</span>
    <span>· 对象树：{dbs.length} 库</span>
    <span class="spacer"></span>
    <span>Tusk v0.2.0 · Tauri 2 + Rust + Svelte 5</span>
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

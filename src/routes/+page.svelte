<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';

  // ================= 连接配置 =================
  let host = $state('localhost');
  let port = $state<number>(5432);
  let user = $state('');
  let password = $state('');
  let dbname = $state('postgres');
  let showConnPanel = $state(true);

  // ================= 连接状态 =================
  let connId = $state('');
  let version = $state('');
  let connecting = $state(false);
  let status = $state('未连接');

  // ================= 对象树 =================
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
  interface QueryTab {
    id: number;
    title: string;
    sql: string;
    columns: { name: string; type_name: string }[];
    rows: unknown[][];
    affected: number | null;
    error: string;
    running: boolean;
    elapsed: number | null;
    colWidths: Record<string, string>;
  }
  let tabs = $state<QueryTab[]>([]);
  let activeTabId = $state(0);
  let tabSeq = $state(1);

  function newTab(sql = ''): QueryTab {
    return {
      id: tabSeq++,
      title: `查询 ${tabSeq - 1}`,
      sql,
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
    } catch (e) {
      status = '连接失败';
      const t = ensureTab();
      t.error = String(e);
    }
    connecting = false;
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

  // 双击表：在查询标签里打开 SELECT
  function openTableSql(db: string, table: string) {
    const t = ensureTab();
    t.title = table;
    t.sql = `SELECT * FROM "${db}"."${table}"\nLIMIT 100;`;
    t.columns = [];
    t.rows = [];
    t.error = '';
    runQuery(t);
  }

  // ================= 查询 =================
  async function runQuery(tab?: QueryTab) {
    const t = tab ?? activeTab;
    if (!t || !connId || !t.sql.trim()) return;
    t.running = true;
    t.error = '';
    const t0 = performance.now();
    try {
      const res = await invoke<{
        columns: { name: string; type_name: string }[];
        rows: unknown[][];
        rows_affected: number | null;
      }>('query', { connId, sql: t.sql });
      t.columns = res.columns;
      t.rows = res.rows;
      t.affected = res.rows_affected;
      t.colWidths = {}; // 新结果集重置列宽
    } catch (e) {
      t.error = String(e);
      t.columns = [];
      t.rows = [];
      t.affected = null;
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
    <!-- ============ 连接面板（未连接时显示） ============ -->
    {#if showConnPanel && !connId}
      <div class="conn-panel">
        <div class="conn-form">
          <input bind:value={host} placeholder="host" />
          <input bind:value={port} type="number" placeholder="port" class="narrow" />
          <input bind:value={user} placeholder="user" />
          <input bind:value={password} type="password" placeholder="password" />
          <input bind:value={dbname} placeholder="database" />
          <button onclick={doConnect} disabled={connecting}>
            {connecting ? '连接中…' : '连接'}
          </button>
        </div>
      </div>
    {/if}

    <!-- ============ 左侧对象树 ============ -->
    <aside class="sidebar">
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
                        onclick={() => toggleTable(db.name, tb.name)}
                        onkeydown={(e) => e.key === 'Enter' && toggleTable(db.name, tb.name)}
                        ondblclick={() => openTableSql(db.name, tb.name)}
                        title="双击打开表数据"
                      >
                        <span class="arrow">{treeOpen[`${db.name}.${tb.name}`] ? '▾' : '▸'}</span>
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
            {#if activeTab.error}
              <div class="error">⚠ {activeTab.error}</div>
            {/if}
            {#if activeTab.affected !== null}
              <div class="ok">✓ 影响行数：{activeTab.affected}</div>
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
                    {#each activeTab.rows as row, ri}
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
                共 {activeTab.rows.length} 行
                {#if activeTab.elapsed !== null} · 耗时 {activeTab.elapsed.toFixed(0)} ms{/if}
              </div>
            {:else if !activeTab.error && activeTab.affected === null && !activeTab.running}
              <div class="empty">连接后输入 SQL，点执行查看结果；双击左侧表直接浏览数据</div>
            {/if}
          </div>
        </div>
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

  /* 连接面板 */
  .conn-panel {
    position: absolute;
    top: 44px;
    left: 0;
    right: 0;
    z-index: 10;
    background: #1b1e25;
    border-bottom: 1px solid #2c303a;
    padding: 10px 14px;
  }

  .conn-form {
    display: flex;
    gap: 6px;
    max-width: 720px;
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

  input.narrow {
    flex: 0 0 70px;
  }

  input:focus {
    outline: none;
    border-color: #4fc3f7;
  }

  /* ===== 左侧对象树 ===== */
  .sidebar {
    width: 260px;
    min-width: 260px;
    background: #191c22;
    border-right: 1px solid #2c303a;
    overflow: auto;
    padding-bottom: 12px;
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
    overflow: hidden;
    display: flex;
    flex-direction: column;
    min-height: 0;
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
    flex: 1;
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
    white-space: nowrap;
    z-index: 1;
    position: relative;
  }

  .resizer {
    position: absolute;
    top: 0;
    right: 0;
    width: 5px;
    height: 100%;
    cursor: col-resize;
    user-select: none;
  }

  .resizer:hover {
    background: rgba(79, 195, 247, 0.35);
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

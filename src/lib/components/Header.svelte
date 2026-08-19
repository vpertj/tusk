<script lang="ts">
  let {
    connId,
    openNewTab,
    onSqlFilePicked,
    openDesigner,
    openViewDialog,
    openSyncDialog,
    loadDbs,
    openSearch,
    toggleConnPanel,
    doDisconnect
  } = $props();
</script>

  <header>
    <div class="toolbar">
      <div class="grp">
        <button onclick={openNewTab} disabled={!connId} data-tip="新建查询 (⌘N)" aria-label="新建查询">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
            <polyline points="14 2 14 8 20 8" />
            <line x1="12" y1="18" x2="12" y2="12" />
            <line x1="9" y1="15" x2="15" y2="15" />
          </svg>
        </button>
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
          data-tip="打开 .sql 文件" aria-label="打开 SQL 文件"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
          </svg>
        </button>
      </div>
      <div class="sep"></div>
      <div class="grp">
        <button onclick={openDesigner} disabled={!connId} data-tip="新建表" aria-label="新建表">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <rect x="3" y="3" width="7" height="7" />
            <rect x="14" y="3" width="7" height="7" />
            <rect x="14" y="14" width="7" height="7" />
            <rect x="3" y="14" width="7" height="7" />
          </svg>
        </button>
        <button onclick={openViewDialog} disabled={!connId} data-tip="新建视图" aria-label="新建视图">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z" />
            <circle cx="12" cy="12" r="3" />
          </svg>
        </button>
        <button onclick={openSyncDialog} disabled={!connId} data-tip="结构同步" aria-label="结构同步">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="23 4 23 10 17 10" />
            <polyline points="1 20 1 14 7 14" />
            <path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15" />
          </svg>
        </button>
      </div>
      <div class="sep"></div>
      <div class="grp">
        <button onclick={loadDbs} disabled={!connId} data-tip="刷新对象树" aria-label="刷新对象树">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="23 4 23 10 17 10" />
            <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10" />
          </svg>
        </button>
        <button onclick={openSearch} disabled={!connId} data-tip="搜索对象 (⌘F)" aria-label="搜索对象">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="11" cy="11" r="8" />
            <line x1="21" y1="21" x2="16.65" y2="16.65" />
          </svg>
        </button>
      </div>
      <div class="spacer"></div>
      <div class="grp">
        {#if connId}
          <button onclick={doDisconnect} class="danger" title="断开连接">断开</button>
        {:else}
          <button onclick={toggleConnPanel} class="primary" title="连接数据库">连接</button>
        {/if}
      </div>
    </div>
  </header>

<style>
  header {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 8px 14px;
    background: #1e2128;
    border-bottom: 1px solid #2c303a;
    position: relative;
    z-index: 100;
  }

  .toolbar {
    display: flex;
    align-items: center;
    flex: 1;
    min-width: 0;
  }

  .toolbar .grp {
    display: flex;
    align-items: center;
    gap: 2px;
  }

  .toolbar .sep {
    width: 1px;
    height: 18px;
    background: #2c303a;
    margin: 0 8px;
    flex-shrink: 0;
  }

  .toolbar .spacer {
    flex: 1;
  }

  /* 全局按钮：默认幽灵样式，主操作 .primary */
  button {
    background: transparent;
    border: 1px solid transparent;
    border-radius: 6px;
    color: #c3c9d4;
    padding: 5px 12px;
    font-size: 12px;
    cursor: pointer;
    white-space: nowrap;
    transition: background 0.12s ease, color 0.12s ease;
  }

  button:hover:not(:disabled) {
    background: #262b36;
    color: #f0f2f5;
  }

  button:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  button.primary {
    background: #2f6fed;
    border-color: #2f6fed;
    color: #fff;
    padding: 5px 16px;
    font-size: 12px;
  }

  button.primary:hover:not(:disabled) {
    background: #4a83f5;
    border-color: #4a83f5;
    color: #fff;
  }

  button.danger {
    background: #d64545;
    border-color: #d64545;
    color: #ffffff;
    padding: 5px 12px;
    font-size: 12px;
    font-weight: 600;
  }

  button.danger:hover:not(:disabled) {
    background: #e05656;
    border-color: #e05656;
    color: #ffffff;
  }

  /* 工具栏图标按钮 */
  .toolbar button {
    width: 30px;
    height: 30px;
    padding: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 6px;
    color: #9aa3b2;
  }

  .toolbar button:hover:not(:disabled) {
    background: #262b36;
    color: #f0f2f5;
  }

  .toolbar button.primary,
  .toolbar button.danger {
    width: auto;
    padding: 4px 11px;
    display: inline-flex;
    align-items: center;
    font-size: 11px;
  }

  .toolbar button svg {
    width: 16px;
    height: 16px;
  }

  /* 工具栏按钮自定义 tooltip（WKWebView 不显示 title） */
  .toolbar button[data-tip] {
    position: relative;
  }

  .toolbar button[data-tip]::after {
    content: attr(data-tip);
    position: absolute;
    top: calc(100% + 14px);
    left: 50%;
    transform: translateX(-50%);
    background: #262b36;
    border: 1px solid #3a4150;
    color: #e8ebf0;
    font-size: 11px;
    padding: 4px 10px;
    border-radius: 6px;
    white-space: nowrap;
    opacity: 0;
    pointer-events: none;
    transition: opacity 0.12s ease;
    z-index: 999;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  }

  .toolbar button[data-tip]:hover::after {
    opacity: 1;
  }

</style>

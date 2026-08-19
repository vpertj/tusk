<script lang="ts">
  let {
    sidebarWidth,
    connNodes,
    connDbs,
    tables,
    treeOpen,
    columns,
    loadingKey,
    connId,
    openBlankMenu,
    openConnPanel,
    connRowClick,
    openConnMenu,
    openDbDialog,
    toggleDb,
    openDbMenu,
    toggleTable,
    openTableTab,
    openTableMenu,
    openDesignerForEdit,
    dropViewFromTree,
    dropTableFromTree,
    ck,
    startSidebarResize
  } = $props();
</script>

    <aside
      class="sidebar"
      style={`width:${sidebarWidth}px`}
      oncontextmenu={openBlankMenu}
    >
      <div class="sidebar-title">
        连接
        <button
          class="db-add-btn"
          onclick={openConnPanel}
          data-tip="新建连接"
          aria-label="新建连接"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <line x1="12" y1="5" x2="12" y2="19" />
            <line x1="5" y1="12" x2="19" y2="12" />
          </svg>
        </button>
      </div>
      {#if connNodes.length > 0}
        <div class="tree">
          {#each connNodes as c}
            <div class="tree-node">
              <div
                class="tree-row conn-row"
                class:active={c.id === connId}
                role="button"
                tabindex="0"
                onclick={() => connRowClick(c)}
                oncontextmenu={(e) => openConnMenu(e, c)}
                onkeydown={(e) => e.key === 'Enter' && connRowClick(c)}
              >
                <span class="arrow">{c.connected && c.expanded ? '▾' : '▸'}</span>
                <span class="ico conn-dot" class:ok={c.connected}></span>
                <span class="label">{c.name}</span>
                {#if c.connected}<span class="conn-host">{c.host}:{c.port}</span>{/if}
                {#if c.connected && c.id === connId}
                  <button
                    class="db-add-btn"
                    onclick={(e) => {
                      e.stopPropagation();
                      openDbDialog();
                    }}
                    data-tip="新建数据库"
                    aria-label="新建数据库"
                  >
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                      <line x1="12" y1="5" x2="12" y2="19" />
                      <line x1="5" y1="12" x2="19" y2="12" />
                    </svg>
                  </button>
                {/if}
              </div>
              {#if c.connected && c.expanded}
                <div class="tree-children">
                  {#each connDbs[c.id] ?? [] as db}
                    <div class="tree-node">
                      <div
                        class="tree-row"
                        class:open={treeOpen[ck(c.id, db.name)]}
                        role="button"
                        tabindex="0"
                        onclick={() => toggleDb(c.id, db.name)}
                        oncontextmenu={(e) => openDbMenu(e, c.id, db.name)}
                        onkeydown={(e) => e.key === 'Enter' && toggleDb(c.id, db.name)}
                      >
                        <span class="arrow">{treeOpen[ck(c.id, db.name)] ? '▾' : '▸'}</span>
                        <span class="ico tree-db-ico">
                          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                            <ellipse cx="12" cy="5" rx="8" ry="3" />
                            <path d="M4 5v6c0 1.66 3.58 3 8 3s8-1.34 8-3V5" />
                            <path d="M4 11v6c0 1.66 3.58 3 8 3s8-1.34 8-3v-6" />
                          </svg>
                        </span>
                        <span class="label">{db.name}</span>
                        {#if loadingKey === ck(c.id, db.name)}<span class="spin">…</span>{/if}
                      </div>
                      {#if treeOpen[ck(c.id, db.name)] && tables[ck(c.id, db.name)]}
                        <div class="tree-children">
                          {#each tables[ck(c.id, db.name)] as tb}
                            <div class="tree-node">
                              <div
                                class="tree-row"
                                class:open={treeOpen[ck(c.id, `${db.name}.${tb.name}`)]}
                                role="button"
                                tabindex="0"
                                onclick={() => openTableTab(c.id, db.name, tb.name)}
                                onkeydown={(e) => e.key === 'Enter' && openTableTab(c.id, db.name, tb.name)}
                                oncontextmenu={(e) => openTableMenu(e, c.id, db.name, tb.name, tb.kind)}
                              >
                                <span
                                  class="arrow"
                                  role="button"
                                  tabindex="0"
                                  onclick={(e) => {
                                    e.stopPropagation();
                                    toggleTable(c.id, db.name, tb.name);
                                  }}
                                  onkeydown={(e) => {
                                    e.stopPropagation();
                                    if (e.key === 'Enter') toggleTable(c.id, db.name, tb.name);
                                  }}
                                  >{treeOpen[ck(c.id, `${db.name}.${tb.name}`)] ? '▾' : '▸'}</span
                                >
                                <span class="ico tree-tbl-ico">
                                {#if tb.kind === 'view'}
                                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                    <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z" />
                                    <circle cx="12" cy="12" r="3" />
                                  </svg>
                                {:else}
                                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                    <rect x="3" y="3" width="18" height="18" rx="2" />
                                    <line x1="3" y1="9" x2="21" y2="9" />
                                    <line x1="3" y1="15" x2="21" y2="15" />
                                    <line x1="9" y1="3" x2="9" y2="21" />
                                    <line x1="15" y1="3" x2="15" y2="21" />
                                  </svg>
                                {/if}
                              </span>
                                <span class="label">{tb.name}</span>
                                {#if loadingKey === ck(c.id, `${db.name}.${tb.name}`)}<span class="spin">…</span>{/if}
                                {#if tb.kind === 'table'}
                                  <button
                                    class="tree-del"
                                    onclick={(e) => {
                                      e.stopPropagation();
                                      openDesignerForEdit(c.id, db.name, tb.name);
                                    }}
                                    data-tip="编辑表结构"
                                    aria-label="编辑表结构"
                                    >✎</button
                                  >
                                {/if}
                                <button
                                  class="tree-del"
                                  onclick={(e) => {
                                    e.stopPropagation();
                                    if (tb.kind === 'view') {
                                      dropViewFromTree(c.id, db.name, tb.name);
                                    } else {
                                      dropTableFromTree(c.id, db.name, tb.name);
                                    }
                                  }}
                                  data-tip="删除（不可恢复）"
                                  aria-label="删除"
                                  >🗑</button
                                >
                              </div>
                              {#if treeOpen[ck(c.id, `${db.name}.${tb.name}`)] && columns[ck(c.id, `${db.name}.${tb.name}`)]}
                                <div class="tree-children">
                                  {#each columns[ck(c.id, `${db.name}.${tb.name}`)] as col}
                                    <div class="tree-row leaf">
                                      <span class="ico tree-col-ico">
                                {#if col.is_pk}
                                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                    <circle cx="7.5" cy="15.5" r="4.5" />
                                    <path d="M10.7 12.3 21 2" />
                                    <line x1="17" y1="6" x2="20" y2="9" />
                                  </svg>
                                {:else}
                                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
                                    <circle cx="12" cy="12" r="2" />
                                  </svg>
                                {/if}
                              </span>
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
              {/if}
            </div>
          {/each}
        </div>
      {:else}
        <div class="empty-tree">在「连接管理」中保存连接，双击连接展开数据库</div>
      {/if}
    </aside>

    <!-- 侧栏拖拽手柄 -->
    <div class="sidebar-resizer" role="presentation" onmousedown={startSidebarResize}></div>

<style>
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
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 12px;
    font-size: 11px;
    color: #6b7484;
    letter-spacing: 1px;
    border-bottom: 1px solid #23262e;
    position: sticky;
    top: 0;
    background: #191c22;
  }

  .db-add-btn {
    width: 22px;
    height: 22px;
    padding: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 5px;
    color: #8b93a3;
    background: transparent;
    border: none;
    cursor: pointer;
  }

  .db-add-btn:hover {
    background: #262b36;
    color: #e8ebf0;
  }

  .db-add-btn[data-tip] {
    position: relative;
  }

  .db-add-btn[data-tip]::after {
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

  .db-add-btn[data-tip]:hover::after {
    opacity: 1;
  }

  .db-add-btn svg {
    width: 13px;
    height: 13px;
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

  /* 连接树节点（Navicat 式） */
  .conn-row {
    font-weight: 600;
    color: #e8ebf0;
    padding: 5px 12px 5px 8px;
    border-left: 2px solid transparent;
  }

  .conn-row.active {
    border-left-color: #2f6fed;
    background: #1d2a44;
  }


  .tree-row .ico svg {
    width: 13px;
    height: 13px;
    flex-shrink: 0;
  }

  .tree-row .ico {
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }

  .conn-dot::before {
    content: '';
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: #4c5462;
    display: inline-block;
  }

  .conn-dot.ok::before {
    background: #4cd67d;
    box-shadow: 0 0 4px rgba(76, 214, 125, 0.5);
  }
  .conn-dot {
    font-size: 9px;
    color: #5a6270;
  }

  .conn-dot.ok {
    color: #3fbf6a;
  }

  .conn-host {
    font-size: 10px;
    font-weight: 400;
    color: #6b7484;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 110px;
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
</style>

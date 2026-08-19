<script lang="ts">
  let {
    showConnPanel,
    savedConns,
    connecting,
    dbType,
    sqlitePath,
    host,
    port,
    user,
    password,
    dbname,
    sshEnabled,
    sshHost,
    sshPort,
    sshUser,
    sshPass,
    connName,
    saveConn,
    pgHelp,
    PG_INSTALL_CMD,
    PG_START_CMD,
    close,
    connectSaved,
    deleteSaved,
    doConnect,
    copyCmd
  } = $props();
</script>

    <!-- ============ 连接管理弹窗 ============ -->
    {#if showConnPanel}
      <div class="overlay" role="presentation" onclick={close}>
        <div
          class="conn-dialog"
          role="dialog"
          aria-label="连接数据库"
          tabindex="-1"
          onclick={(e) => e.stopPropagation()}
          onkeydown={(e) => e.key === 'Escape' && close()}
        >
          <div class="dialog-head">
            <span class="dialog-title">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" class="title-ico">
                <ellipse cx="12" cy="5" rx="8" ry="3" />
                <path d="M4 5v6c0 1.66 3.58 3 8 3s8-1.34 8-3V5" />
                <path d="M4 11v6c0 1.66 3.58 3 8 3s8-1.34 8-3v-6" />
              </svg>
              连接数据库
            </span>
            <button class="dialog-close" onclick={close}>×</button>
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
                    <span class="conn-ico">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                      <ellipse cx="12" cy="5" rx="8" ry="3" />
                      <path d="M4 5v6c0 1.66 3.58 3 8 3s8-1.34 8-3V5" />
                      <path d="M4 11v6c0 1.66 3.58 3 8 3s8-1.34 8-3v-6" />
                    </svg>
                  </span>
                    <span class="conn-main">
                      <span class="conn-name">{sc.name}</span>
                      <span class="conn-sub">
                        <span class="badge">{sc.db_type === 'mysql' ? 'MySQL' : sc.db_type === 'sqlite' ? 'SQLite' : 'PG'}</span>
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
                  <option value="sqlite">SQLite 📄</option>
                  <option value="mysql" disabled>MySQL（即将支持）</option>
                </select>
              </div>
              {#if dbType === 'postgres'}
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
                <div class="ssh-block">
                  <label class="save-label">
                    通过 SSH 隧道连接
                    <input type="checkbox" bind:checked={sshEnabled} />
                  </label>
                  {#if sshEnabled}
                    <div class="field">
                      <label for="f-ssh-host">SSH 主机</label>
                      <input id="f-ssh-host" bind:value={sshHost} placeholder="跳板机 IP / 域名" />
                    </div>
                    <div class="field">
                      <label for="f-ssh-port">SSH 端口</label>
                      <input id="f-ssh-port" bind:value={sshPort} type="number" placeholder="22" />
                    </div>
                    <div class="field">
                      <label for="f-ssh-user">SSH 用户</label>
                      <input id="f-ssh-user" bind:value={sshUser} placeholder="root" />
                    </div>
                    <div class="field">
                      <label for="f-ssh-pass">SSH 密码</label>
                      <input
                        id="f-ssh-pass"
                        bind:value={sshPass}
                        type="password"
                        placeholder="留空则尝试 ~/.ssh 私钥"
                      />
                    </div>
                    <p class="ssh-hint">连接地址/端口为内网 PG 目标，经跳板机隧道转发</p>
                  {/if}
                </div>
              {:else if dbType === 'sqlite'}
                <div class="field">
                  <label for="f-path">数据库文件</label>
                  <input id="f-path" bind:value={sqlitePath} placeholder="/path/to/your.db（不存在则自动创建）" />
                </div>
              {/if}
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
              {#if pgHelp}
                <div class="pg-help">
                  <div class="pg-help-title">🐘 未检测到本机 PostgreSQL 服务</div>
                  <p class="pg-help-desc">
                    看起来本机 PostgreSQL 未安装或未启动。在「终端」中执行以下命令（按顺序），然后重新连接：
                  </p>
                  <div class="pg-help-cmd">
                    <code>{PG_INSTALL_CMD}</code>
                    <button onclick={() => copyCmd(PG_INSTALL_CMD)} title="复制命令">⧉ 复制</button>
                  </div>
                  <div class="pg-help-cmd">
                    <code>{PG_START_CMD}</code>
                    <button onclick={() => copyCmd(PG_START_CMD)} title="复制命令">⧉ 复制</button>
                  </div>
                  <p class="pg-help-note">已安装但没启动？执行第二条命令即可。Windows 用户请安装官方 PostgreSQL 安装包。</p>
                </div>
              {/if}
            </div>
          </div>
        </div>
      </div>
    {/if}

<style>
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

  .field + .ssh-block {
    margin-top: 6px;
  }

  .ssh-block {
    padding: 10px 12px;
    border: 1px dashed #2c303a;
    border-radius: 8px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .ssh-block .field {
    margin: 0;
  }

  .ssh-hint {
    font-size: 11px;
    color: #6b7484;
    margin: 0;
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


  .dialog-title .title-ico {
    width: 15px;
    height: 15px;
    vertical-align: -2px;
    margin-right: 4px;
    color: #4a83f5;
  }

  .conn-ico {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: #4a83f5;
  }

  .conn-ico svg {
    width: 16px;
    height: 16px;
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

  .pg-help-cmd code {
    flex: 1;
    background: #14171d;
    border: 1px solid #2c303a;
    border-radius: 6px;
    padding: 6px 10px;
    color: #8fc7f0;
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .pg-help-cmd button {
    background: #262a33;
    border: 1px solid #363b47;
    color: #d7dae0;
    font-size: 11px;
    padding: 4px 10px;
    border-radius: 6px;
    cursor: pointer;
    flex-shrink: 0;
  }

  .pg-help-note {
    color: #5c6472;
    font-size: 11px;
    margin: 4px 0 0;
  }
</style>

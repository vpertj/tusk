<script lang="ts">
  let { connId, connMeta, appVersion, openSettings } = $props();
</script>

  <footer>
    <button class="footer-settings" onclick={openSettings} title="设置" aria-label="设置">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <circle cx="12" cy="12" r="3" />
        <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
      </svg>
    </button>
    {#if connId}
      <span class="conn-info">
        <span class="conn-ok">已连接</span>
        · {connMeta.user}@{connMeta.host}:{connMeta.port} {connMeta.version.replace(/ on .*$/, '')}
      </span>
    {:else}
      <span>未连接</span>
    {/if}
    <span class="spacer"></span>
    <img src="/tusk-icon.png" class="footer-logo" alt="Tusk" />
    <span class="footer-app-name">Tusk</span>
    <span class="footer-ver">v{appVersion}</span>
  </footer>

<style>
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

  .footer-logo {
    width: 14px;
    height: 14px;
    border-radius: 3px;
    flex-shrink: 0;
    margin-left: 4px;
  }

  .footer-app-name {
    font-size: 11px;
    font-weight: 600;
    color: #aab2c0;
  }

  .footer-ver {
    font-size: 11px;
    color: #6b7484;
    margin-left: 2px;
  }
  .conn-info {
    color: #3fbf6a;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .conn-ok {
    color: #4cd67d;
    font-weight: 600;
  }

  /* 底部设置按钮（蓝色图标） */
  .footer-settings {
    width: 22px;
    height: 22px;
    padding: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 5px;
    border: none;
    background: #2f6fed;
    color: #fff;
    cursor: pointer;
    margin-right: 2px;
    flex-shrink: 0;
  }

  .footer-settings:hover {
    background: #4a83f5;
  }

  .footer-settings svg {
    width: 12px;
    height: 12px;
  }

</style>

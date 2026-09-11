<script lang="ts">
  import { tick } from 'svelte';
  import { complete, type CompletionItem, type CompletionKind, type CompletionSchema } from '$lib/sql-complete';
  import type { Dialect } from '$lib/sql-dialect';

  let {
    value = $bindable(''),
    schema = { tables: [], columns: {} },
    dialect = 'postgresql',
    onkeydown,
  }: {
    value?: string;
    schema?: CompletionSchema;
    dialect?: Dialect;
    onkeydown?: (e: KeyboardEvent) => void;
  } = $props();

  const KIND_LABEL: Record<CompletionKind, string> = {
    table: '表',
    column: '字段',
    function: '函数',
    keyword: '关键字',
  };

  let el: HTMLTextAreaElement | undefined = $state();
  let mirror: HTMLDivElement | undefined = $state();
  let acEl: HTMLDivElement | undefined = $state();
  let composing = $state(false);
  let navigating = $state(false);
  let result = $state<{ items: CompletionItem[]; replaceFrom: number; replaceTo: number } | null>(null);
  let sel = $state(0);
  let pos = $state({ left: 0, top: 0 });

  const open = $derived(!!result && result.items.length > 0);

  /** 把选中的候选滚进可视区 */
  $effect(() => {
    if (!open || !acEl) return;
    const node = acEl.children[sel] as HTMLElement | undefined;
    node?.scrollIntoView({ block: 'nearest' });
  });

  function close() {
    result = null;
  }

  function recompute() {
    if (!el || composing || el.selectionStart !== el.selectionEnd) {
      close();
      return;
    }
    try {
      const r = complete({ text: el.value, caret: el.selectionStart ?? 0, dialect, schema });
      result = r;
      sel = 0;
      if (r) measure(r.replaceFrom);
    } catch {
      // 补全永不阻塞输入
      close();
    }
  }

  /** 用隐藏镜像量出光标坐标，浮层放在光标下方（不够高就翻到上方） */
  function measure(caret: number) {
    if (!el || !mirror) return;
    mirror.style.width = `${el.offsetWidth}px`;
    mirror.textContent = el.value.slice(0, caret);
    const mark = document.createElement('span');
    mark.textContent = el.value.slice(caret) || '.';
    mirror.appendChild(mark);
    const cs = getComputedStyle(el);
    const lineH = parseFloat(cs.lineHeight) || parseFloat(cs.fontSize) * 1.5;
    const left = Math.max(0, mark.offsetLeft - el.scrollLeft);
    const caretTop = mark.offsetTop - el.scrollTop;
    const popupH = Math.min((result?.items.length ?? 0) * 22 + 8, 200);
    let top = caretTop + lineH;
    if (top + popupH > el.clientHeight) top = Math.max(0, caretTop - popupH);
    pos = { left: Math.min(left, Math.max(0, el.clientWidth - 240)), top };
  }

  async function accept(item: CompletionItem) {
    const r = result;
    if (!r) return;
    const ins = item.insertText ?? item.label;
    const caret = r.replaceFrom + ins.length;
    value = value.slice(0, r.replaceFrom) + ins + value.slice(r.replaceTo);
    close();
    await tick();
    if (el) {
      el.focus();
      el.setSelectionRange(caret, caret);
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (open) {
      const items = result!.items;
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        navigating = true;
        sel = (sel + 1) % items.length;
        return onkeydown?.(e);
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        navigating = true;
        sel = (sel - 1 + items.length) % items.length;
        return onkeydown?.(e);
      }
      if (e.key === 'Tab') {
        e.preventDefault();
        void accept(items[sel]);
        return onkeydown?.(e);
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        close();
        return onkeydown?.(e);
      }
      if (e.key === 'Enter') {
        // 只关浮层，不拦截：Enter 照常换行
        close();
      }
    }
    onkeydown?.(e);
  }

  function handleKeyup(e: KeyboardEvent) {
    if (navigating) {
      navigating = false;
      return;
    }
    if (e.key === 'Escape') return;
    recompute();
  }
</script>

<div class="sql-editor">
  <textarea
    bind:this={el}
    bind:value
    placeholder="输入 SQL…（Cmd/Ctrl + Enter 执行）"
    spellcheck="false"
    autocomplete="off"
    autocapitalize="off"
    onkeydown={handleKeydown}
    onkeyup={handleKeyup}
    oninput={recompute}
    onclick={recompute}
    onscroll={() => {
      if (result && el) measure(el.selectionStart ?? 0);
    }}
    onblur={close}
    oncompositionstart={() => {
      composing = true;
      close();
    }}
    oncompositionend={() => {
      composing = false;
      recompute();
    }}
  ></textarea>

  {#if open}
    <div class="ac" bind:this={acEl} style={`left:${pos.left}px; top:${pos.top}px`}>
      {#each result!.items as it, i (it.kind + ':' + it.label)}
        <div
          class="ac-item"
          class:sel={i === sel}
          role="presentation"
          onmousedown={(e) => {
            e.preventDefault();
            void accept(it);
          }}
          onmouseenter={() => (sel = i)}
        >
          <span class="kind k-{it.kind}">{KIND_LABEL[it.kind]}</span>
          <span class="ac-label">{it.label}</span>
          {#if it.detail}<span class="ac-detail">{it.detail}</span>{/if}
        </div>
      {/each}
    </div>
  {/if}

  <div class="mirror" bind:this={mirror} aria-hidden="true"></div>
</div>

<style>
  .sql-editor {
    position: relative;
  }

  textarea {
    display: block;
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

  /* 与 textarea 同字体度量，仅用于量光标坐标 */
  .mirror {
    position: absolute;
    top: 0;
    left: 0;
    visibility: hidden;
    pointer-events: none;
    white-space: pre-wrap;
    overflow-wrap: break-word;
    overflow: hidden;
    font-family: 'SF Mono', Menlo, Consolas, monospace;
    font-size: 13px;
    line-height: 1.5;
    padding: 10px 12px;
    border: 1px solid transparent;
    box-sizing: border-box;
    tab-size: 4;
  }

  .ac {
    position: absolute;
    z-index: 60;
    min-width: 220px;
    max-width: 340px;
    max-height: 200px;
    overflow-y: auto;
    background: #1b1e25;
    border: 1px solid #2c303a;
    border-radius: 7px;
    box-shadow: 0 8px 22px rgba(0, 0, 0, 0.5);
    padding: 4px 0;
  }

  .ac-item {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 3px 8px;
    cursor: pointer;
    font-family: 'SF Mono', Menlo, Consolas, monospace;
    font-size: 12px;
    color: #c3cad6;
  }

  .ac-item.sel {
    background: #1d2a44;
    color: #e8ebf0;
  }

  .kind {
    flex-shrink: 0;
    min-width: 32px;
    text-align: center;
    font-size: 10px;
    padding: 1px 3px;
    border-radius: 4px;
    background: #232833;
  }

  .k-table {
    color: #4fc3f7;
  }

  .k-column {
    color: #9ccc65;
  }

  .k-function {
    color: #ffb74d;
  }

  .k-keyword {
    color: #8b93a3;
  }

  .ac-label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .ac-detail {
    margin-left: auto;
    color: #5c6472;
    font-size: 10px;
    flex-shrink: 0;
  }
</style>

import { keywords, functions, type Dialect } from './sql-dialect';

export type CompletionKind = 'keyword' | 'function' | 'table' | 'column';

export type CompletionItem = {
  /** 浮层里显示的文字 */
  label: string;
  kind: CompletionKind;
  detail?: string;
  /** 实际插入的文字，缺省等于 label（带引号的前缀会补回引号） */
  insertText?: string;
};

export type CompletionSchema = {
  tables: string[];
  columns: Record<string, { name: string; type_name?: string }[]>;
};

export type CompletionResult = {
  items: CompletionItem[];
  replaceFrom: number;
  replaceTo: number;
};

export type CompleteInput = {
  text: string;
  caret: number;
  dialect: Dialect;
  schema: CompletionSchema;
};

const MAX_ITEMS = 50;
const IDENT = /[A-Za-z0-9_$]/;

/** 表上下文：这些关键字之后提示表名 */
const TABLE_CTX = new Set(['FROM', 'JOIN', 'UPDATE', 'INTO', 'TABLE']);
/** 字段上下文：这些关键字之后提示字段名 */
const COLUMN_CTX = new Set([
  'SELECT', 'WHERE', 'AND', 'OR', 'ON', 'GROUP', 'ORDER', 'HAVING', 'SET', 'BY', 'USING', 'RETURNING',
]);

type ScanState = 'code' | 'string' | 'line' | 'block' | 'quoted';

/** 扫描器推进一步：返回 [新状态, 新下标, 是否在 code 状态遇到语句分隔符] */
function step(state: ScanState, text: string, i: number): [ScanState, number, boolean] {
  const c = text[i];
  const n = text[i + 1];
  if (state === 'code') {
    if (c === "'") return ['string', i + 1, false];
    if (c === '"') return ['quoted', i + 1, false];
    if (c === '-' && n === '-') return ['line', i + 2, false];
    if (c === '/' && n === '*') return ['block', i + 2, false];
    return ['code', i + 1, c === ';'];
  }
  if (state === 'string') {
    if (c === "'" && n === "'") return ['string', i + 2, false];
    if (c === "'") return ['code', i + 1, false];
    return ['string', i + 1, false];
  }
  if (state === 'quoted') {
    if (c === '"' && n === '"') return ['quoted', i + 2, false];
    if (c === '"') return ['code', i + 1, false];
    return ['quoted', i + 1, false];
  }
  if (state === 'line') {
    if (c === '\n') return ['code', i + 1, false];
    return ['line', i + 1, false];
  }
  if (c === '*' && n === '/') return ['block', i + 2, false];
  return ['block', i + 1, false];
}

/** 从文本开头扫到 caret，得到光标处的状态、当前语句起点、未闭合引号标识符的起始位置 */
function scanBack(text: string, caret: number) {
  let state: ScanState = 'code';
  let stmtStart = 0;
  let quotedStart = -1;
  let i = 0;
  while (i < caret) {
    const prev = state;
    const [next, ni, semi] = step(state, text, i);
    if (prev === 'code' && next === 'quoted') quotedStart = i;
    if (prev === 'quoted' && next === 'code') quotedStart = -1;
    if (semi) stmtStart = i + 1;
    state = next;
    i = ni;
  }
  return { state, stmtStart, quotedStart };
}

/** 从 caret 向后扫到下一条语句的分号（跳过字符串与注释），返回语句结束下标 */
function scanForward(text: string, caret: number, initial: ScanState): number {
  let state = initial;
  let i = caret;
  while (i < text.length) {
    const [next, ni, semi] = step(state, text, i);
    if (semi) return i;
    state = next;
    i = ni;
  }
  return text.length;
}

/** 词元：标识符会被解出引号；标点作为 sep 标记保留（用于识别逗号分隔的表列表） */
type Token = { word: string; quoted: boolean; sep: boolean };

function tokenize(src: string): Token[] {
  const out: Token[] = [];
  let buf = '';
  let i = 0;
  const flush = () => {
    if (buf) {
      out.push({ word: buf, quoted: false, sep: false });
      buf = '';
    }
  };
  while (i < src.length) {
    const c = src[i];
    if (IDENT.test(c)) {
      buf += c;
      i++;
      continue;
    }
    if (c === '"') {
      flush();
      let w = '';
      i++;
      while (i < src.length) {
        if (src[i] === '"' && src[i + 1] === '"') {
          w += '"';
          i += 2;
          continue;
        }
        if (src[i] === '"') {
          i++;
          break;
        }
        w += src[i++];
      }
      out.push({ word: w, quoted: true, sep: false });
      continue;
    }
    if (c === "'") {
      flush();
      i++;
      while (i < src.length) {
        if (src[i] === "'" && src[i + 1] === "'") {
          i += 2;
          continue;
        }
        if (src[i] === "'") {
          i++;
          break;
        }
        i++;
      }
      continue;
    }
    if (c === '-' && src[i + 1] === '-') {
      flush();
      while (i < src.length && src[i] !== '\n') i++;
      continue;
    }
    if (c === '/' && src[i + 1] === '*') {
      flush();
      i += 2;
      while (i < src.length && !(src[i] === '*' && src[i + 1] === '/')) i++;
      i += 2;
      continue;
    }
    if (/\s/.test(c)) {
      // 空白不是标点：直接跳过，否则 FROM 后面的空格会被当成表列表的终止符
      flush();
      i++;
      continue;
    }
    flush();
    out.push({ word: c, quoted: false, sep: true });
    i++;
  }
  flush();
  return out;
}

const STOP_WORDS = new Set(keywords('merged'));

/** 解析语句里的 FROM/JOIN 子句：别名 → 表名 映射，以及出现在作用域里的表（按出现顺序） */
function parseScope(stmt: string): { aliases: Map<string, string>; tables: string[] } {
  const toks = tokenize(stmt);
  const aliases = new Map<string, string>();
  const tables: string[] = [];
  for (let i = 0; i < toks.length; i++) {
    const w = toks[i].word.toUpperCase();
    if (w !== 'FROM' && w !== 'JOIN') continue;
    let j = i + 1;
    while (j < toks.length) {
      const t = toks[j];
      if (!t || t.sep) break;
      tables.push(t.word);
      j++;
      const after = toks[j];
      if (after && !after.sep && after.word.toUpperCase() === 'AS' && toks[j + 1] && !toks[j + 1].sep) {
        aliases.set(toks[j + 1].word.toLowerCase(), t.word);
        j += 2;
      } else if (after && !after.sep && (after.quoted || !STOP_WORDS.has(after.word.toUpperCase()))) {
        aliases.set(after.word.toLowerCase(), t.word);
        j += 1;
      }
      if (toks[j] && toks[j].sep && toks[j].word === ',') {
        j++;
        continue;
      }
      break;
    }
  }
  return { aliases, tables };
}

function detectContext(prefixTokens: Token[]): 'table' | 'column' | 'any' {
  for (let i = prefixTokens.length - 1; i >= 0; i--) {
    const t = prefixTokens[i];
    if (t.sep) continue;
    const w = t.word.toUpperCase();
    if (TABLE_CTX.has(w)) return 'table';
    if (COLUMN_CTX.has(w)) return 'column';
  }
  return 'any';
}

/** 按 schema 里的原始拼写找到表名（列索引的 key 大小写可能与 SQL 里写的不一致） */
function realTableName(name: string, schema: CompletionSchema): string | null {
  const lower = name.toLowerCase();
  return schema.tables.find((t) => t.toLowerCase() === lower) ?? null;
}

function columnItems(table: string, schema: CompletionSchema): CompletionItem[] {
  const real = realTableName(table, schema) ?? table;
  return (schema.columns[real] ?? []).map((c) => ({
    label: c.name,
    kind: 'column' as const,
    detail: c.type_name,
  }));
}

/** 作用域内的表优先，其余表其次（两边的字段都会给出） */
function scopedColumnItems(schema: CompletionSchema, scopeTables: string[]): CompletionItem[] {
  const order: string[] = [];
  const push = (t: string | null) => {
    if (t && !order.some((x) => x.toLowerCase() === t.toLowerCase())) order.push(t);
  };
  for (const t of scopeTables) push(realTableName(t, schema) ?? t);
  for (const t of schema.tables) push(t);
  return order.flatMap((t) => columnItems(t, schema));
}

function qualifierColumns(qualifier: string, schema: CompletionSchema, aliases: Map<string, string>): CompletionItem[] {
  const key = qualifier.toLowerCase();
  const target = aliases.get(key) ?? schema.tables.find((t) => t.toLowerCase() === key);
  if (!target) return scopedColumnItems(schema, []); // 解析不到 → 该位置可见的全部字段
  return columnItems(target, schema);
}

export function complete(input: CompleteInput): CompletionResult | null {
  const { text, caret, dialect, schema } = input;
  if (!Number.isInteger(caret) || caret < 0 || caret > text.length) return null;

  const back = scanBack(text, caret);
  // 字符串或注释内部不补全
  if (back.state === 'string' || back.state === 'line' || back.state === 'block') return null;

  const quoted = back.state === 'quoted' && back.quotedStart >= 0;
  let prefixFrom: number;
  if (quoted) {
    prefixFrom = back.quotedStart;
  } else {
    prefixFrom = caret;
    while (prefixFrom > back.stmtStart && IDENT.test(text[prefixFrom - 1])) prefixFrom--;
  }

  // 限定符（alias. 或 table.）
  let qualifier: string | null = null;
  if (!quoted && prefixFrom > back.stmtStart && text[prefixFrom - 1] === '.') {
    const qTo = prefixFrom - 1;
    let qFrom = qTo;
    if (qTo > back.stmtStart && text[qTo - 1] === '"') {
      let k = qTo - 2;
      while (k >= back.stmtStart && !(text[k] === '"' && text[k - 1] !== '"')) k--;
      qFrom = k >= back.stmtStart ? k : qTo;
    } else {
      while (qFrom > back.stmtStart && IDENT.test(text[qFrom - 1])) qFrom--;
    }
    const raw = text.slice(qFrom, qTo);
    qualifier = raw.startsWith('"') && raw.endsWith('"') ? raw.slice(1, -1).replace(/""/g, '"') : raw;
  }

  const rawPrefix = quoted ? text.slice(prefixFrom + 1, caret) : text.slice(prefixFrom, caret);
  const prefix = quoted ? rawPrefix.replace(/""/g, '"') : rawPrefix;

  // 普通位置必须至少敲了一个字符才弹；点号后与引号内允许空前缀
  if (!quoted && !qualifier && prefix.length === 0) return null;

  const stmtEnd = scanForward(text, caret, quoted ? 'quoted' : 'code');
  const stmt = text.slice(back.stmtStart, stmtEnd);
  const { aliases, tables: scopeTables } = parseScope(stmt);

  let candidates: CompletionItem[];
  if (qualifier !== null) {
    candidates = qualifierColumns(qualifier, schema, aliases);
  } else {
    const ctx = detectContext(tokenize(text.slice(back.stmtStart, caret)));
    if (ctx === 'table') {
      candidates = schema.tables.map((t) => ({ label: t, kind: 'table' as const }));
    } else if (ctx === 'column') {
      candidates = scopedColumnItems(schema, scopeTables);
    } else {
      candidates = [
        ...schema.tables.map((t) => ({ label: t, kind: 'table' as const })),
        ...functions(dialect).map((f) => ({ label: f, kind: 'function' as const })),
        ...keywords(dialect).map((k) => ({ label: k, kind: 'keyword' as const })),
      ];
    }
  }

  const q = prefix.toLowerCase();
  const seen = new Set<string>();
  const matched = candidates.filter((c) => {
    const key = c.label.toLowerCase();
    if (!key.startsWith(q) || seen.has(key)) return false;
    seen.add(key);
    return true;
  });
  if (matched.length === 0) return null;

  const weight: Record<CompletionKind, number> = { column: 0, table: 0, function: 1, keyword: 2 };
  const items = matched
    .map((c, idx) => ({ c, idx }))
    .sort(
      (a, b) =>
        weight[a.c.kind] - weight[b.c.kind] ||
        a.c.label.length - b.c.label.length ||
        a.c.label.toLowerCase().localeCompare(b.c.label.toLowerCase()) ||
        a.idx - b.idx,
    )
    .slice(0, MAX_ITEMS)
    .map((x) => x.c);

  const finalItems = quoted
    ? items.map((i) => ({ ...i, insertText: `"${i.label.replace(/"/g, '""')}"` }))
    : items;

  return { items: finalItems, replaceFrom: prefixFrom, replaceTo: caret };
}

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
  /** 手动触发（Ctrl/Cmd+Space）：即使没有上下文、空前缀也弹 */
  force?: boolean;
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
/** 会引入表名的关键字：FROM/JOIN 之外还有 UPDATE 与 INSERT INTO（它们的目标表同样属于作用域） */
const TABLE_INTRO = new Set(['FROM', 'JOIN', 'UPDATE', 'INTO']);

/** 解析语句里的 FROM/JOIN 子句：别名 → 表名 映射，以及出现在作用域里的表（按出现顺序） */
function parseScope(stmt: string): { aliases: Map<string, string>; tables: string[] } {
  const toks = tokenize(stmt);
  const aliases = new Map<string, string>();
  const tables: string[] = [];
  for (let i = 0; i < toks.length; i++) {
    const t0 = toks[i];
    if (t0.sep || !TABLE_INTRO.has(t0.word.toUpperCase())) continue;
    let j = i + 1;
    while (j < toks.length) {
      const t = toks[j];
      if (!t || t.sep) break;
      // 关键字不是表名：`FOR UPDATE OF books` 里的 OF、`INSERT INTO books VALUES` 里的 VALUES
      if (!t.quoted && STOP_WORDS.has(t.word.toUpperCase())) break;
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

/** 光标处是否还在未闭合的括号内 */
function parenDepth(toks: Token[]): number {
  let depth = 0;
  for (const t of toks) {
    if (!t.sep) continue;
    if (t.word === '(') depth++;
    else if (t.word === ')') depth--;
  }
  return depth;
}

/** 前缀里出现过 INTO（即这是一条 INSERT） */
function hasInto(toks: Token[]): boolean {
  return toks.some((t) => !t.sep && t.word.toUpperCase() === 'INTO');
}

/**
 * INSERT 列清单：`INTO <表> (` 且光标处这个括号还没闭合时返回该表名，用于只提示该表的字段。
 * 只看最后一个 INTO；括号一旦闭合就说明光标落在后面的括号里（例如 VALUES 的值括号），返回 null。
 */
function insertColumnTarget(prefixToks: Token[]): string | null {
  for (let i = prefixToks.length - 1; i >= 0; i--) {
    const t = prefixToks[i];
    if (t.sep || t.word.toUpperCase() !== 'INTO') continue;
    const tbl = prefixToks[i + 1];
    const open = prefixToks[i + 2];
    if (!tbl || tbl.sep || !open || !open.sep || open.word !== '(') return null;
    let depth = 0;
    for (let k = i + 2; k < prefixToks.length; k++) {
      const s = prefixToks[k];
      if (!s.sep) continue;
      if (s.word === '(') depth++;
      else if (s.word === ')') {
        depth--;
        if (depth === 0) return null;
      }
    }
    return depth > 0 ? tbl.word : null;
  }
  return null;
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

  const stmtEnd = scanForward(text, caret, quoted ? 'quoted' : 'code');
  const stmt = text.slice(back.stmtStart, stmtEnd);
  const { aliases, tables: scopeTables } = parseScope(stmt);
  const prefixToks = tokenize(text.slice(back.stmtStart, caret));
  const ctx = qualifier !== null ? null : detectContext(prefixToks);
  const insertTarget = qualifier !== null || quoted ? null : insertColumnTarget(prefixToks);

  // 空前缀的弹出门槛：点号后、引号内、INSERT 列清单里、手动触发，或光标刚好处在 FROM/SELECT 这类上下文关键字之后。
  // 空白编辑器（没有上下文）不弹，避免一打开页签就冒出一个列表。
  const allowEmpty =
    quoted ||
    qualifier !== null ||
    insertTarget !== null ||
    input.force === true ||
    (ctx !== null && ctx !== 'any');
  if (prefix.length === 0 && !allowEmpty) return null;

  let candidates: CompletionItem[];
  if (qualifier !== null) {
    candidates = qualifierColumns(qualifier, schema, aliases);
  } else if (insertTarget !== null) {
    // INSERT 列清单里只可能填该表的字段
    candidates = columnItems(insertTarget, schema);
  } else if (ctx === 'table') {
    // INSERT ... VALUES ( 这类位置：表名没有意义，给函数与关键字（NULL/DEFAULT/now()…）
    candidates =
      hasInto(prefixToks) && parenDepth(prefixToks) > 0
        ? [
            ...functions(dialect).map((f) => ({ label: f, kind: 'function' as const })),
            ...keywords(dialect).map((k) => ({ label: k, kind: 'keyword' as const })),
          ]
        : schema.tables.map((t) => ({ label: t, kind: 'table' as const }));
  } else if (ctx === 'column') {
    // 字段排在最前面，但关键字与函数也一并给出（否则 WHERE 之后连 EXISTS 都补不出来）
    candidates = [
      ...scopedColumnItems(schema, scopeTables),
      ...functions(dialect).map((f) => ({ label: f, kind: 'function' as const })),
      ...keywords(dialect).map((k) => ({ label: k, kind: 'keyword' as const })),
    ];
  } else {
    candidates = [
      ...schema.tables.map((t) => ({ label: t, kind: 'table' as const })),
      ...functions(dialect).map((f) => ({ label: f, kind: 'function' as const })),
      ...keywords(dialect).map((k) => ({ label: k, kind: 'keyword' as const })),
    ];
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

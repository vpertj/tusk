import { describe, it, expect } from 'vitest';
import { complete, type CompletionSchema } from './sql-complete';

const schema: CompletionSchema = {
  tables: ['book_pages', 'books', 'item_audits'],
  columns: {
    book_pages: [{ name: 'id' }, { name: 'title' }, { name: 'created_at' }],
    books: [{ name: 'id' }, { name: 'name' }],
  },
};

/** 用 | 标记光标位置，返回 { text, caret } */
function split(marked: string) {
  const caret = marked.indexOf('|');
  if (caret < 0) throw new Error('测试文本里必须有 | 标记光标');
  return { text: marked.slice(0, caret) + marked.slice(caret + 1), caret };
}

function run(marked: string, s: CompletionSchema = schema, dialect = 'postgresql' as const) {
  const { text, caret } = split(marked);
  return complete({ text, caret, dialect, schema: s });
}

describe('上下文判定', () => {
  it('FROM 之后提示表名', () => {
    const r = run('SELECT * FROM bo|');
    expect(r?.items[0].kind).toBe('table');
    expect(r?.items.map((i) => i.label)).toContain('book_pages');
  });

  it('JOIN 之后提示表名', () => {
    const r = run('SELECT * FROM books b JOIN ite|');
    expect(r?.items[0].kind).toBe('table');
    expect(r?.items.map((i) => i.label)).toContain('item_audits');
  });

  it('UPDATE 之后提示表名', () => {
    const r = run('UPDATE books|');
    expect(r?.items[0].kind).toBe('table');
    expect(r?.items[0].label).toBe('books');
  });

  it('SELECT 之后提示字段名', () => {
    const r = run('SELECT na| FROM books');
    expect(r?.items[0].kind).toBe('column');
    expect(r?.items[0].label).toBe('name');
  });

  it('ORDER BY 之后提示字段名', () => {
    const r = run('SELECT * FROM books ORDER BY na|');
    expect(r?.items[0].kind).toBe('column');
    expect(r?.items[0].label).toBe('name');
  });

  it('别名点号之后只提示该表的字段', () => {
    const r = run('SELECT b.| FROM books b');
    expect(r?.items.map((i) => i.kind)).toEqual(['column', 'column']);
    expect(r?.items.map((i) => i.label).sort()).toEqual(['id', 'name']);
  });

  it('用表名作限定符时也解析得到该表字段', () => {
    const r = run('SELECT book_pages.ti| FROM book_pages');
    expect(r?.items[0].label).toBe('title');
    expect(r?.items[0].kind).toBe('column');
  });

  it('限定符不是已知别名或表名时，退化为所有字段', () => {
    const r = run('SELECT zz.|');
    expect(r).not.toBeNull();
    expect(r?.items.every((i) => i.kind === 'column')).toBe(true);
    expect(r?.items.length).toBeGreaterThan(0);
  });

  it('语句开头的空白位置提示关键字', () => {
    const r = run('sel|');
    expect(r?.items[0].kind).toBe('keyword');
    expect(r?.items[0].label).toBe('SELECT');
  });
});

describe('字符串与注释', () => {
  it("字符串字面量里的分号不截断语句上下文", () => {
    const r = run("SELECT * FROM books WHERE name = 'a;b' AND na|");
    expect(r?.items[0].kind).toBe('column');
    expect(r?.items[0].label).toBe('name');
  });

  it('光标在字符串字面量内不补全', () => {
    expect(run("SELECT * FROM books WHERE name = 'ab|'")).toBeNull();
  });

  it('光标在行注释内不补全', () => {
    expect(run('SELECT 1 -- ab|')).toBeNull();
  });

  it('光标在块注释内不补全', () => {
    expect(run('SELECT 1 /* ab| */')).toBeNull();
  });

  it('多语句时只看光标所在的最后一条语句', () => {
    const r = run("INSERT INTO books VALUES (1); SELECT na| FROM books");
    expect(r?.items[0].kind).toBe('column');
  });
});

describe('替换范围', () => {
  it('普通前缀只替换前缀本身', () => {
    const r = run('SELECT * FROM boo|');
    expect(r).not.toBeNull();
    expect('SELECT * FROM boo'.slice(r!.replaceFrom, r!.replaceTo)).toBe('boo');
  });

  it('限定符前缀不吞掉点号', () => {
    const r = run('SELECT b.na| FROM books b');
    expect('SELECT b.na'.slice(r!.replaceFrom, r!.replaceTo)).toBe('na');
  });

  it('带引号的前缀替换整段引号并补回引号', () => {
    const r = run('SELECT * FROM "book_pa|');
    expect(r?.items[0].label).toBe('book_pages');
    expect(r?.items[0].insertText).toBe('"book_pages"');
    expect('SELECT * FROM "book_pa'.slice(r!.replaceFrom, r!.replaceTo)).toBe('"book_pa');
  });
});

describe('过滤、排序与门槛', () => {
  it('大小写不敏感匹配且表名保留原名', () => {
    const r = run('SELECT * FROM BOOK|');
    expect(r?.items.map((i) => i.label)).toContain('books');
  });

  it('表名与字段名优先于关键字', () => {
    const r = run('SELECT * FROM bo|');
    const kinds = r!.items.map((i) => i.kind);
    expect(kinds.indexOf('table')).toBeLessThan(kinds.length);
    expect(r!.items[0].kind).toBe('table');
  });

  it('普通位置空前缀不弹', () => {
    expect(run('SELECT * FROM |')).toBeNull();
  });

  it('点号后空前缀也弹', () => {
    const r = run('SELECT b.| FROM books b');
    expect(r).not.toBeNull();
    expect(r!.items.length).toBeGreaterThan(0);
  });

  it('没有匹配项时返回 null', () => {
    expect(run('SELECT * FROM zzzzzz|')).toBeNull();
  });

  it('候选上限 50 条', () => {
    const many: CompletionSchema = {
      tables: Array.from({ length: 100 }, (_, i) => `t${String(i).padStart(3, '0')}`),
      columns: {},
    };
    const r = run('SELECT * FROM t|', many);
    expect(r?.items.length).toBe(50);
  });
});

describe('降级', () => {
  it('没有字段信息时不产生字段候选', () => {
    const r = run('SELECT na| FROM books', { tables: ['books'], columns: {} });
    expect(r === null || r.items.every((i) => i.kind !== 'column')).toBe(true);
  });

  it('完全没有 schema 时仍能提示关键字', () => {
    const r = run('sel|', { tables: [], columns: {} });
    expect(r?.items.map((i) => i.label)).toContain('SELECT');
  });
});

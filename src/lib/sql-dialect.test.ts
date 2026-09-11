import { describe, it, expect } from 'vitest';
import { keywords, functions } from './sql-dialect';

describe('sql-dialect', () => {
  it('每个方言都有非空的关键字与函数表', () => {
    for (const d of ['postgresql', 'sqlite', 'merged'] as const) {
      expect(keywords(d).length).toBeGreaterThan(50);
      expect(functions(d).length).toBeGreaterThan(20);
    }
  });

  it('merged 是 postgresql 与 sqlite 的并集', () => {
    const merged = new Set(keywords('merged'));
    for (const d of ['postgresql', 'sqlite'] as const) {
      for (const k of keywords(d)) expect(merged.has(k)).toBe(true);
    }
  });

  it('关键字与函数一律大写且不重复', () => {
    for (const d of ['postgresql', 'sqlite', 'merged'] as const) {
      for (const w of [...keywords(d), ...functions(d)]) {
        expect(w).toBe(w.toUpperCase());
      }
      expect(new Set(keywords(d)).size).toBe(keywords(d).length);
      expect(new Set(functions(d)).size).toBe(functions(d).length);
    }
  });
});

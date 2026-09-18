import { describe, it, expect } from 'vitest';
import { resolveConnName, type SavedConnLike, type ConnDraft } from './conn-name';

const base: ConnDraft = {
  dbType: 'postgres',
  host: '192.168.10.10',
  port: 5432,
  user: 'user_kd',
  dbname: 'postgres',
  path: '',
  sshEnabled: false,
  sshHost: '',
  sshPort: 22,
  sshUser: '',
};

const saved = (over: Partial<SavedConnLike>): SavedConnLike => ({
  db_type: 'postgres',
  name: '旧连接',
  host: '192.168.10.10',
  port: 5432,
  user: 'user_kd',
  dbname: 'postgres',
  path: null,
  ssh_enabled: false,
  ssh_host: '',
  ssh_port: 22,
  ssh_user: '',
  ...over,
});

describe('resolveConnName', () => {
  it('用户填了名字就直接用（即使存在参数相同的旧连接）', () => {
    const name = resolveConnName([saved({ name: '生产库' })], base, '我自己起的名');
    expect(name).toBe('我自己起的名');
  });

  it('留空且存在参数完全相同的旧连接 → 复用旧名字（覆盖更新，不产生重复）', () => {
    expect(resolveConnName([saved({ name: '测试环境数据库' })], base, '')).toBe('测试环境数据库');
  });

  it('端口的数字/字符串差异不影响匹配', () => {
    expect(resolveConnName([saved({ name: '测试环境数据库', port: '5432' as unknown as number })], { ...base, port: '5432' }, '')).toBe('测试环境数据库');
  });

  it('path 为 null 的旧记录与空字符串等价', () => {
    expect(resolveConnName([saved({ name: '测试环境数据库', path: null })], base, '')).toBe('测试环境数据库');
  });

  it('参数不同（用户名不一样）→ 自动生成新名字', () => {
    const name = resolveConnName([saved({ name: '测试环境数据库' })], { ...base, user: '另一个人' }, '');
    expect(name).toBe('另一个人@192.168.10.10:5432/postgres');
  });

  it('自动命名格式：user@host:port/db', () => {
    expect(resolveConnName([], base, '')).toBe('user_kd@192.168.10.10:5432/postgres');
  });

  it('用户名为空时省略 user@', () => {
    expect(resolveConnName([], { ...base, user: '' }, '')).toBe('192.168.10.10:5432/postgres');
  });

  it('SQLite 按文件路径匹配并复用旧名', () => {
    const s: SavedConnLike = {
      ...saved({ name: '本地库', db_type: 'sqlite', host: '', user: '', dbname: '', path: '/data/a.db' }),
    };
    const d: ConnDraft = { ...base, dbType: 'sqlite', host: '', user: '', dbname: '', path: '/data/a.db' };
    expect(resolveConnName([s], d, '')).toBe('本地库');
  });

  it('SQLite 无匹配 → 用文件名自动命名', () => {
    const d: ConnDraft = { ...base, dbType: 'sqlite', host: '', user: '', dbname: '', path: '/data/demo.db' };
    expect(resolveConnName([], d, '')).toBe('SQLite·demo.db');
  });

  it('SSH 开启时 ssh 配置参与比较：配置不同不复用', () => {
    const old1 = saved({ name: '走隧道', ssh_enabled: true, ssh_host: 'jump1', ssh_port: 22, ssh_user: 'ops' });
    const d: ConnDraft = { ...base, sshEnabled: true, sshHost: 'jump2', sshPort: 22, sshUser: 'ops' };
    expect(resolveConnName([old1], d, '')).toBe('user_kd@192.168.10.10:5432/postgres');
  });

  it('SSH 关闭时 ssh 字段不参与比较：填了不同的 ssh_host 也复用旧名', () => {
    const old1 = saved({ name: '测试环境数据库', ssh_host: '无所谓', ssh_enabled: false });
    expect(resolveConnName([old1], base, '')).toBe('测试环境数据库');
  });
});

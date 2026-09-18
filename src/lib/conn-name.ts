/** 已保存连接里参与比较/命名的最小字段（与后端 SavedConn 对齐，缺省走默认值） */
export type SavedConnLike = {
  db_type: string;
  name: string;
  host: string;
  port: number;
  user: string;
  dbname: string;
  path?: string | null;
  ssh_enabled?: boolean;
  ssh_host?: string;
  ssh_port?: number;
  ssh_user?: string;
};

/** 连接弹窗里草拟的参数 */
export type ConnDraft = {
  dbType: string;
  host: string;
  port: number | string;
  user: string;
  dbname: string;
  path: string;
  sshEnabled: boolean;
  sshHost: string;
  sshPort: number | string;
  sshUser: string;
};

/** 旧记录与草稿的参数是否完全一致（决定复用旧名字做覆盖更新，还是生成新记录） */
function sameConn(a: SavedConnLike, d: ConnDraft): boolean {
  if (a.db_type !== d.dbType) return false;
  if (a.host !== d.host) return false;
  if (Number(a.port) !== Number(d.port)) return false;
  if (a.user !== d.user) return false;
  if (a.dbname !== d.dbname) return false;
  if ((a.path ?? '') !== d.path) return false;
  if ((a.ssh_enabled ?? false) !== d.sshEnabled) return false;
  // 隧道没开时，ssh 字段是残留值，不参与比较
  if (d.sshEnabled) {
    if ((a.ssh_host ?? '') !== d.sshHost) return false;
    if (Number(a.ssh_port ?? 22) !== Number(d.sshPort || 22)) return false;
    if ((a.ssh_user ?? '') !== d.sshUser) return false;
  }
  return true;
}

function autoName(d: ConnDraft): string {
  if (d.dbType === 'sqlite') {
    const file = d.path.split('/').pop() || d.path;
    return `SQLite·${file}`;
  }
  return d.user ? `${d.user}@${d.host}:${d.port}/${d.dbname}` : `${d.host}:${d.port}/${d.dbname}`;
}

/**
 * 连接成功后要保存的名字。
 * 用户填了名字就直接用；留空时优先复用"参数完全相同"的旧连接名（覆盖更新、绝不产生重复），
 * 没有就按参数自动生成 —— 保证勾选保存的连接一定被记住。
 */
export function resolveConnName(
  saved: SavedConnLike[],
  draft: ConnDraft,
  userNamed: string,
): string {
  const named = userNamed.trim();
  if (named) return named;
  const same = saved.find((s) => sameConn(s, draft));
  return same ? same.name : autoName(draft);
}

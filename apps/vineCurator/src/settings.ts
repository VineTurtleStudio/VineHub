import { open, save } from '@tauri-apps/plugin-dialog';
import {
  addRoot,
  backupNow,
  changePassword,
  createUser,
  deleteRule,
  deleteUser,
  exportPort,
  getSettings,
  importPort,
  listAudit,
  listDupes,
  listRoots,
  listRules,
  listUsers,
  login,
  logout,
  ocrPendingCount,
  probeTools,
  redirectRoot,
  removeRoot,
  revealPath,
  saveRule,
  saveSettings,
  sessionUser,
  setupAdmin,
  setUserRole,
  startDupeJob,
  startHashJob,
  startMetaJob,
  startOcrJob,
  startRuleJob,
  startAutocatJob,
  startScan,
  type AuditRow,
  type DupeGroup,
  type RootEntry,
  type RuleRow,
  type SettingEntry,
  type ToolStatus,
} from './api';
import { state } from './state';

function el<T extends HTMLElement>(id: string): T {
  return document.getElementById(id) as T;
}

export const toastFn: (msg: string, ok?: boolean) => void = () => {};

let toastRef: (msg: string, ok?: boolean) => void = (msg, ok) => toastFn(msg, ok);
export function setToast(fn: (msg: string, ok?: boolean) => void): void {
  toastRef = fn;
}

const rootsBody = el<HTMLTableSectionElement>('roots-body');
const rootForm = el<HTMLFormElement>('root-add-form');
const rootPathInput = el<HTMLInputElement>('root-path');
const rootLabelInput = el<HTMLInputElement>('root-label');
const derivedForm = el<HTMLFormElement>('derived-form');
const derivedInput = el<HTMLInputElement>('derived-dir');
const toolList = el<HTMLDivElement>('tool-list');
const metaScope = el<HTMLSelectElement>('meta-scope');
const beetsPathInput = el<HTMLInputElement>('beets-db-path');
const calibrePathInput = el<HTMLInputElement>('calibre-lib-path');
const rulesBody = el<HTMLTableSectionElement>('rules-body');
const ruleForm = el<HTMLFormElement>('rule-form');
const ruleIdInput = el<HTMLInputElement>('rule-id');
const ruleNameInput = el<HTMLInputElement>('rule-name');
const rulePriorityInput = el<HTMLInputElement>('rule-priority');
const ruleEnabledInput = el<HTMLInputElement>('rule-enabled');
const ruleConditionsInput = el<HTMLInputElement>('rule-conditions');
const ruleActionsInput = el<HTMLInputElement>('rule-actions');
const sessionInfo = el<HTMLElement>('session-info');
const setupAdminForm = el<HTMLFormElement>('setup-admin-form');
const loginForm = el<HTMLFormElement>('login-form');
const changePwForm = el<HTMLFormElement>('change-pw-form');
const createUserForm = el<HTMLFormElement>('create-user-form');
const usersBody = el<HTMLTableSectionElement>('users-body');
const auditBody = el<HTMLTableSectionElement>('audit-body');
const auditEntitySel = el<HTMLSelectElement>('audit-entity');
const backupForm = el<HTMLFormElement>('backup-form');
const backupIntervalInput = el<HTMLInputElement>('backup-interval');
const backupKeepInput = el<HTMLInputElement>('backup-keep');
const backupDirInput = el<HTMLInputElement>('backup-dir');
const resticRepoInput = el<HTMLInputElement>('restic-repo');
const resticPwInput = el<HTMLInputElement>('restic-password-file');
const backupLastBadge = el<HTMLElement>('backup-last');
const autocatDepthInput = el<HTMLInputElement>('autocat-depth');

let rules: RuleRow[] = [];

export let settings = new Map<string, string>();

async function pickDir(title: string): Promise<string | null> {
  try {
    const sel = await open({ directory: true, multiple: false, title });
    return typeof sel === 'string' ? sel : null;
  } catch (err) {
    toastRef(String(err), false);
    return null;
  }
}

function renderRoots(): void {
  rootsBody.innerHTML = '';
  if (state.roots.length === 0) {
    const tr = document.createElement('tr');
    const td = document.createElement('td');
    td.colSpan = 4;
    td.className = 'empty';
    td.textContent = '尚未添加根目录 — 选择外置硬盘上的媒体目录开始索引';
    tr.appendChild(td);
    rootsBody.appendChild(tr);
    return;
  }
  for (const r of state.roots) {
    const tr = document.createElement('tr');
    const tdPath = document.createElement('td');
    tdPath.className = 'path';
    tdPath.textContent = r.path;
    tdPath.title = r.path;
    const tdLabel = document.createElement('td');
    tdLabel.textContent = r.label || '—';
    const tdCount = document.createElement('td');
    tdCount.className = 'num';
    tdCount.textContent = String(r.item_count);
    const tdOps = document.createElement('td');
    tdOps.className = 'ops ops-wide';
    const mkBtn = (label: string, act: string, cls: string): HTMLButtonElement => {
      const b = document.createElement('button');
      b.type = 'button';
      b.className = cls;
      b.dataset.act = act;
      b.dataset.id = String(r.id);
      b.textContent = label;
      b.disabled = state.jobRunning;
      return b;
    };
    tdOps.append(
      mkBtn('扫描', 'scan', 'btn sm'),
      mkBtn('哈希', 'hash', 'btn-ghost sm'),
      mkBtn('重定向', 'redirect', 'btn-ghost sm'),
      mkBtn('删除索引', 'remove', 'btn-ghost sm danger'),
    );
    tr.append(tdPath, tdLabel, tdCount, tdOps);
    rootsBody.appendChild(tr);
  }
}

function renderMetaScope(): void {
  const cur = metaScope.value;
  metaScope.innerHTML = '';
  const optAll = document.createElement('option');
  optAll.value = '';
  optAll.textContent = '全部根目录';
  metaScope.appendChild(optAll);
  for (const r of state.roots) {
    const opt = document.createElement('option');
    opt.value = String(r.id);
    opt.textContent = r.label || r.path;
    metaScope.appendChild(opt);
  }
  if ([...metaScope.options].some((o) => o.value === cur)) {
    metaScope.value = cur;
  }
}

export async function refreshRoots(): Promise<void> {
  state.roots = await listRoots();
  renderRoots();
  renderMetaScope();
}

export function refreshRootsView(): void {
  renderRoots();
  renderMetaScope();
}

async function onRootAction(ev: Event): Promise<void> {
  const btn = ev.target as HTMLButtonElement;
  const act = btn.dataset.act;
  const id = Number(btn.dataset.id);
  if (!act || !id) return;
  const root = state.roots.find((r) => r.id === id);
  if (!root) return;
  if (act === 'remove') {
    const ok = confirm(
      `删除根目录只清除索引（${root.item_count} 条），磁盘文件保持原样。确定？\n${root.path}`,
    );
    if (!ok) return;
    try {
      await removeRoot(id);
      toastRef('已删除索引');
      await refreshRoots();
    } catch (err) {
      toastRef(String(err), false);
    }
  } else if (act === 'redirect') {
    try {
      const dir = await pickDir('选择根目录的新位置');
      if (!dir) return;
      await redirectRoot(id, dir);
      toastRef('已重定向，条目按相对路径保留');
      await refreshRoots();
    } catch (err) {
      toastRef(String(err), false);
    }
  } else if (act === 'scan') {
    window.dispatchEvent(new CustomEvent('vc:begin-job', { detail: '扫描' }));
    try {
      await startScan(id);
    } catch (err) {
      toastRef(String(err), false);
      window.dispatchEvent(new CustomEvent('vc:end-job', { detail: '扫描启动失败' }));
    }
  } else if (act === 'hash') {
    window.dispatchEvent(new CustomEvent('vc:begin-job', { detail: '哈希' }));
    try {
      await startHashJob(id);
    } catch (err) {
      toastRef(String(err), false);
      window.dispatchEvent(new CustomEvent('vc:end-job', { detail: '哈希启动失败' }));
    }
  }
}

function renderTools(tools: ToolStatus[]): void {
  toolList.innerHTML = '';
  for (const t of tools) {
    const row = document.createElement('div');
    row.className = `tool-row ${t.found ? 'ok' : 'missing'}`;
    const dot = document.createElement('span');
    dot.className = 'dot';
    const name = document.createElement('span');
    name.className = 'tool-name';
    name.textContent = t.name;
    const version = document.createElement('span');
    version.className = 'tool-version';
    version.textContent = t.found ? (t.version ?? '') : '未安装';
    const input = document.createElement('input');
    input.type = 'text';
    input.className = 'tool-path';
    input.placeholder = 'PATH 自动探测';
    input.value = t.path ?? '';
    input.dataset.tool = t.name;
    input.addEventListener('change', onToolPathChange);
    row.append(dot, name, version, input);
    toolList.appendChild(row);
  }
}

const overrideKey = (name: string): string => `${name.replace(/-/g, '_')}_path`;

async function onToolPathChange(ev: Event): Promise<void> {
  const input = ev.target as HTMLInputElement;
  const tool = input.dataset.tool;
  if (!tool) return;
  const key = overrideKey(tool);
  try {
    await saveSettings([{ key, value: input.value.trim() }]);
    toastRef(input.value.trim() ? `已保存 ${key}` : `已恢复 ${tool} 自动探测`);
    settings.set(key, input.value.trim());
    renderTools((await probeTools()).tools);
  } catch (err) {
    toastRef(String(err), false);
  }
}

export async function refreshSettings(): Promise<void> {
  const entries = await getSettings();
  settings = new Map(entries.map((e: SettingEntry) => [e.key, e.value]));
  derivedInput.value = settings.get('derived_dir') ?? '';
  beetsPathInput.value = settings.get('beets_db_path') ?? '';
  calibrePathInput.value = settings.get('calibre_library_path') ?? '';
  backupIntervalInput.value = settings.get('backup_interval_hours') ?? '0';
  backupKeepInput.value = settings.get('backup_keep') ?? '7';
  backupDirInput.value = settings.get('backup_dir') ?? '';
  resticRepoInput.value = settings.get('restic_repo') ?? '';
  resticPwInput.value = settings.get('restic_password_file') ?? '';
  autocatDepthInput.value = settings.get('auto_cat_depth') ?? '1';
  refreshBackupLastBadge();
  applyBackupSchedule();
}

// ---------------------------------------------------------------- 规则自动打标

function shortJson(raw: string, max = 60): string {
  const one = raw.replace(/\s+/g, ' ').trim();
  return one.length > max ? `${one.slice(0, max)}…` : one;
}

function renderRules(): void {
  rulesBody.innerHTML = '';
  if (rules.length === 0) {
    const tr = document.createElement('tr');
    const td = document.createElement('td');
    td.colSpan = 6;
    td.className = 'empty';
    td.textContent = '尚无规则 — 新建后可按条件批量打标，扫描/导入后自动运行';
    tr.appendChild(td);
    rulesBody.appendChild(tr);
    return;
  }
  for (const r of rules) {
    const tr = document.createElement('tr');
    const tdName = document.createElement('td');
    tdName.textContent = r.name;
    const tdPrio = document.createElement('td');
    tdPrio.className = 'num';
    tdPrio.textContent = String(r.priority);
    const tdState = document.createElement('td');
    tdState.textContent = r.enabled ? '启用' : '停用';
    const tdCond = document.createElement('td');
    tdCond.className = 'path';
    tdCond.textContent = shortJson(r.conditions);
    tdCond.title = r.conditions;
    const tdAct = document.createElement('td');
    tdAct.className = 'path';
    tdAct.textContent = shortJson(r.actions, 40);
    tdAct.title = r.actions;
    const tdOps = document.createElement('td');
    tdOps.className = 'ops ops-wide';
    const mkBtn = (label: string, act: string, cls: string): HTMLButtonElement => {
      const b = document.createElement('button');
      b.type = 'button';
      b.className = cls;
      b.dataset.act = act;
      b.dataset.id = String(r.id);
      b.textContent = label;
      b.disabled = state.jobRunning;
      return b;
    };
    tdOps.append(
      mkBtn('编辑', 'edit', 'btn-ghost sm'),
      mkBtn('删除', 'delete', 'btn-ghost sm danger'),
    );
    tr.append(tdName, tdPrio, tdState, tdCond, tdAct, tdOps);
    rulesBody.appendChild(tr);
  }
}

export async function refreshRules(): Promise<void> {
  try {
    rules = await listRules();
    renderRules();
  } catch (err) {
    toastRef(String(err), false);
  }
}

function openRuleForm(rule: RuleRow | null): void {
  ruleForm.hidden = false;
  ruleIdInput.value = rule ? String(rule.id) : '';
  ruleNameInput.value = rule?.name ?? '';
  rulePriorityInput.value = rule ? String(rule.priority) : '0';
  ruleEnabledInput.checked = rule?.enabled ?? true;
  ruleConditionsInput.value = rule?.conditions ?? '';
  ruleActionsInput.value = rule?.actions ?? '[{"op":"add_tag","value":""}]';
  ruleNameInput.focus();
}

async function onRuleRowAction(ev: Event): Promise<void> {
  const btn = ev.target as HTMLButtonElement;
  const act = btn.dataset.act;
  const id = Number(btn.dataset.id);
  if (!act || !id) return;
  const rule = rules.find((r) => r.id === id);
  if (!rule) return;
  if (act === 'edit') {
    openRuleForm(rule);
  } else if (act === 'delete') {
    const ok = confirm(`删除规则「${rule.name}」？已打的标签保留。`);
    if (!ok) return;
    try {
      await deleteRule(id);
      toastRef('已删除规则');
      await refreshRules();
    } catch (err) {
      toastRef(String(err), false);
    }
  }
}

// ---------------------------------------------------------------- 用户 / 登录 / 审计

const ROLE_LABELS: Record<string, string> = {
  admin: 'admin（管理）',
  editor: 'editor（编辑）',
  viewer: 'viewer（只读）',
};

async function refreshUsers(): Promise<void> {
  try {
    const [session, users] = await Promise.all([sessionUser(), listUsers()]);
    const noUsers = users.length === 0;
    setupAdminForm.hidden = !noUsers;
    loginForm.hidden = session !== null || noUsers;
    changePwForm.hidden = session === null;
    createUserForm.hidden = noUsers;
    el<HTMLButtonElement>('user-logout').hidden = session === null;
    sessionInfo.textContent = session
      ? `当前会话：${session.username}（${session.role}）`
      : noUsers
        ? '尚未创建用户 — 请先创建初始管理员（Argon2）'
        : '未登录（本地模式，写操作以未登录身份入审计）';

    usersBody.innerHTML = '';
    for (const u of users) {
      const tr = document.createElement('tr');
      const tdId = document.createElement('td');
      tdId.textContent = String(u.id);
      const tdName = document.createElement('td');
      tdName.textContent = u.username;
      const tdRole = document.createElement('td');
      const roleSel = document.createElement('select');
      roleSel.dataset.act = 'role';
      roleSel.dataset.id = String(u.id);
      for (const [k, label] of Object.entries(ROLE_LABELS)) {
        const opt = document.createElement('option');
        opt.value = k;
        opt.textContent = label;
        roleSel.appendChild(opt);
      }
      roleSel.value = u.role;
      tdRole.appendChild(roleSel);
      const tdCreated = document.createElement('td');
      tdCreated.textContent = u.created_at;
      const tdOps = document.createElement('td');
      tdOps.className = 'ops';
      const del = document.createElement('button');
      del.type = 'button';
      del.className = 'btn-ghost';
      del.textContent = '删除';
      del.dataset.act = 'user-del';
      del.dataset.id = String(u.id);
      del.dataset.name = u.username;
      tdOps.appendChild(del);
      tr.append(tdId, tdName, tdRole, tdCreated, tdOps);
      usersBody.appendChild(tr);
    }
  } catch (err) {
    toastRef(String(err), false);
  }
}

async function refreshAudit(): Promise<void> {
  try {
    const rows: AuditRow[] = await listAudit(500, auditEntitySel.value || null);
    auditBody.innerHTML = '';
    if (rows.length === 0) {
      const tr = document.createElement('tr');
      const td = document.createElement('td');
      td.colSpan = 6;
      td.className = 'empty';
      td.textContent = '暂无审计记录 — 任何写操作都会出现在这里';
      tr.appendChild(td);
      auditBody.appendChild(tr);
      return;
    }
    for (const r of rows) {
      const tr = document.createElement('tr');
      const cells: [string, string][] = [
        [r.created_at, ''],
        [r.username ?? '（未登录）', ''],
        [r.action, ''],
        [r.entity, ''],
        [r.entity_id ?? '', ''],
        [shortJson(r.detail, 80), r.detail],
      ];
      for (const [text, title] of cells) {
        const td = document.createElement('td');
        td.textContent = text;
        if (title) td.title = title;
        tr.appendChild(td);
      }
      auditBody.appendChild(tr);
    }
  } catch (err) {
    toastRef(String(err), false);
  }
}

/// 切回设置页时刷新会话/用户/审计
export async function refreshCollabView(): Promise<void> {
  await refreshUsers();
  await refreshAudit();
}

function fmtBytes(n: number): string {
  if (n >= 1 << 30) return `${(n / (1 << 30)).toFixed(1)} GB`;
  if (n >= 1 << 20) return `${(n / (1 << 20)).toFixed(1)} MB`;
  if (n >= 1 << 10) return `${(n / (1 << 10)).toFixed(1)} KB`;
  return `${n} B`;
}

async function refreshDupes(): Promise<void> {
  const box = el<HTMLElement>('dupe-list');
  try {
    const groups = await listDupes();
    box.innerHTML = '';
    if (groups.length === 0) {
      const empty = document.createElement('p');
      empty.className = 'hint';
      empty.textContent = '没有可疑重复组——先点「补部分哈希」计算候选（或运行完整哈希后重查）。';
      box.append(empty);
      return;
    }
    for (const g of groups) {
      const grp = document.createElement('div');
      grp.className = 'dupe-group';
      const head = document.createElement('div');
      head.className = 'dupe-group-head';
      const badge = document.createElement('span');
      badge.className = g.confirmed ? 'dupe-badge confirmed' : 'dupe-badge';
      badge.textContent = g.confirmed ? '已确认重复' : '部分哈希相同 · 待完整哈希';
      const title = document.createElement('span');
      title.textContent = `${fmtBytes(g.size)} · ${g.count} 个 · ${g.partial_hash.slice(0, 12)}…`;
      head.append(badge, title);
      grp.append(head);
      for (const item of g.items) {
        const row = document.createElement('div');
        row.className = 'dupe-item';
        const p = document.createElement('span');
        p.className = 'dupe-path';
        p.textContent = item.rel_path;
        const st = document.createElement('span');
        st.className = 'dupe-state';
        st.textContent = item.hash_state;
        const btn = document.createElement('button');
        btn.type = 'button';
        btn.className = 'btn-ghost';
        btn.textContent = '定位';
        btn.addEventListener('click', () => {
          void listRoots()
            .then((roots) => {
              const root = roots.find((r) => r.id === item.root_id);
              if (!root) return;
              const sep = /[\\/]$/.test(root.path) ? '' : '/';
              void revealPath(`${root.path}${sep}${item.rel_path}`);
            })
            .catch((err) => toastRef(String(err), false));
        });
        row.append(p, st, btn);
        grp.append(row);
      }
      box.append(grp);
    }
  } catch (err) {
    box.innerHTML = '';
    const p = document.createElement('p');
    p.className = 'hint';
    p.textContent = `读取报告失败：${String(err)}`;
    box.append(p);
  }
}

async function refreshOcrCount(): Promise<void> {
  const badge = el<HTMLElement>('ocr-count');
  try {
    const n = await ocrPendingCount(null);
    badge.textContent = n > 0 ? `待识别 ${n}` : '无待识别';
  } catch {
    badge.textContent = '…';
  }
}

function refreshBackupLastBadge(): void {
  const last = Number(settings.get('last_backup_at') ?? 0);
  backupLastBadge.textContent = last > 0 ? `上次 ${new Date(last).toLocaleString()}` : '从未备份';
}

let backupTimer: number | null = null;
let backupBusy = false;

async function runBackup(silent: boolean): Promise<void> {
  if (backupBusy) return;
  backupBusy = true;
  try {
    const r = await backupNow();
    if (!silent) {
      const resticNote = r.restic
        ? `；restic：${r.restic}`
        : r.restic_skipped
          ? `（${r.restic_skipped}）`
          : '';
      toastRef(`已快照：${r.snapshot_path}${resticNote}`);
    } else if (r.restic_skipped?.includes('失败')) {
      toastRef(`定时快照已完成，但 ${r.restic_skipped}`, false);
    }
    await refreshSettings();
  } catch (err) {
    toastRef(String(err), false);
  } finally {
    backupBusy = false;
  }
}

function applyBackupSchedule(): void {
  if (backupTimer !== null) {
    clearInterval(backupTimer);
    backupTimer = null;
  }
  const hours = Number(settings.get('backup_interval_hours') ?? 0);
  if (!(hours > 0)) return;
  const due = (): boolean =>
    Date.now() - Number(settings.get('last_backup_at') ?? 0) >= hours * 3600_000;
  // 到点即备份；每 15 分钟巡检一次（应用打开时生效）
  if (due()) void runBackup(true);
  backupTimer = window.setInterval(() => {
    if (due()) void runBackup(true);
  }, 15 * 60_000);
}

export async function initSettings(): Promise<void> {
  await refreshSettings();

  rootForm.addEventListener('submit', async (ev) => {
    ev.preventDefault();
    let path = rootPathInput.value.trim();
    if (!path) {
      path = (await pickDir('选择媒体根目录')) || '';
      if (!path) return;
      rootPathInput.value = path;
    }
    try {
      await addRoot(path, rootLabelInput.value.trim() || null);
      toastRef('已添加根目录');
      rootPathInput.value = '';
      rootLabelInput.value = '';
      await refreshRoots();
      window.dispatchEvent(new CustomEvent('vc:roots-changed'));
    } catch (err) {
      toastRef(String(err), false);
    }
  });

  el<HTMLButtonElement>('root-browse').addEventListener('click', () => {
    void pickDir('选择媒体根目录').then((dir) => {
      if (dir) rootPathInput.value = dir;
    });
  });

  rootsBody.addEventListener('click', (ev) => {
    void onRootAction(ev);
  });

  derivedForm.addEventListener('submit', async (ev) => {
    ev.preventDefault();
    try {
      await saveSettings([{ key: 'derived_dir', value: derivedInput.value.trim() }]);
      toastRef('已保存派生数据目录');
      await refreshSettings();
    } catch (err) {
      toastRef(String(err), false);
    }
  });

  el<HTMLButtonElement>('derived-browse').addEventListener('click', () => {
    void pickDir('选择派生数据目录').then((dir) => {
      if (dir) derivedInput.value = dir;
    });
  });

  el<HTMLButtonElement>('reprobe').addEventListener('click', () => {
    void probeTools()
      .then((r) => renderTools(r.tools))
      .catch((err) => toastRef(String(err), false));
  });

  async function startMeta(source: string, label: string): Promise<void> {
    window.dispatchEvent(new CustomEvent('vc:begin-job', { detail: label }));
    try {
      await startMetaJob(metaScope.value ? Number(metaScope.value) : null, source);
    } catch (err) {
      toastRef(String(err), false);
      window.dispatchEvent(new CustomEvent('vc:end-job', { detail: `${label}启动失败` }));
    }
  }

  el<HTMLButtonElement>('meta-probe').addEventListener('click', () => {
    void startMeta('probe', '探测');
  });
  el<HTMLButtonElement>('meta-beets').addEventListener('click', () => {
    void startMeta('beets', 'beets 导入');
  });
  el<HTMLButtonElement>('meta-calibre').addEventListener('click', () => {
    void startMeta('calibre', 'Calibre 导入');
  });

  el<HTMLFormElement>('meta-lib-form').addEventListener('submit', async (ev) => {
    ev.preventDefault();
    try {
      await saveSettings([
        { key: 'beets_db_path', value: beetsPathInput.value.trim() },
        { key: 'calibre_library_path', value: calibrePathInput.value.trim() },
      ]);
      toastRef('已保存库路径');
      await refreshSettings();
    } catch (err) {
      toastRef(String(err), false);
    }
  });

  el<HTMLButtonElement>('beets-browse').addEventListener('click', () => {
    void open({ directory: false, multiple: false, title: '选择 beets library.db' }).then((sel) => {
      if (typeof sel === 'string') beetsPathInput.value = sel;
    });
  });

  el<HTMLButtonElement>('calibre-browse').addEventListener('click', () => {
    void pickDir('选择 Calibre 书库目录').then((dir) => {
      if (dir) calibrePathInput.value = dir;
    });
  });

  // 规则自动打标
  el<HTMLButtonElement>('rule-new').addEventListener('click', () => openRuleForm(null));
  el<HTMLButtonElement>('rule-cancel').addEventListener('click', () => {
    ruleForm.hidden = true;
  });
  ruleForm.addEventListener('submit', async (ev) => {
    ev.preventDefault();
    const id = ruleIdInput.value ? Number(ruleIdInput.value) : null;
    try {
      await saveRule(
        id,
        ruleNameInput.value,
        ruleEnabledInput.checked,
        Number(rulePriorityInput.value) || 0,
        ruleConditionsInput.value,
        ruleActionsInput.value,
      );
      toastRef(id ? '规则已更新' : '规则已创建');
      ruleForm.hidden = true;
      await refreshRules();
    } catch (err) {
      toastRef(String(err), false);
    }
  });
  rulesBody.addEventListener('click', (ev) => {
    void onRuleRowAction(ev);
  });
  el<HTMLButtonElement>('rule-run').addEventListener('click', () => {
    window.dispatchEvent(new CustomEvent('vc:begin-job', { detail: '规则' }));
    void startRuleJob(null).catch((err) => {
      toastRef(String(err), false);
      window.dispatchEvent(new CustomEvent('vc:end-job', { detail: '规则启动失败' }));
    });
  });

  // 用户与登录 / 审计
  setupAdminForm.addEventListener('submit', async (ev) => {
    ev.preventDefault();
    try {
      const u = await setupAdmin(
        el<HTMLInputElement>('setup-admin-name').value.trim(),
        el<HTMLInputElement>('setup-admin-pw').value,
      );
      el<HTMLInputElement>('setup-admin-pw').value = '';
      toastRef(`已创建管理员 ${u.username} 并登录`);
      window.dispatchEvent(new CustomEvent('vc:user-changed'));
      await refreshCollabView();
    } catch (err) {
      toastRef(String(err), false);
    }
  });
  loginForm.addEventListener('submit', async (ev) => {
    ev.preventDefault();
    try {
      const u = await login(
        el<HTMLInputElement>('login-name').value.trim(),
        el<HTMLInputElement>('login-pw').value,
      );
      el<HTMLInputElement>('login-pw').value = '';
      toastRef(`已登录：${u.username}（${u.role}）`);
      window.dispatchEvent(new CustomEvent('vc:user-changed'));
      await refreshCollabView();
    } catch (err) {
      toastRef(String(err), false);
      await refreshAudit();
    }
  });
  el<HTMLButtonElement>('user-logout').addEventListener('click', () => {
    void logout()
      .then(() => {
        toastRef('已退出登录');
        window.dispatchEvent(new CustomEvent('vc:user-changed'));
        return refreshCollabView();
      })
      .catch((err) => toastRef(String(err), false));
  });
  changePwForm.addEventListener('submit', async (ev) => {
    ev.preventDefault();
    try {
      await changePassword(
        el<HTMLInputElement>('old-pw').value,
        el<HTMLInputElement>('new-pw').value,
      );
      el<HTMLInputElement>('old-pw').value = '';
      el<HTMLInputElement>('new-pw').value = '';
      toastRef('密码已修改');
      await refreshAudit();
    } catch (err) {
      toastRef(String(err), false);
    }
  });
  createUserForm.addEventListener('submit', async (ev) => {
    ev.preventDefault();
    try {
      const name = el<HTMLInputElement>('new-user-name').value.trim();
      const role = el<HTMLSelectElement>('new-user-role').value;
      await createUser(name, el<HTMLInputElement>('new-user-pw').value, role);
      el<HTMLInputElement>('new-user-name').value = '';
      el<HTMLInputElement>('new-user-pw').value = '';
      toastRef(`已创建用户 ${name}`);
      await refreshCollabView();
    } catch (err) {
      toastRef(String(err), false);
    }
  });
  usersBody.addEventListener('click', (ev) => {
    const btn = ev.target as HTMLButtonElement;
    if (btn.dataset.act !== 'user-del') return;
    const id = Number(btn.dataset.id);
    const name = btn.dataset.name;
    if (!confirm(`删除用户 ${name}？历史审计记录会保留（user_id 置空）。`)) return;
    void deleteUser(id)
      .then(() => {
        toastRef(`已删除用户 ${name}`);
        return refreshCollabView();
      })
      .catch((err) => toastRef(String(err), false));
  });
  usersBody.addEventListener('change', (ev) => {
    const sel = ev.target as HTMLSelectElement;
    if (sel.dataset.act !== 'role') return;
    const id = Number(sel.dataset.id);
    void setUserRole(id, sel.value)
      .then(() => {
        toastRef('角色已更新');
        return refreshCollabView();
      })
      .catch((err) => {
        toastRef(String(err), false);
        void refreshUsers();
      });
  });
  auditEntitySel.addEventListener('change', () => {
    void refreshAudit();
  });
  el<HTMLButtonElement>('audit-refresh').addEventListener('click', () => {
    void refreshAudit();
  });

  // 重复检测
  el<HTMLButtonElement>('dupe-run').addEventListener('click', () => {
    window.dispatchEvent(new CustomEvent('vc:begin-job', { detail: '重复检测' }));
    void startDupeJob().catch((err) => {
      toastRef(String(err), false);
      window.dispatchEvent(new CustomEvent('vc:end-job', { detail: '重复检测启动失败' }));
    });
  });
  el<HTMLButtonElement>('dupe-refresh').addEventListener('click', () => {
    void refreshDupes();
  });
  window.addEventListener('vc:dupes-done', () => {
    void refreshDupes();
  });

  // OCR 队列
  el<HTMLButtonElement>('ocr-run').addEventListener('click', () => {
    window.dispatchEvent(new CustomEvent('vc:begin-job', { detail: 'OCR 队列' }));
    void startOcrJob(null).catch((err) => {
      toastRef(String(err), false);
      window.dispatchEvent(new CustomEvent('vc:end-job', { detail: 'OCR 启动失败' }));
    });
  });
  el<HTMLButtonElement>('ocr-refresh').addEventListener('click', () => {
    void refreshOcrCount();
  });
  window.addEventListener('vc:ocr-done', () => {
    void refreshOcrCount();
  });

  // 自动分类
  el<HTMLFormElement>('autocat-form').addEventListener('submit', async (ev) => {
    ev.preventDefault();
    const v = String(Math.min(5, Math.max(0, Math.floor(Number(autocatDepthInput.value) || 0))));
    autocatDepthInput.value = v;
    try {
      await saveSettings([{ key: 'auto_cat_depth', value: v }]);
      settings.set('auto_cat_depth', v);
      toastRef('已保存自动分类层级');
    } catch (err) {
      toastRef(String(err), false);
    }
  });
  el<HTMLButtonElement>('autocat-run').addEventListener('click', () => {
    window.dispatchEvent(new CustomEvent('vc:begin-job', { detail: '自动分类' }));
    void startAutocatJob(null).catch((err) => {
      toastRef(String(err), false);
      window.dispatchEvent(new CustomEvent('vc:end-job', { detail: '自动分类启动失败' }));
    });
  });

  // 备份
  backupForm.addEventListener('submit', async (ev) => {
    ev.preventDefault();
    try {
      await saveSettings([
        { key: 'backup_interval_hours', value: backupIntervalInput.value.trim() || '0' },
        { key: 'backup_keep', value: backupKeepInput.value.trim() || '7' },
        { key: 'backup_dir', value: backupDirInput.value.trim() },
        { key: 'restic_repo', value: resticRepoInput.value.trim() },
        { key: 'restic_password_file', value: resticPwInput.value.trim() },
      ]);
      toastRef('已保存备份设置');
      await refreshSettings();
    } catch (err) {
      toastRef(String(err), false);
    }
  });
  el<HTMLButtonElement>('backup-run').addEventListener('click', () => {
    void runBackup(false);
  });

  // 标签 / 分类导入导出
  el<HTMLButtonElement>('port-export').addEventListener('click', () => {
    void save({
      title: '导出标签 / 分类',
      defaultPath: 'vinecurator-tags-categories.json',
      filters: [{ name: 'JSON', extensions: ['json'] }],
    })
      .then(async (path) => {
        if (!path) return;
        const n = await exportPort(path);
        toastRef(`已导出 ${n} 项到 ${path}`);
      })
      .catch((err) => toastRef(String(err), false));
  });
  el<HTMLButtonElement>('port-import').addEventListener('click', () => {
    void open({
      title: '导入标签 / 分类',
      multiple: false,
      filters: [{ name: 'JSON', extensions: ['json'] }],
    })
      .then(async (sel) => {
        if (typeof sel !== 'string') return;
        const rep = await importPort(sel);
        const skip = rep.skipped.length
          ? `；跳过 ${rep.skipped.length}：${rep.skipped.slice(0, 3).join('、')}`
          : '';
        toastRef(
          `导入完成：分类 +${rep.categories_created}/更新 ${rep.categories_updated}、标签 +${rep.tags_created}、别名 +${rep.aliases_added}${skip}`,
          rep.skipped.length === 0,
        );
        window.dispatchEvent(new CustomEvent('vc:roots-changed'));
      })
      .catch((err) => toastRef(String(err), false));
  });

  const report = await probeTools();
  renderTools(report.tools);
  await refreshRules();
  await refreshCollabView();
  await refreshDupes();
  await refreshOcrCount();
}

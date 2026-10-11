import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import {
  autoFillProposals,
  cancelJob,
  checkNames,
  copyItems,
  coverImage,
  fixNames,
  listDir,
  loadProject,
  makeDirectory,
  moveItems,
  nameProfiles,
  noteDiff,
  onDupesProgress,
  onExportProgress,
  onNoteAuditProgress,
  onNotesProgress,
  onVerifyProgress,
  openPath,
  preflightExport,
  previewNote,
  probeTools,
  quitApp,
  renamePath,
  revealPath,
  saveProject,
  scanDirectory,
  startExport,
  startFindDuplicates,
  startGenerateNotes,
  startNoteAudit,
  startVerify,
  trashItems,
  type AuditProgress,
  type DetectNode,
  type DupGroup,
  type DupesProgress,
  type Entry,
  type ExportRequest,
  type FieldDiff,
  type NameCheckResult,
  type NoteAuditItem,
  type Overrides,
  type PasswordProfile,
  type PreflightIssue,
  type Project,
  type Proposal,
  type ToolInfo,
  type VerifyProgress,
  type VerifyReport,
} from './api';
import { initSplitters, paneVisible, togglePane } from './layout';
import { initMenubar, setMenuBusy, setRecentProjects, type MenuActions } from './menu';
import { getPrefs, initPrefs, openSettings } from './settings';
import {
  clearEntryFilters,
  emptyProject,
  ensureOverrides,
  filtersActive,
  findEntry,
  findNode,
  formatSize,
  KINDS,
  kindLabel,
  newState,
  searchHaystack,
  selectedEntries,
  sortEntries,
  unitsUnder,
  type AppState,
  type SortState,
} from './state';
import {
  autofillFieldsHtml,
  autofillScope,
  checkedAutofillFields,
  clearLog,
  closeModal,
  el,
  esc,
  exportStatus,
  log,
  notesStatus,
  openModal,
  openProposalModal,
  openPrompt,
  readArchiveForm,
  renderDetail,
  renderFileList,
  renderPasswords,
  renderStats,
  renderTable,
  renderTree,
  renderTreeOps,
  setProgress,
  setProgressText,
  syncArchiveForm,
  syncFilterOptions,
  toast,
  updateFormatAvailability,
  visibleEntries,
} from './ui';

const state: AppState = newState();
let tools: ToolInfo | null = null;
/** 文件面板 Shift 范围选的锚点 */
let lastFileIndex: number | null = null;
/** 表格多选锚点（Ctrl/Shift 点选与 Shift 范围勾选）；存 rel_path——排序/筛选后索引会漂移 */
let lastRowRel: string | null = null;
/** 说明文件批量核对：本次结果、范围、进度弹窗是否打开 */
let auditItems: NoteAuditItem[] = [];
let auditScope = '';
let auditCancelled = false;
let auditOpen = false;
/** 查找重复文件：本次分组结果、进度弹窗是否打开 */
let dupeGroups: DupGroup[] = [];
let dupOpen = false;
/** 导出清单校验：本次报告、进度弹窗是否打开 */
let verifyReport: VerifyReport | null = null;
let verifyCancelled = false;
let verifyOpen = false;
/** 封面缩略图缓存：key = root\0rel（null = 无封面），在途请求去重 */
const coverCache = new Map<string, string | null>();
const coverPending = new Map<string, Promise<string | null>>();

// 主题/缩放/动效在首帧前套用（跟随系统时监听系统深浅色切换）
initPrefs();

// ---------------------------------------------------------------- 通用

function render(): void {
  renderTree(state);
  renderTreeOps(state);
  renderTable(state);
  syncFilterOptions(state);
  refreshDetail(state);
  renderStats(state);
  renderPasswords(state);
  renderFileList(state);
}

/** 过滤条件归零并同步工具栏输入（新建/打开项目时调用） */
function resetEntryFilterUi(): void {
  clearEntryFilters(state.filters);
  state.filters.treeQuery = '';
  el<HTMLInputElement>('entry-filter').value = '';
  el<HTMLSelectElement>('kind-filter').value = '';
  el<HTMLSelectElement>('status-filter').value = '';
  el<HTMLInputElement>('tree-filter').value = '';
  syncAdvancedInputs();
  updateFilterCount();
}

function fetchCover(root: string, rel: string): Promise<string | null> {
  const key = `${root}\u0000${rel}`;
  if (coverCache.has(key)) return Promise.resolve(coverCache.get(key)!);
  let pending = coverPending.get(key);
  if (!pending) {
    pending = coverImage(root, rel)
      .then((url) => {
        coverCache.set(key, url);
        coverPending.delete(key);
        return url;
      })
      .catch((err) => {
        coverPending.delete(key);
        throw err;
      });
    coverPending.set(key, pending);
  }
  return pending;
}

function loadCover(s: AppState): void {
  const rel = s.selectedRel;
  if (!rel || !s.project) return;
  if (!document.getElementById('detail-cover-box')) return;
  const root = s.project.root;
  const show = (url: string | null): void => {
    const host = document.getElementById('detail-cover-box');
    if (!host || state.selectedRel !== rel) return;
    host.innerHTML = url
      ? `<img class="detail-cover" src="${esc(url)}" alt="封面" />`
      : '<div class="detail-cover-empty">无封面</div>';
  };
  fetchCover(root, rel).then(show).catch(() => show(null));
}

function refreshDetail(s: AppState): void {
  renderDetail(s);
  loadCover(s);
}

// ---------------------------------------------------------------- 路径工具

function rootPath(): string {
  return project().root.replace(/[\\/]+$/, '');
}

function joinRoot(rel: string): string {
  const root = rootPath();
  const r = rel.replace(/\//g, '\\').replace(/^\\+/, '');
  return root ? `${root}\\${r}` : r;
}

function relOf(abs: string): string {
  const root = rootPath().toLowerCase();
  const a = abs.replace(/\//g, '\\').replace(/\\+$/, '');
  if (root && a.toLowerCase().startsWith(root)) {
    return a.slice(rootPath().length).replace(/^\\+/, '').replace(/\\/g, '/');
  }
  return '';
}

function parentDir(abs: string): string | null {
  const root = rootPath();
  const a = abs.replace(/\//g, '\\').replace(/\\+$/, '');
  if (!root || a.toLowerCase() === root.toLowerCase()) return null;
  const i = a.lastIndexOf('\\');
  if (i < 0) return null;
  const parent = a.slice(0, i);
  return parent.toLowerCase().startsWith(root.toLowerCase()) ? parent : root;
}

function project(): Project {
  if (!state.project) state.project = emptyProject();
  return state.project;
}

function setBusy(kind: AppState['busy']): void {
  state.busy = kind;
  // 统一由 data-busy-guard 列表驱动：任务期间禁用全部编辑入口
  document
    .querySelectorAll<HTMLButtonElement>('[data-busy-guard]')
    .forEach((b) => (b.disabled = kind !== null));
  setMenuBusy(kind !== null);
  // 忙碌且尚无百分比进度时（setProgress 会撤掉），状态栏显示不确定进度动画
  el('progress-track').classList.toggle('indeterminate', kind !== null);
  if (kind === null) renderTreeOps(state); // 树操作按钮另有启用条件
  el<HTMLButtonElement>('notes-btn').disabled = kind !== null;
  el<HTMLButtonElement>('export-btn').disabled = kind !== null;
  el<HTMLButtonElement>('scan-btn').disabled = kind !== null;
  el<HTMLButtonElement>('export-cancel-btn').disabled = kind === null;
}

/** 写操作入口守卫：任务进行中拒绝新的磁盘写入（data-busy-guard 之外的动态按钮兜底） */
function busyGuard(): boolean {
  if (state.busy) {
    toast('有任务正在进行，请等它完成或先取消');
    return true;
  }
  return false;
}

function rememberPaths(p: Project): void {
  localStorage.setItem('vinepress.root', p.root);
  localStorage.setItem('vinepress.output', p.output_root);
}

function mergeEntries(p: Project, fresh: Entry[]): void {
  const byRel = new Map(p.entries.map((e) => [e.rel_path, e]));
  p.entries = fresh.map((f) => {
    const prev = byRel.get(f.rel_path);
    if (!prev) return f;
    prev.stats = f.stats;
    prev.has_notes = f.has_notes;
    prev.name = f.name;
    return prev;
  });
}

// ---------------------------------------------------------------- 扫描

async function doScan(): Promise<void> {
  const root = el<HTMLInputElement>('root-path').value.trim();
  if (!root) {
    toast('请先填写根目录');
    return;
  }
  if (busyGuard()) return;
  const p = project();
  p.root = root;
  if (!p.output_root) {
    const guess = localStorage.getItem('vinepress.output');
    if (guess) {
      p.output_root = guess;
      el<HTMLInputElement>('output-path').value = guess;
    }
  }

  setBusy('scan');
  setProgressText('扫描中：识别目录与单位…');
  try {
    log(`扫描：${root}`);
    const res = await scanDirectory(root, ensureOverrides(p));
    const prevTop = new Set(state.tree.map((n) => n.rel_path));
    state.tree = res.tree;
    state.scan = {
      container_count: res.container_count,
      ignored_count: res.ignored_count,
      total_size: res.total_size,
      total_files: res.total_files,
      root_loose: res.root_loose,
    };
    // 重扫保持已展开的节点；新增的顶层节点默认展开
    const kept = new Set<string>();
    const walk = (nodes: DetectNode[]): void => {
      for (const n of nodes) {
        if (!n.children.length) continue;
        if (state.expanded.has(n.rel_path)) kept.add(n.rel_path);
        else if (n.rel_path.split('/').length === 1 && !prevTop.has(n.rel_path)) kept.add(n.rel_path);
        walk(n.children);
      }
    };
    walk(state.tree);
    state.expanded = kept;
    mergeEntries(p, res.entries);
    markDirty();
    if (state.selectedRel && !findEntry(p, state.selectedRel)) {
      state.selectedRel = null;
    }
    if (state.selectedNodeRel && !findNode(state.tree, state.selectedNodeRel)) {
      state.selectedNodeRel = null;
    }
    render();
    setProgress(0, `识别完成 · 单位 ${p.entries.length}`);
    log(
      `识别完成：单位 ${p.entries.length} · 容器 ${res.container_count} · 忽略 ${res.ignored_count}`,
      'ok',
    );
    toast(`识别到 ${p.entries.length} 个单位`);
    if (res.root_loose.length) {
      const shown = res.root_loose.slice(0, 5).join('、');
      log(
        `根目录散件未纳入单位（${res.root_loose.length}）：${res.root_loose.join('、')}`,
      );
      toast(
        `根目录有 ${res.root_loose.length} 个散件未纳入单位：${shown}${res.root_loose.length > 5 ? ' …' : ''}`,
        6000,
      );
    }
    rememberPaths(p);
    if (!state.cwd || !state.cwd.toLowerCase().startsWith(root.toLowerCase())) {
      state.cwd = root;
      state.fileSelection.clear();
      void refreshFiles();
    }
  } catch (err) {
    setProgress(0, '扫描失败');
    log(`扫描失败：${String(err)}`, 'error');
    toast(`扫描失败：${String(err)}`, 4000);
  } finally {
    setBusy(null);
  }
}

// ---------------------------------------------------------------- 手动覆盖（合并 / 拆分）

/** 修改 overrides 后统一重扫；条目元数据按 rel_path 保留 */
function applyOverrideEdit(label: string, edit: (ov: Overrides) => void): void {
  if (busyGuard()) return;
  const ov = ensureOverrides(project());
  edit(ov);
  log(label, 'ok');
  renderTreeOps(state);
  void doScan();
}

/** 清除 prefix 及其全部子孙上的覆盖（整段变成单位/容器后它们不可达） */
function dropOverridesUnder(ov: Overrides, prefix: string): void {
  const keep = (p: string): boolean => p !== prefix && !p.startsWith(`${prefix}/`);
  ov.force_units = ov.force_units.filter(keep);
  ov.force_containers = ov.force_containers.filter(keep);
}

function forceUnit(rel: string): void {
  applyOverrideEdit(`强制为单位：${rel}`, (ov) => {
    dropOverridesUnder(ov, rel);
    ov.force_units.push(rel);
  });
}

function forceContainer(rel: string): void {
  const doSplit = () =>
    applyOverrideEdit(`强制为容器（拆分）：${rel}`, (ov) => {
      ov.force_units = ov.force_units.filter((p) => p !== rel);
      ov.force_containers = ov.force_containers.filter((p) => p !== rel);
      ov.force_containers.push(rel);
    });
  // 直属内容文件（散件）在拆分后不属于任何子单位 → 先警示再执行
  const direct = findEntry(state.project, rel)?.stats.direct_files ?? [];
  if (!direct.length) {
    doSplit();
    return;
  }
  openModal(
    '拆分为子目录',
    `<p>将把 <b>${esc(rel)}</b> 拆分为容器：其下子目录各自成为独立条目，说明文件一目录一文件。</p>
     <p class="hint">该目录直属 ${direct.length} 个内容文件：<b>${esc(direct.slice(0, 8).join('、'))}${
       direct.length > 8 ? ' …' : ''
     }</b></p>
     <p class="hint">拆分后这些散件<strong>不属于任何子单位</strong>——不出现在条目表，也<strong>不会被打包导出</strong>。建议先在文件面板把它们移入某个子目录，或保持整体不拆。</p>`,
    [
      { label: '取消', onClick: (close) => close() },
      {
        label: '仍要拆分',
        primary: true,
        onClick: (close) => {
          close();
          doSplit();
        },
      },
    ],
  );
}

function restoreAuto(rel: string): void {
  applyOverrideEdit(`恢复自动识别：${rel}`, (ov) => {
    ov.force_units = ov.force_units.filter((p) => p !== rel);
    ov.force_containers = ov.force_containers.filter((p) => p !== rel);
  });
}

function confirmMergeUp(rel: string): void {
  const parent = rel.split('/').slice(0, -1).join('/');
  if (!parent) {
    toast('已在根目录下，没有可并入的上级');
    return;
  }
  const units = unitsUnder(state.tree, parent);
  const names = units
    .map((u) => u.split('/').pop() ?? u)
    .slice(0, 12)
    .join('、');
  const more = units.length > 12 ? ` …等 ${units.length} 个` : '';
  openModal(
    '并入上级',
    `<p>将把 <b>${esc(parent)}</b> 整体作为一个压缩单位。</p>
     <p class="hint">其下 ${units.length} 个单位会被一并吸收：${esc(names)}${more}</p>
     <p class="hint">想撤销时选中该目录，点「恢复自动识别」。</p>`,
    [
      { label: '取消', onClick: (close) => close() },
      {
        label: '确认并入',
        primary: true,
        onClick: (close) => {
          close();
          applyOverrideEdit(`并入上级：${rel} → ${parent}`, (ov) => {
            dropOverridesUnder(ov, parent);
            ov.force_units.push(parent);
          });
          state.selectedNodeRel = parent;
          renderTreeOps(state);
        },
      },
    ],
  );
}

// ---------------------------------------------------------------- 重命名（磁盘）

/** 改写 rel 前缀匹配的路径（overrides / 选中态 / 当前目录） */
function shiftRel(list: string[], oldRel: string, newRel: string): string[] {
  if (!oldRel) return list;
  return list.map((p) => {
    if (p === oldRel) return newRel;
    if (p.startsWith(`${oldRel}/`)) return newRel ? newRel + p.slice(oldRel.length) : p;
    return p;
  });
}

function shiftRelValue(value: string | null, oldRel: string, newRel: string): string | null {
  if (!value || !oldRel) return value;
  if (value === oldRel) return newRel;
  if (value.startsWith(`${oldRel}/`)) return newRel ? newRel + value.slice(oldRel.length) : value;
  return value;
}

/** 改写 rel 前缀匹配的项目状态（overrides / 选中态 / 当前目录） */
function applyRelShift(oldRel: string, newRel: string): void {
  const p = project();
  const ov = ensureOverrides(p);
  ov.force_units = shiftRel(ov.force_units, oldRel, newRel);
  ov.force_containers = shiftRel(ov.force_containers, oldRel, newRel);
  state.selectedRel = shiftRelValue(state.selectedRel, oldRel, newRel);
  state.selectedNodeRel = shiftRelValue(state.selectedNodeRel, oldRel, newRel);
  if (state.cwd) {
    const cwdRel = relOf(state.cwd);
    const moved = shiftRelValue(cwdRel, oldRel, newRel);
    if (moved !== cwdRel && moved !== null) state.cwd = joinRoot(moved);
  }
}

async function renameTarget(absPath: string, oldName: string): Promise<void> {
  const p = project();
  if (!p.root) {
    toast('请先扫描根目录');
    return;
  }
  if (busyGuard()) return;
  openPrompt('重命名', '新名称', oldName, oldName, async (newName) => {
    if (newName === oldName) return;
    try {
      const oldRel = relOf(absPath);
      const outcome = await renamePath(
        p.root,
        p.entries.map((e) => ({ ...e })),
        absPath,
        newName,
      );
      p.entries = outcome.entries;
      const newRel = outcome.new_rel_path;
      markDirty();
      applyRelShift(oldRel, newRel);

      log(`重命名：${oldRel || absPath} → ${newRel || newName}`, 'ok');
      if (oldRel) await doScan();
      await refreshFiles();
      toast(`已重命名为「${newName}」`);
    } catch (err) {
      toast(String(err), 4000);
      log(`重命名失败：${String(err)}`, 'error');
    }
  });
}

// ---------------------------------------------------------------- 说明预览 / 回收站

async function previewCurrentNote(): Promise<void> {
  const p = project();
  const entry = findEntry(state.project, state.selectedRel);
  if (!entry) return;
  if (!p.root) {
    toast('请先扫描根目录');
    return;
  }
  try {
    const text = await previewNote(p.root, { ...entry });
    openModal(
      `说明文件预览 · ${entry.name}`,
      `<pre class="note-preview">${esc(text)}</pre>`,
      [{ label: '关闭', onClick: (close) => close() }],
      true,
    );
  } catch (err) {
    toast(String(err), 4000);
  }
}

// ---------------------------------------------------------------- 说明文件字段级对比

const DIFF_STATUS_LABEL: Record<FieldDiff['status'], string> = {
  added: '新增',
  removed: '移除',
  changed: '变更',
  same: '未变',
};

function diffRowsHtml(fields: FieldDiff[], showSame: boolean): string {
  const cell = (v: string): string =>
    v ? `<div class="diff-val">${esc(v)}</div>` : '<span class="hint">—</span>';
  return fields
    .filter((f) => showSame || f.status !== 'same')
    .map(
      (f) => `<tr class="diff-${f.status}">
        <td class="diff-field">${esc(f.field)}</td>
        <td>${cell(f.old)}</td>
        <td>${cell(f.new)}</td>
        <td><span class="diff-badge ${f.status}">${DIFF_STATUS_LABEL[f.status]}</span></td>
      </tr>`,
    )
    .join('');
}

async function openNoteDiff(back?: () => void): Promise<void> {
  const p = project();
  const entry = findEntry(state.project, state.selectedRel);
  if (!entry) {
    toast('请先在表格中选择一个条目');
    return;
  }
  if (!p.root) {
    toast('请先扫描根目录');
    return;
  }
  try {
    const res = await noteDiff(p.root, { ...entry });
    const fields = res.fields;
    const n = (s: FieldDiff['status']): number => fields.filter((f) => f.status === s).length;
    const changed = n('changed') + n('added') + n('removed');
    const summary =
      `变更 ${n('changed')} · 新增 ${n('added')} · 移除 ${n('removed')} · 未变 ${n('same')}`;
    const actions: { label: string; onClick: (close: () => void) => void }[] = [];
    if (back) actions.push({ label: '← 返回核对清单', onClick: () => back() });
    actions.push({ label: '关闭', onClick: (close) => close() });
    openModal(
      `说明文件对比 · ${entry.name}`,
      `<p class="hint">${res.existing ? '' : '尚无现有说明文件（重新生成将全部新建）。<br>'}${summary}</p>
       <label class="check-row"><input type="checkbox" id="diff-show-same" ${n('same') && changed === 0 ? 'checked' : ''} /><span>显示未变化字段</span></label>
       <div class="diff-wrap"><table class="diff-table">
         <thead><tr><th>字段</th><th>现有</th><th>将生成</th><th>状态</th></tr></thead>
         <tbody id="diff-tbody">${diffRowsHtml(fields, n('same') > 0 && changed === 0)}</tbody>
       </table></div>
       <p class="hint">按「生成说明文件」后，磁盘上的说明文件将与「将生成」列一致。</p>`,
      actions,
      true,
    );
    el<HTMLInputElement>('diff-show-same').addEventListener('change', (e) => {
      const show = (e.target as HTMLInputElement).checked;
      el('diff-tbody').innerHTML = diffRowsHtml(fields, show);
    });
    log(`说明文件对比：${entry.name} — ${summary}`);
  } catch (err) {
    toast(String(err), 4000);
  }
}

// ---------------------------------------------------------------- 说明文件批量核对

type AuditStatus = 'error' | 'missing-note' | 'missing-fields' | 'changed' | 'stale' | 'same';

const AUDIT_LABEL: Record<AuditStatus, string> = {
  error: '错误',
  'missing-note': '缺说明',
  'missing-fields': '缺字段',
  changed: '有变更',
  stale: '多余字段',
  same: '一致',
};

/** 单条核对状态（优先级：错误 > 缺说明 > 缺字段 > 变更 > 多余 > 一致） */
function auditStatus(it: NoteAuditItem): AuditStatus {
  if (it.error) return 'error';
  if (!it.exists) return 'missing-note';
  if (it.missing.length) return 'missing-fields';
  if (it.changed) return 'changed';
  if (it.removed) return 'stale';
  return 'same';
}

function auditCounts(): {
  same: number;
  missingNote: number;
  missingFields: number;
  changed: number;
  stale: number;
  error: number;
} {
  const c = { same: 0, missingNote: 0, missingFields: 0, changed: 0, stale: 0, error: 0 };
  for (const it of auditItems) {
    switch (auditStatus(it)) {
      case 'error':
        c.error++;
        break;
      case 'missing-note':
        c.missingNote++;
        break;
      case 'missing-fields':
        c.missingFields++;
        break;
      case 'changed':
        c.changed++;
        break;
      case 'stale':
        c.stale++;
        break;
      default:
        c.same++;
    }
  }
  return c;
}

function auditRowsHtml(): string {
  return auditItems
    .map((it) => {
      const s = auditStatus(it);
      const missing = it.error
        ? esc(it.error)
        : !it.exists
          ? '尚未生成'
          : it.missing.length
            ? esc(it.missing.join('、'))
            : '—';
      const changes =
        it.error || !it.exists
          ? '—'
          : [it.changed ? `变更 ${it.changed}` : '', it.removed ? `多余 ${it.removed}` : '']
              .filter(Boolean)
              .join(' · ') || '—';
      return `<tr data-rel="${esc(it.rel_path)}" class="audit-row">
        <td class="audit-name" title="${esc(it.rel_path)}">${esc(it.name)}</td>
        <td><span class="audit-badge audit-${s}">${AUDIT_LABEL[s]}</span></td>
        <td class="audit-missing" title="${esc(it.missing.join('、'))}">${missing}</td>
        <td class="audit-changes">${changes}</td>
      </tr>`;
    })
    .join('');
}

function openAuditProgressModal(total: number): void {
  auditOpen = true;
  openModal(
    `核对说明文件 · ${auditScope}`,
    `<p class="hint">按当前条目重新渲染，与磁盘上的 &lt;单位名&gt;.txt 逐字段比对（只读，不写盘）。</p>
     <div class="audit-progress">
       <div class="audit-bar"><div id="audit-bar" style="width:0%"></div></div>
       <span class="hint"><span id="audit-idx">0</span> / ${total}</span>
     </div>
     <div class="hint" id="audit-cur">准备中…</div>`,
    [
      {
        label: '取消',
        onClick: async () => {
          await cancelJob().catch(() => {});
        },
      },
    ],
    false,
    () => {
      auditOpen = false;
    },
  );
}

function openAuditResultModal(): void {
  const counts = auditCounts();
  auditOpen = true;
  openModal(
    `核对结果 · ${auditScope}${auditCancelled ? ' · 已取消' : ''}`,
    `<p class="hint">共 ${auditItems.length} · 一致 ${counts.same} · 缺说明 ${counts.missingNote} · 缺字段 ${counts.missingFields} · 有变更 ${counts.changed} · 多余字段 ${counts.stale} · 错误 ${counts.error}</p>
     <div class="audit-wrap"><table class="audit-table">
       <thead><tr><th>单位</th><th>状态</th><th>缺失字段</th><th>变更 / 多余</th></tr></thead>
       <tbody id="audit-tbody">${auditRowsHtml()}</tbody>
     </table></div>
     <p class="hint">点击行查看该条目的字段级对比；执行「生成说明文件」后，磁盘内容将与当前模板一致。</p>`,
    [{ label: '关闭', onClick: (close) => close() }],
    false,
    () => {
      auditOpen = false;
    },
  );
  document.querySelectorAll<HTMLTableRowElement>('#audit-tbody tr[data-rel]').forEach((tr) => {
    tr.addEventListener('click', () => {
      const rel = tr.dataset.rel;
      if (!rel) return;
      state.selectedRel = rel;
      render();
      void openNoteDiff(openAuditResultModal);
    });
  });
}

function finishAudit(p: AuditProgress): void {
  auditCancelled = p.status === 'cancelled';
  const counts = auditCounts();
  const proj = state.project;
  if (proj) {
    let changed = false;
    for (const it of auditItems) {
      if (it.exists) {
        const e = findEntry(proj, it.rel_path);
        if (e && !e.has_notes) {
          e.has_notes = true;
          changed = true;
        }
      }
    }
    if (changed) markDirty();
  }
  const line = `共 ${auditItems.length} · 一致 ${counts.same} · 缺说明 ${counts.missingNote} · 缺字段 ${counts.missingFields} · 有变更 ${counts.changed} · 多余 ${counts.stale} · 错误 ${counts.error}`;
  log(`说明文件核对完成${auditCancelled ? '（已取消）' : ''}：${line}`, counts.error ? 'error' : 'ok');
  toast(auditCancelled ? '核对已取消' : '说明文件核对完成');
  render();
  if (auditOpen) openAuditResultModal();
}

async function auditNotes(): Promise<void> {
  const p = project();
  if (!p.root) {
    toast('请先扫描根目录');
    return;
  }
  const sel = selectedEntries(p);
  const targets = sel.length ? sel : p.entries;
  if (!targets.length) {
    toast('没有可核对的条目');
    return;
  }
  auditScope =
    targets.length === p.entries.length ? `全部 ${targets.length} 条` : `已选 ${targets.length} 条`;
  auditItems = [];
  openAuditProgressModal(targets.length);
  try {
    await startNoteAudit(
      p.root,
      targets.map((e) => ({ ...e })),
    );
    log(`说明文件核对开始：${auditScope}`);
  } catch (err) {
    closeModal();
    toast(String(err), 4000);
    log(`核对启动失败：${String(err)}`, 'error');
  }
}

// ---------------------------------------------------------------- 查找重复文件

function openDupesProgressModal(): void {
  dupOpen = true;
  openModal(
    '查找重复文件',
    `<p class="hint">按「大小 → 全文 SHA-256」两级分组，查找根目录下内容完全相同的文件（只读，不写盘）。</p>
     <div class="audit-progress">
       <div class="audit-bar indeterminate" id="dupes-track"><div id="dupes-bar" style="width:0%"></div></div>
       <span class="hint" id="dupes-idx">准备中…</span>
     </div>
     <div class="hint" id="dupes-cur">扫描目录…</div>`,
    [
      {
        label: '取消',
        onClick: async () => {
          await cancelJob().catch(() => {});
        },
      },
    ],
    false,
    () => {
      dupOpen = false;
    },
  );
}

function dupeStats(): { files: number; waste: number } {
  let files = 0;
  let waste = 0;
  for (const g of dupeGroups) {
    files += g.files.length;
    waste += g.size * (g.files.length - 1);
  }
  return { files, waste };
}

function openDupesResultModal(): void {
  dupOpen = true;
  const { files, waste } = dupeStats();
  const body = dupeGroups.length
    ? `<p class="hint">共 ${dupeGroups.length} 组 · ${files} 个文件 · 清掉全部多余副本约可释放 ${formatSize(waste)}</p>
     <div class="dupe-list" id="dupe-list">${dupeGroups
       .map(
         (g, gi) => `<div class="dupe-group">
         <div class="dupe-head">
           <span class="dupe-size">${formatSize(g.size)} × ${g.files.length} 份</span>
           <span class="hint" title="SHA-256 ${g.hash}">${g.hash.slice(0, 12)}…</span>
         </div>
         ${g.files
           .map(
             (f, fi) => `<div class="dupe-file">
           <label class="check-row compact"><input type="checkbox" class="dupe-check" data-g="${gi}" data-f="${fi}" ${fi > 0 ? 'checked' : ''} /><span class="dupe-path" title="${esc(f)}">${esc(f)}</span></label>
           <button type="button" class="btn-sm" data-dupe-locate="${gi}/${fi}">定位</button>
         </div>`,
           )
           .join('')}
       </div>`,
       )
       .join('')}</div>
     <p class="hint">每组首个文件默认保留（按字典序），其余副本默认勾选；删除走系统回收站，可找回。</p>`
    : `<p class="hint">未找到重复文件（相同大小且全文一致才会成组）。</p>`;
  openModal(
    '查找重复文件',
    body,
    [
      ...(dupeGroups.length
        ? [
            {
              label: '删除所选副本（回收站）',
              onClick: () => confirmDupeDelete(),
            },
          ]
        : []),
      { label: '关闭', onClick: (close) => close() },
    ],
    false,
    () => {
      dupOpen = false;
    },
  );
  document.querySelectorAll<HTMLButtonElement>('[data-dupe-locate]').forEach((btn) => {
    btn.addEventListener('click', () => {
      const [gi, fi] = (btn.dataset.dupeLocate ?? '').split('/').map(Number);
      const rel = dupeGroups[gi]?.files[fi];
      if (!rel) return;
      const abs = joinRoot(rel);
      state.cwd = parentDir(abs);
      state.fileSelection = new Set([abs]);
      void refreshFiles();
      log(`定位重复文件：${rel}`);
    });
  });
}

function confirmDupeDelete(): void {
  const p = project();
  const picked: string[] = [];
  document.querySelectorAll<HTMLInputElement>('.dupe-check:checked').forEach((cb) => {
    const gi = Number(cb.dataset.g);
    const fi = Number(cb.dataset.f);
    const rel = dupeGroups[gi]?.files[fi];
    if (rel) picked.push(rel);
  });
  if (!picked.length) {
    toast('请先勾选要删除的副本');
    return;
  }
  const names = picked.map((r) => r.split('/').pop() ?? r);
  const list = names.slice(0, 12).map((n) => `<li>${esc(n)}</li>`).join('');
  const more = names.length > 12 ? `<li>…等 ${names.length} 个文件</li>` : '';
  openModal(
    '删除重复副本',
    `<p>将把以下 ${picked.length} 个文件移入回收站（每组保留的文件不会被勾选/删除）：</p>
     <ul class="trash-list">${list}${more}</ul>
     <p class="hint">只移入回收站，不会永久删除文件。</p>`,
    [
      { label: '取消', onClick: () => openDupesResultModal() },
      {
        label: '移入回收站',
        primary: true,
        onClick: async () => {
          await trashItems(
            p.root,
            picked.map((r) => joinRoot(r)),
          );
          const del = new Set(picked.map((r) => r.toLowerCase()));
          dupeGroups = dupeGroups
            .map((g) => ({ ...g, files: g.files.filter((f) => !del.has(f.toLowerCase())) }))
            .filter((g) => g.files.length >= 2);
          openDupesResultModal();
          toast(`已移入回收站 ${picked.length} 个文件`);
          log(`查重删除：${picked.length} 个副本 → 回收站`, 'ok');
          if (state.cwd) void refreshFiles();
        },
      },
    ],
  );
}

function finishDupes(p: DupesProgress): void {
  if (p.status === 'error') {
    if (dupOpen) closeModal();
    toast(p.message || '查重任务失败', 4000);
    log(`查找重复失败：${p.message}`, 'error');
    return;
  }
  dupeGroups = p.groups;
  const { files, waste } = dupeStats();
  const tag = p.status === 'cancelled' ? '（已取消）' : '';
  log(
    `查找重复完成${tag}：${dupeGroups.length} 组 · ${files} 个文件 · 可释放约 ${formatSize(waste)}`,
    dupeGroups.length ? 'ok' : 'info',
  );
  toast(dupeGroups.length ? `发现 ${dupeGroups.length} 组重复文件` : '未找到重复文件');
  if (dupOpen) openDupesResultModal();
}

async function findDupes(): Promise<void> {
  const p = project();
  if (!p.root) {
    toast('请先扫描根目录');
    return;
  }
  dupeGroups = [];
  openDupesProgressModal();
  try {
    await startFindDuplicates(p.root);
    log(`开始查找重复文件：${p.root}`);
  } catch (err) {
    closeModal();
    toast(String(err), 4000);
    log(`查重启动失败：${String(err)}`, 'error');
  }
}

// ---------------------------------------------------------------- 校验导出完整性

const VERIFY_LABEL: Record<string, string> = {
  missing: '缺失',
  size: '大小不符',
  hash: '哈希不符',
  error: '读取失败',
};

function openVerifyProgressModal(): void {
  verifyOpen = true;
  openModal(
    '校验导出完整性',
    `<p class="hint">对照 <span class="verify-mono">vinepress-manifest.json</span> 逐文件核对存在性、大小与 SHA-256（只读，不写盘）。</p>
     <div class="audit-progress">
       <div class="audit-bar" id="verify-track"><div id="verify-bar" style="width:0%"></div></div>
       <span class="hint"><span id="verify-idx">0</span> / <span id="verify-total">?</span></span>
     </div>
     <div class="hint" id="verify-cur">读取清单…</div>`,
    [
      {
        label: '取消',
        onClick: async () => {
          await cancelJob().catch(() => {});
        },
      },
    ],
    false,
    () => {
      verifyOpen = false;
    },
  );
}

function openVerifyResultModal(): void {
  const r = verifyReport;
  if (!r) return;
  verifyOpen = true;
  const bad = r.items.filter((i) => i.status !== 'ok');
  const problems = bad.length + r.unlisted.length;
  const summary =
    `共 ${r.total} 个压缩包 · 正常 ${r.ok}` +
    (bad.length ? ` · 异常 ${bad.length}` : '') +
    (r.unlisted.length ? ` · 未入清单 ${r.unlisted.length}` : '');
  const rows = bad
    .map(
      (i) => `<tr>
        <td><span class="verify-badge verify-${i.status}">${VERIFY_LABEL[i.status] ?? i.status}</span></td>
        <td class="verify-archive" title="${esc(i.rel_path)}">${esc(i.archive)}</td>
        <td class="verify-size">${i.status === 'missing' ? '—' : formatSize(i.expected_size)}</td>
        <td class="verify-size">${i.status === 'missing' || i.status === 'error' ? '—' : formatSize(i.actual_size)}</td>
      </tr>`,
    )
    .join('');
  const body = `<p class="hint">${summary}${verifyCancelled ? '（已取消，仅含已检部分）' : ''}</p>
    ${
      bad.length
        ? `<div class="audit-wrap"><table class="audit-table">
      <thead><tr><th>状态</th><th>压缩包</th><th>期望大小</th><th>实际大小</th></tr></thead>
      <tbody>${rows}</tbody></table></div>`
        : `<p class="hint">清单记录与磁盘现状全部一致。</p>`
    }
    ${
      r.unlisted.length
        ? `<p class="hint">未入清单（输出目录内存在但清单未记录，重新导出后会补入）：</p>
      <div class="dupe-list verify-unlisted">${r.unlisted
        .map((u) => `<div class="dupe-file"><span class="dupe-path" title="${esc(u)}">${esc(u)}</span></div>`)
        .join('')}</div>`
        : ''
    }
    <p class="hint">清单文件：输出根目录下的 vinepress-manifest.json；每次导出会自动更新，取消的导出不会写入。</p>`;
  openModal(
    `校验结果${verifyCancelled ? ' · 已取消' : ''}`,
    body,
    [{ label: '关闭', onClick: (close) => close() }],
    false,
    () => {
      verifyOpen = false;
    },
  );
}

function finishVerify(p: VerifyProgress): void {
  if (p.status === 'error') {
    if (verifyOpen) closeModal();
    toast(p.message || '校验任务失败', 4000);
    log(`导出完整性校验失败：${p.message}`, 'error');
    return;
  }
  verifyReport = p.report;
  verifyCancelled = p.status === 'cancelled';
  const bad = p.report.items.filter((i) => i.status !== 'ok').length;
  const tag = verifyCancelled ? '（已取消）' : '';
  log(
    `导出完整性校验完成${tag}：共 ${p.report.total} · 正常 ${p.report.ok} · 异常 ${bad} · 未入清单 ${p.report.unlisted.length}`,
    bad || p.report.unlisted.length ? 'error' : 'ok',
  );
  toast(bad || p.report.unlisted.length ? `发现 ${bad + p.report.unlisted.length} 个问题` : '导出完整性校验通过');
  if (verifyOpen) openVerifyResultModal();
}

async function verifyManifest(): Promise<void> {
  const p = project();
  const out = p.output_root.trim();
  if (!out) {
    toast('请先设置输出目录');
    return;
  }
  verifyReport = null;
  verifyCancelled = false;
  openVerifyProgressModal();
  try {
    await startVerify(out);
    log(`开始校验导出完整性：${out}`);
  } catch (err) {
    closeModal();
    toast(String(err), 4000);
    log(`校验启动失败：${String(err)}`, 'error');
  }
}

// ---------------------------------------------------------------- 命名检查与修复

let naResult: NameCheckResult | null = null;
let naProfilesCache: [string, string][] | null = null;
/** 二次确认：第一次点击武装，4 秒内再点才执行 */
let naArmed = false;

async function openNameAudit(): Promise<void> {
  const p = project();
  if (!p.root) {
    toast('请先扫描根目录');
    return;
  }
  if (!naProfilesCache) {
    try {
      naProfilesCache = await nameProfiles();
    } catch (err) {
      toast(String(err), 4000);
      return;
    }
  }
  naResult = null;
  naArmed = false;
  openModal(
    '命名检查与修复',
    `<p class="hint">按目标平台/网盘的命名规则检查全盘目录与文件名（只读）。勾选问题项后「修复选中」会在磁盘上安全重命名，并同步条目路径与 &lt;单位名&gt;.txt 说明文件名（压缩包名 ≡ 目录名 ≡ 说明文件名）。</p>
     <div class="na-bar">
       <label class="field"><span>目标平台</span>
         <select id="na-profile">${naProfilesCache
           .map(([v, l]) => `<option value="${v}">${esc(l)}</option>`)
           .join('')}</select>
       </label>
       <button type="button" class="btn primary" id="na-run">开始检查</button>
       <button type="button" class="btn-sm" id="na-all" hidden>全选可修复</button>
       <button type="button" class="btn-sm" id="na-none" hidden>清空</button>
       <button type="button" class="btn danger" id="na-fix" hidden disabled>修复选中</button>
     </div>
     <div id="na-out"><p class="hint">尚未检查。</p></div>`,
    [{ label: '关闭', onClick: (close) => close() }],
    true,
  );
  el<HTMLButtonElement>('na-run').addEventListener('click', () => void naRun());
  el<HTMLButtonElement>('na-all').addEventListener('click', () => naSetAll(true));
  el<HTMLButtonElement>('na-none').addEventListener('click', () => naSetAll(false));
  el<HTMLButtonElement>('na-fix').addEventListener('click', () => void naFix());
}

async function naRun(): Promise<void> {
  const p = project();
  const btn = el<HTMLButtonElement>('na-run');
  btn.disabled = true;
  btn.textContent = '检查中…';
  try {
    naResult = await checkNames(p.root, el<HTMLSelectElement>('na-profile').value);
    naArmed = false;
    naRender();
    log(
      `命名检查（${naResult.profile}）：目录 ${naResult.checked_dirs} · 文件 ${naResult.checked_files} · 问题 ${naResult.issues.length}`,
      naResult.issues.length ? 'error' : 'ok',
    );
  } catch (err) {
    toast(String(err), 4000);
    log(`命名检查失败：${String(err)}`, 'error');
  } finally {
    btn.disabled = false;
    btn.textContent = '开始检查';
  }
}

function naFixable(it: NameCheckResult['issues'][number]): boolean {
  return it.kind !== 'root' && it.new_name !== it.name;
}

function naRender(): void {
  const r = naResult;
  const out = el('na-out');
  if (!r) {
    out.innerHTML = '<p class="hint">尚未检查。</p>';
    return;
  }
  const hasIssues = r.issues.length > 0;
  for (const id of ['na-all', 'na-none', 'na-fix']) {
    el<HTMLButtonElement>(id).hidden = !hasIssues;
  }
  if (!hasIssues) {
    out.innerHTML = `<p class="hint">检查 ${r.checked_dirs} 个目录 + ${r.checked_files} 个文件：命名全部合规（档案：${esc(r.profile)}）。</p>`;
    naUpdateFixBtn();
    return;
  }
  const errors = r.issues.filter((i) => i.level === 'error').length;
  const warns = r.issues.length - errors;
  const fixable = r.issues.filter(naFixable).length;
  const rows = r.issues
    .map((it, i) => {
      const canFix = naFixable(it);
      const kind = it.kind === 'root' ? '根目录' : it.kind === 'dir' ? '目录' : '文件';
      const name = canFix
        ? `<span class="na-old" title="${esc(it.rel_path || it.name)}">${esc(it.name)}</span><span class="na-arrow">→</span><span class="na-new">${esc(it.new_name)}</span>`
        : `<span class="na-old">${esc(it.name)}</span>${it.kind === 'root' ? '<span class="na-arrow">·</span><span class="na-hint">需手动改</span>' : ''}`;
      return `<tr class="${it.level === 'error' ? 'na-err' : 'na-warn'}">
        <td><input type="checkbox" class="na-check" data-i="${i}" ${canFix ? '' : 'disabled'}></td>
        <td><span class="na-level na-level-${it.level}">${it.level === 'error' ? '违规' : '提示'}</span></td>
        <td>${kind}</td>
        <td class="na-path" title="${esc(it.rel_path)}">${esc(it.rel_path || '（根目录）')}</td>
        <td class="na-name">${name}</td>
        <td class="na-reason">${esc(it.reasons.join('；'))}</td>
      </tr>`;
    })
    .join('');
  out.innerHTML = `<p class="hint">检查 ${r.checked_dirs} 目录 + ${r.checked_files} 文件 · 违规 ${errors} · 提示 ${warns} · 可自动修复 ${fixable}${fixable < r.issues.length ? '（其余需手动处理）' : ''}</p>
    <div class="audit-wrap"><table class="audit-table na-table">
      <thead><tr><th></th><th>级别</th><th>类型</th><th>路径</th><th>名称 → 建议</th><th>原因</th></tr></thead>
      <tbody>${rows}</tbody></table></div>`;
  out.querySelectorAll<HTMLInputElement>('.na-check').forEach((cb) =>
    cb.addEventListener('change', () => {
      naArmed = false;
      naUpdateFixBtn();
    }),
  );
  naUpdateFixBtn();
}

function naUpdateFixBtn(): void {
  const btn = el<HTMLButtonElement>('na-fix');
  if (btn.hidden) return;
  const n = document.querySelectorAll<HTMLInputElement>('#na-out .na-check:checked').length;
  btn.disabled = n === 0;
  btn.textContent = naArmed
    ? `确认修复 ${n} 项（磁盘改名）`
    : n
      ? `修复选中 (${n})`
      : '修复选中';
}

function naSetAll(on: boolean): void {
  document
    .querySelectorAll<HTMLInputElement>('#na-out .na-check:not(:disabled)')
    .forEach((cb) => (cb.checked = on));
  naArmed = false;
  naUpdateFixBtn();
}

async function naFix(): Promise<void> {
  const p = project();
  const r = naResult;
  if (!r) return;
  if (busyGuard()) return;
  const checks = [...document.querySelectorAll<HTMLInputElement>('#na-out .na-check:checked')];
  if (!checks.length) return;
  if (!naArmed) {
    naArmed = true;
    naUpdateFixBtn();
    toast('再次点击「确认修复」执行磁盘改名');
    window.setTimeout(() => {
      if (naArmed) {
        naArmed = false;
        naUpdateFixBtn();
      }
    }, 4000);
    return;
  }
  naArmed = false;
  const items = checks.map((cb) => {
    const it = r.issues[Number(cb.dataset.i)];
    return { rel_path: it.rel_path, new_name: it.new_name };
  });
  const btn = el<HTMLButtonElement>('na-fix');
  btn.disabled = true;
  btn.textContent = '修复中…';
  try {
    const outcome = await fixNames(
      p.root,
      p.entries.map((e) => ({ ...e })),
      items,
    );
    p.entries = outcome.entries;
    for (const pair of outcome.renamed) applyRelShift(pair.old_rel, pair.new_rel);
    markDirty();
    log(
      `命名修复：成功 ${outcome.renamed.length} · 失败 ${outcome.failed.length}`,
      outcome.failed.length ? 'error' : 'ok',
    );
    for (const f of outcome.failed) {
      log(`  修复失败 ${f.rel_path}：${f.message}`, 'error');
    }
    toast(
      outcome.failed.length
        ? `已修复 ${outcome.renamed.length} 项 · ${outcome.failed.length} 项失败`
        : `已修复 ${outcome.renamed.length} 项`,
    );
    await doScan();
    await refreshFiles();
    await naRun();
  } catch (err) {
    toast(String(err), 4000);
    log(`命名修复失败：${String(err)}`, 'error');
    naUpdateFixBtn();
  }
}

// ---------------------------------------------------------------- 高级筛选

function syncAdvancedInputs(): void {
  el<HTMLSelectElement>('year-filter').value = state.filters.year;
  el<HTMLSelectElement>('tag-filter').value = state.filters.tag;
  el<HTMLSelectElement>('profile-filter').value = state.filters.profile;
  el<HTMLInputElement>('size-min').value = state.filters.sizeMin;
  el<HTMLInputElement>('size-max').value = state.filters.sizeMax;
}

function updateFilterCount(): void {
  const f = state.filters;
  const n = [f.year, f.tag, f.profile, f.sizeMin, f.sizeMax].filter((v) => v !== '').length;
  const badge = document.getElementById('filter-count');
  if (badge) badge.textContent = n ? `高级筛选 ${n} 项生效` : '';
}

function applyFilters(): void {
  renderTable(state);
  renderStats(state);
  updateFilterCount();
}

function toggleAdvancedFilter(): void {
  const bar = document.getElementById('advanced-filter');
  if (!bar) return;
  bar.classList.toggle('hidden');
  el('filter-toggle').classList.toggle('accent', !bar.classList.contains('hidden'));
}

function clearAllFilters(): void {
  clearEntryFilters(state.filters);
  el<HTMLInputElement>('entry-filter').value = '';
  el<HTMLSelectElement>('kind-filter').value = '';
  el<HTMLSelectElement>('status-filter').value = '';
  syncAdvancedInputs();
  applyFilters();
  toast('已清除全部筛选');
}

// ---------------------------------------------------------------- 全局搜索（Ctrl+Shift+F）

let searchHits: Entry[] = [];
let searchSel = 0;

function closeSearch(): void {
  document.getElementById('search-overlay')?.classList.add('hidden');
}

function renderSearch(): void {
  const input = document.getElementById('search-input') as HTMLInputElement | null;
  const list = document.getElementById('search-list');
  if (!input || !list) return;
  const q = input.value.trim().toLowerCase();
  const proj = state.project;
  searchHits = !q || !proj ? [] : proj.entries.filter((e) => searchHaystack(e).includes(q)).slice(0, 60);
  if (searchSel >= searchHits.length) searchSel = Math.max(0, searchHits.length - 1);
  list.innerHTML = searchHits.length
    ? searchHits
        .map((e, i) => {
          const title = e.title || e.name;
          const at = title.toLowerCase().indexOf(q);
          const marked =
            at >= 0
              ? esc(title.slice(0, at)) + `<mark>${esc(title.slice(at, at + q.length))}</mark>` + esc(title.slice(at + q.length))
              : esc(title);
          return `<div class="search-item${i === searchSel ? ' active' : ''}" data-idx="${i}">
            <span class="si-kind">${kindLabel(e.kind)}</span>
            <span class="si-title">${marked}</span>
            <span class="si-path hint" title="${esc(e.rel_path)}">${esc(e.rel_path)}</span>
          </div>`;
        })
        .join('')
    : `<div class="search-empty hint">${proj ? '没有匹配的条目' : '尚未加载项目'}</div>`;
  list.querySelector('.search-item.active')?.scrollIntoView({ block: 'nearest' });
  list.querySelectorAll<HTMLElement>('.search-item').forEach((row) => {
    row.addEventListener('click', () => {
      searchSel = Number(row.dataset.idx);
      activateSearch();
    });
    row.addEventListener('mouseenter', () => {
      searchSel = Number(row.dataset.idx);
      list.querySelectorAll('.search-item').forEach((r, i) => r.classList.toggle('active', i === searchSel));
    });
  });
}

function activateSearch(): void {
  const hit = searchHits[searchSel];
  if (!hit) return;
  closeSearch();
  // 目标可能被当前筛选隐藏 → 有筛选时先清空（树过滤不受影响）
  if (filtersActive(state.filters)) {
    clearEntryFilters(state.filters);
    el<HTMLInputElement>('entry-filter').value = '';
    el<HTMLSelectElement>('kind-filter').value = '';
    el<HTMLSelectElement>('status-filter').value = '';
    syncAdvancedInputs();
    updateFilterCount();
    toast('已清除筛选以便跳转');
  }
  state.selectedRel = hit.rel_path;
  state.selectedNodeRel = null;
  render();
  focusUnit(hit.rel_path);
  const row = [...document.querySelectorAll<HTMLTableRowElement>('#entry-tbody tr[data-rel]')].find(
    (r) => r.dataset.rel === hit.rel_path,
  );
  row?.scrollIntoView({ block: 'nearest' });
  log(`全局搜索跳转：${hit.rel_path}`);
}

function openSearch(): void {
  const overlay = document.getElementById('search-overlay');
  const input = document.getElementById('search-input') as HTMLInputElement | null;
  if (!overlay || !input) return;
  overlay.classList.remove('hidden');
  input.value = '';
  searchSel = 0;
  renderSearch();
  input.focus();
}

function initSearch(): void {
  const overlay = document.getElementById('search-overlay');
  const input = document.getElementById('search-input') as HTMLInputElement | null;
  if (!overlay || !input) return;
  input.addEventListener('input', () => {
    searchSel = 0;
    renderSearch();
  });
  input.addEventListener('keydown', (e) => {
    const count = searchHits.length;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (count) searchSel = (searchSel + 1) % count;
      renderSearch();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (count) searchSel = (searchSel - 1 + count) % count;
      renderSearch();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      activateSearch();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      closeSearch();
    }
  });
  overlay.addEventListener('click', (e) => {
    if (e.target === overlay) closeSearch();
  });
}

function confirmTrash(): void {
  const p = project();
  const items = [...state.fileSelection];
  if (!items.length) {
    toast('先在文件列表中选择要删除的项');
    return;
  }
  if (busyGuard()) return;
  if (!p.root) {
    toast('请先扫描根目录');
    return;
  }
  const names = items.map((i) => i.split(/[\\/]/).filter(Boolean).pop() ?? i);
  const list = names.slice(0, 12).map((n) => `<li>${esc(n)}</li>`).join('');
  const more = names.length > 12 ? `<li>…等 ${names.length} 项</li>` : '';
  openModal(
    '移入回收站',
    `<p>将把以下 ${items.length} 项移入回收站（可从回收站找回）：</p>
     <ul class="trash-list">${list}${more}</ul>
     <p class="hint">只移入回收站，不会永久删除文件。</p>`,
    [
      { label: '取消', onClick: (close) => close() },
      {
        label: '移入回收站',
        primary: true,
        onClick: async (close) => {
          const deleted = await trashItems(p.root, items);
          close();
          toast(`已移入回收站 ${deleted.length} 项`);
          log(`回收站：${deleted.length} 项（来自 ${state.cwd ?? p.root}）`, 'ok');
          // 删掉了当前目录或其祖先 → 回到根目录
          if (state.cwd) {
            const cwd = state.cwd.toLowerCase();
            const hit = items.some((it) => {
              const t = it.toLowerCase();
              return cwd === t || cwd.startsWith(`${t}\\`);
            });
            if (hit) state.cwd = rootPath();
          }
          await refreshFiles();
          await doScan();
        },
      },
    ],
  );
}

// ---------------------------------------------------------------- 自动识别填充

async function openAutofill(entries: Entry[] | null): Promise<void> {
  const p = project();
  if (!p.root) {
    toast('请先扫描根目录');
    return;
  }
  if (!p.entries.length) {
    toast('尚未识别条目，请先扫描');
    return;
  }
  const includeScope = entries === null;
  if (entries && !entries.length) {
    toast('没有可填充的条目');
    return;
  }

  openModal(
    '自动识别填充',
    autofillFieldsHtml(includeScope, p.entries.length, selectedEntries(p).length),
    [
      { label: '取消', onClick: (close) => close() },
      {
        label: '识别…',
        primary: true,
        onClick: (close) => {
          const fields = checkedAutofillFields();
          if (!fields.length) {
            toast('至少选择一个字段');
            return;
          }
          const targets =
            entries ?? (autofillScope() === 'selected' ? selectedEntries(p) : [...p.entries]);
          if (!targets.length) {
            toast('填充范围内没有条目');
            return;
          }
          close();
          void runProposals(targets, fields);
        },
      },
    ],
  );
}

async function runProposals(targets: Entry[], fields: string[]): Promise<void> {
  const p = project();
  toast(`正在识别 ${targets.length} 个单位…`, 6000);
  try {
    const props = await autoFillProposals(
      p.root,
      targets.map((e) => ({ ...e })),
      fields,
    );
    log(`自动识别：${targets.length} 个单位 → ${props.length} 条提案`);
    if (!props.length) {
      toast('未发现可填充内容');
      return;
    }
    openProposalModal(props, applyProposals);
  } catch (err) {
    toast(`识别失败：${String(err)}`, 4000);
    log(`自动识别失败：${String(err)}`, 'error');
  }
}

function applyProposals(checked: Proposal[]): void {
  const p = project();
  const byRel = new Map(p.entries.map((e) => [e.rel_path, e]));
  let n = 0;
  for (const pr of checked) {
    const e = byRel.get(pr.rel_path);
    if (!e) continue;
    if (pr.field === 'tags') {
      e.tags = pr.value
        .split(/[、,，]/)
        .map((s) => s.trim())
        .filter(Boolean);
    } else if (pr.field === 'tracklist') {
      e.tracklist = pr.value
        .split('\n')
        .map((s) => s.trim())
        .filter(Boolean);
    } else {
      (e as unknown as Record<string, string>)[pr.field] = pr.value;
    }
    n++;
  }
  markDirty();
  render();
  toast(`已应用 ${n} 项填充`);
  log(`自动填充应用 ${n} 项`, 'ok');
}

// ---------------------------------------------------------------- 文件面板

async function refreshFiles(): Promise<void> {
  if (!state.cwd) {
    state.fileItems = [];
    renderFileList(state);
    return;
  }
  try {
    state.fileItems = await listDir(project().root, state.cwd, state.showAllFiles);
    const paths = new Set(state.fileItems.map((f) => f.path));
    state.fileSelection = new Set([...state.fileSelection].filter((x) => paths.has(x)));
    lastFileIndex = null;
    renderFileList(state);
  } catch (err) {
    state.fileItems = [];
    state.fileSelection.clear();
    renderFileList(state);
    log(`文件列表失败：${String(err)}`, 'error');
  }
}

function focusUnit(rel: string): void {
  if (!project().root) return;
  state.cwd = joinRoot(rel);
  state.fileSelection.clear();
  void refreshFiles();
}

/** 选中行并聚焦详情面板标题输入（双击行 / Enter） */
function focusDetailEdit(rel: string): void {
  state.selectedRel = rel;
  renderTable(state);
  refreshDetail(state);
  focusUnit(rel);
  if (!paneVisible('right')) togglePane('right');
  const input = document.querySelector<HTMLInputElement>('#detail-form input[data-field="title"]');
  input?.focus();
  input?.select();
}

async function doOpenSelected(reveal: boolean): Promise<void> {
  const p = project();
  if (!p.root) return;
  const target = [...state.fileSelection][0] ?? state.cwd;
  if (!target) {
    toast('未选择路径');
    return;
  }
  try {
    if (reveal) await revealPath(p.root, target);
    else await openPath(p.root, target);
  } catch (err) {
    toast(String(err), 4000);
  }
}

function destCandidates(): string[] {
  const p = project();
  const set = new Set<string>(['']);
  for (const e of p.entries) {
    const parent = e.rel_path.split('/').slice(0, -1).join('/');
    if (parent) set.add(parent);
  }
  if (state.cwd) set.add(relOf(state.cwd));
  return [...set].sort();
}

function fileTransfer(mode: 'copy' | 'move'): void {
  const p = project();
  const items = [...state.fileSelection];
  if (!items.length) {
    toast('先在文件列表中选择条目');
    return;
  }
  if (busyGuard()) return;
  if (!state.cwd) {
    toast('先选择目录');
    return;
  }
  openPrompt(
    mode === 'copy' ? '复制到…' : '移动到…',
    '目标目录（相对根目录，留空 = 根目录）',
    relOf(state.cwd),
    '例如：分类/新分类',
    async (rel) => {
      try {
        const dest = joinRoot(rel);
        const created =
          mode === 'copy'
            ? await copyItems(p.root, items, dest)
            : await moveItems(p.root, items, dest);
        await refreshFiles();
        toast(`${mode === 'copy' ? '已复制' : '已移动'} ${created.length} 项`);
        log(`${mode === 'copy' ? '复制' : '移动'} ${created.length} 项 → ${dest}`, 'ok');
      } catch (err) {
        toast(String(err), 4000);
        log(`${mode} 失败：${String(err)}`, 'error');
      }
    },
    destCandidates(),
    true,
  );
}

function openClassify(): void {
  const p = project();
  const sel = selectedEntries(p);
  if (!sel.length) {
    toast('请先勾选条目');
    return;
  }
  if (!p.root) {
    toast('请先扫描根目录');
    return;
  }
  const base = state.cwd && state.cwd.toLowerCase().startsWith(rootPath().toLowerCase()) ? relOf(state.cwd) : '';
  openPrompt(
    '新建分类并复制',
    `分类名（将在「${base || '根目录'}」下新建，并复制已选 ${sel.length} 个条目）`,
    '',
    '例如：精选推荐',
    async (name) => {
      const dest = joinRoot(base ? `${base}/${name}` : name);
      try {
        await makeDirectory(p.root, dest);
        const created = await copyItems(
          p.root,
          sel.map((e) => joinRoot(e.rel_path)),
          dest,
        );
        await refreshFiles();
        toast(`已复制 ${created.length} 个条目到「${name}」`);
        log(`新建分类「${name}」并复制 ${created.length} 个条目 → ${dest}`, 'ok');
      } catch (err) {
        toast(String(err), 4000);
        log(`分类失败：${String(err)}`, 'error');
      }
    },
  );
}

// ---------------------------------------------------------------- 说明文件

async function doGenerateNotes(): Promise<void> {
  const p = project();
  if (!p.root) {
    toast('请先扫描根目录');
    return;
  }
  const sel = selectedEntries(p);
  if (!sel.length) {
    toast('请先勾选要生成说明文件的条目');
    return;
  }
  readArchiveForm(p);
  setBusy('notes');
  setProgress(0, `生成说明文件 0/${sel.length}`);
  try {
    await startGenerateNotes(p.root, sel.map((e) => ({ ...e })));
    log(`开始生成说明文件：${sel.length} 个单位`);
  } catch (err) {
    setBusy(null);
    toast(String(err), 4000);
    log(`启动失败：${String(err)}`, 'error');
  }
}

// ---------------------------------------------------------------- 导出

function doExport(): void {
  const p = project();
  if (!p.root) {
    toast('请先扫描根目录');
    return;
  }
  if (!p.output_root) {
    toast('请填写输出目录');
    return;
  }
  const sel = selectedEntries(p);
  if (!sel.length) {
    toast('请先勾选要导出的条目');
    return;
  }
  if (p.archive.format !== 'zip' && !tools?.rar) {
    toast('未找到 WinRAR（Rar.exe），无法导出 RAR', 4000);
    return;
  }
  readArchiveForm(p);
  void preflightThenExport(sel);
}

/** 导出前自检 + 大批量确认；无任何提示时直接开跑 */
async function preflightThenExport(sel: Entry[]): Promise<void> {
  const p = project();
  let issues: PreflightIssue[] = [];
  try {
    issues = await preflightExport(
      p.root,
      p.output_root,
      sel.map((e) => ({ ...e })),
      { ...p.archive },
      p.password_profiles.map((x) => ({ ...x, passwords: [...x.passwords] })),
    );
  } catch (err) {
    toast(`自检失败：${String(err)}`, 4000);
    log(`导出自检失败：${String(err)}`, 'error');
    return;
  }

  const errors = issues.filter((i) => i.level === 'error').length;
  const bytes = sel.reduce((a, e) => a + e.stats.total_size, 0);
  const big = sel.length > 50 || bytes > 100 * 1024 * 1024 * 1024;

  if (!issues.length && !big) {
    log(`导出自检通过：${sel.length} 个单位`);
    void runExport(sel);
    return;
  }

  const rows = issues
    .map(
      (i) => `<div class="pf-row ${i.level === 'error' ? 'pf-error' : i.level === 'info' ? 'pf-info' : 'pf-warn'}">
        <b>${i.level === 'error' ? '错误' : '提示'}</b>
        ${i.rel_path ? `<code>${esc(i.rel_path)}</code> ` : ''}${esc(i.message)}
      </div>`,
    )
    .join('');
  const bigNote = big
    ? `<div class="pf-row pf-warn"><b>大批量</b> 共 ${sel.length} 个单位、约 ${formatSize(bytes)}，确认无误再继续。</div>`
    : '';
  log(
    `导出自检：${issues.length} 条提示（错误 ${errors} / 提示 ${issues.length - errors}）`,
    errors ? 'error' : 'info',
  );

  openModal(
    '导出前自检',
    `${rows || ''}${bigNote}<p class="hint">自检只提示风险；确认后仍可导出，随时可取消。</p>`,
    [
      { label: '取消', onClick: (close) => close() },
      {
        label: '仍然导出',
        primary: true,
        onClick: (close) => {
          close();
          void runExport(sel);
        },
      },
    ],
    true,
  );
}

async function runExport(sel: Entry[]): Promise<void> {
  const p = project();
  rememberPaths(p);
  const request: ExportRequest = {
    root: p.root,
    output_root: p.output_root,
    entries: sel.map((e) => ({ ...e })),
    settings: { ...p.archive },
    profiles: p.password_profiles.map((x) => ({ ...x, passwords: [...x.passwords] })),
  };
  setBusy('export');
  setProgress(0, `导出 0/${sel.length}`);
  try {
    await startExport(request);
    log(`开始导出：${sel.length} 个单位 → ${p.output_root}`);
  } catch (err) {
    setBusy(null);
    toast(String(err), 4000);
    log(`启动失败：${String(err)}`, 'error');
  }
}

async function doCancel(): Promise<void> {
  try {
    await cancelJob();
    log('已请求取消', 'error');
  } catch (err) {
    toast(String(err));
  }
}

// ---------------------------------------------------------------- 项目

/** 清空扫描结果与文件面板（保留过滤条件） */
function clearScanResults(): void {
  state.tree = [];
  state.scan = null;
  state.selectedRel = null;
  state.selectedNodeRel = null;
  state.expanded = new Set<string>();
  state.cwd = null;
  state.fileItems = [];
  state.fileSelection.clear();
  lastFileIndex = null;
}

// ---------------------------------------------------------------- 脏状态（未保存修改）

const TITLE_BASE = 'vinePress - 文件归档';
let savedSnapshot: string | null = null;
/** 已确认退出/关闭：跳过 window close-requested 的重复询问 */
let forceClose = false;

/** 项目数据快照（剔除瞬态 UI 字段 selected），用于脏状态判定 */
function snapshotOf(p: Project | null): string | null {
  if (!p) return null;
  return JSON.stringify({
    ...p,
    entries: p.entries.map(({ selected: _ignored, ...rest }) => rest),
  });
}

function updateDirtyUi(): void {
  document.title = state.dirty ? `● ${TITLE_BASE}` : TITLE_BASE;
  el('project-save-btn').classList.toggle('dirty', state.dirty);
}

/** 数据变更后重新计算脏标记（保存/加载/新建后用 clearDirty 归零） */
function markDirty(): void {
  state.dirty = savedSnapshot !== snapshotOf(state.project);
  updateDirtyUi();
}

/** 关键路径（退出/新建/打开）：强制复核，防止个别调用点遗漏 */
function isDirty(): boolean {
  markDirty();
  return state.dirty;
}

function clearDirty(): void {
  savedSnapshot = snapshotOf(state.project);
  state.dirty = false;
  updateDirtyUi();
}

/** 新建/打开项目前的丢弃确认（仅真正有未保存修改时弹出） */
async function confirmDiscard(): Promise<boolean> {
  if (!isDirty()) return true;
  const n = state.project?.entries.length ?? 0;
  return new Promise((resolve) => {
    openModal(
      '未保存的修改',
      `<p>当前项目有<b>未保存的修改</b>${n ? `（${n} 个条目）` : ''}。继续将丢弃它们。</p>
       <p class="hint">提示：先点「保存」再继续。</p>`,
      [
        { label: '取消', onClick: (close) => { close(); resolve(false); } },
        { label: '仍然继续', primary: true, onClick: (close) => { close(); resolve(true); } },
      ],
    );
  });
}

async function newProject(): Promise<void> {
  if (!(await confirmDiscard())) return;
  const root = el<HTMLInputElement>('root-path').value.trim();
  const output = el<HTMLInputElement>('output-path').value.trim();
  state.project = emptyProject(root ? root.split(/[\\/]/).filter(Boolean).pop() ?? '未命名项目' : '未命名项目');
  const defaults = getPrefs().archiveDefaults;
  if (defaults) state.project.archive = { ...defaults };
  state.project.root = root;
  state.project.output_root = output;
  state.projectPath = null;
  clearScanResults();
  resetEntryFilterUi();
  state.activeProfileId = null;
  clearDirty();
  render();
  toast('已新建项目');
}

// ---------------------------------------------------------------- 最近打开的项目

function recentList(): string[] {
  try {
    const raw = localStorage.getItem('vinepress.recent');
    const arr = raw ? (JSON.parse(raw) as unknown) : [];
    return Array.isArray(arr) ? arr.filter((x): x is string => typeof x === 'string') : [];
  } catch {
    return [];
  }
}

function pushRecent(path: string): void {
  const list = [path, ...recentList().filter((p) => p.toLowerCase() !== path.toLowerCase())].slice(0, 8);
  localStorage.setItem('vinepress.recent', JSON.stringify(list));
  setRecentProjects(list);
}

function dropRecent(path: string): void {
  const list = recentList().filter((p) => p.toLowerCase() !== path.toLowerCase());
  localStorage.setItem('vinepress.recent', JSON.stringify(list));
  setRecentProjects(list);
}

async function openProjectPath(path: string): Promise<void> {
  if (!(await confirmDiscard())) return;
  try {
    const p = await loadProject(path);
    state.project = p;
    ensureOverrides(p);
    state.projectPath = path;
    clearScanResults();
    resetEntryFilterUi();
    el<HTMLInputElement>('root-path').value = p.root;
    el<HTMLInputElement>('output-path').value = p.output_root;
    syncArchiveForm(p);
    clearDirty();
    render();
    log(`已打开项目：${path}（${p.entries.length} 条目）`, 'ok');
    pushRecent(path);
    if (p.root) await doScan();
  } catch (err) {
    toast(`打开失败：${String(err)}`, 4000);
    log(`打开失败：${String(err)}`, 'error');
    dropRecent(path);
  }
}

async function openProjectFile(): Promise<void> {
  const path = await openDialog({
    multiple: false,
    filters: [{ name: 'vinePress 项目', extensions: ['json'] }],
  });
  if (typeof path !== 'string') return;
  await openProjectPath(path);
}

async function saveProjectFile(forceAs = false): Promise<boolean> {
  const p = project();
  readArchiveForm(p);
  const path =
    (!forceAs && state.projectPath) ||
    (await saveDialog({
      filters: [{ name: 'vinePress 项目', extensions: ['json'] }],
      defaultPath: state.projectPath?.split(/[\\/]/).pop() ?? 'vinePress-project.json',
    }));
  if (typeof path !== 'string') return false;
  try {
    await saveProject(path, p);
    state.projectPath = path;
    rememberPaths(p);
    pushRecent(path);
    clearDirty();
    toast('项目已保存');
    log(`已保存项目：${path}`, 'ok');
    return true;
  } catch (err) {
    toast(`保存失败：${String(err)}`, 4000);
    return false;
  }
}

/** 设置 → 默认压缩参数「应用到当前项目」 */
function applyArchiveDefaults(): void {
  const defaults = getPrefs().archiveDefaults;
  if (!defaults) {
    toast('尚未保存默认压缩参数');
    return;
  }
  const p = project();
  p.archive = { ...defaults };
  syncArchiveForm(p);
  updateFormatAvailability();
  markDirty();
  toast('已应用默认压缩参数');
}

async function reallyQuit(): Promise<void> {
  forceClose = true;
  try {
    await quitApp();
  } catch (err) {
    forceClose = false;
    toast(`退出失败：${String(err)}`, 4000);
  }
}

/** 文件 → 退出 */
function quit(): void {
  if (state.busy) {
    openModal(
      '退出 vinePress',
      `<p>任务正在进行中，退出会中断操作。</p>
       <p class="hint">中断的 RAR 导出可能留下半成品文件（ZIP 半成品会被清理）。</p>`,
      [
        { label: '取消', onClick: (close) => close() },
        {
          label: '仍然退出',
          primary: true,
          onClick: (close) => {
            close();
            void reallyQuit();
          },
        },
      ],
    );
    return;
  }
  if (isDirty()) {
    openModal(
      '退出 vinePress',
      `<p>当前项目有<b>未保存的修改</b>，退出将丢弃它们。</p>`,
      [
        { label: '取消', onClick: (close) => close() },
        {
          label: '不保存退出',
          onClick: (close) => {
            close();
            void reallyQuit();
          },
        },
        {
          label: '保存并退出',
          primary: true,
          onClick: async (close) => {
            const ok = await saveProjectFile();
            if (ok) {
              close();
              void reallyQuit();
            }
          },
        },
      ],
    );
    return;
  }
  void reallyQuit();
}

/** 窗口右上角 ×：有未保存修改 / 任务进行中时先询问（force 标志绕过重复询问） */
async function wireClosePrompt(): Promise<void> {
  try {
    const win = (await import('@tauri-apps/api/window')).getCurrentWindow();
    await win.onCloseRequested(async (event) => {
      if (forceClose) return;
      const dirty = isDirty();
      if (!state.busy && !dirty) return;
      event.preventDefault();
      if (state.busy) {
        openModal(
          '关闭 vinePress',
          `<p>任务正在进行中，关闭会中断操作。</p>
           <p class="hint">中断的 RAR 导出可能留下半成品文件（ZIP 半成品会被清理）。</p>`,
          [
            { label: '取消', onClick: (close) => close() },
            {
              label: '仍然关闭',
              primary: true,
              onClick: (close) => {
                close();
                forceClose = true;
                void win.destroy();
              },
            },
          ],
        );
        return;
      }
      openModal(
        '关闭 vinePress',
        `<p>当前项目有<b>未保存的修改</b>，关闭将丢弃它们。</p>`,
        [
          { label: '取消', onClick: (close) => close() },
          {
            label: '不保存关闭',
            onClick: (close) => {
              close();
              forceClose = true;
              void win.destroy();
            },
          },
          {
            label: '保存并关闭',
            primary: true,
            onClick: async (close) => {
              const ok = await saveProjectFile();
              if (ok) {
                close();
                forceClose = true;
                await win.destroy();
              }
            },
          },
        ],
      );
    });
  } catch {
    // 非 Tauri 环境（纯浏览器调试）：无关闭事件可监听
  }
}

/** 主菜单栏命令表（data-cmd → 实现） */
function menuActions(): MenuActions {
  return {
    newProject: () => void newProject(),
    openProject: () => void openProjectFile(),
    openRecent: (path) => void openProjectPath(path),
    saveProject: () => void saveProjectFile(),
    saveProjectAs: () => void saveProjectFile(true),
    settings: () => openSettings({ applyArchiveDefaultsToProject: applyArchiveDefaults }),
    quit,
    selectAll: () => {
      for (const e of visibleEntries(state)) e.selected = true;
      renderTable(state);
      renderStats(state);
    },
    selectNone: () => {
      for (const e of visibleEntries(state)) e.selected = false;
      renderTable(state);
      renderStats(state);
    },
    batchEdit: () => openBatchEdit(),
    autofill: () => void openAutofill(null),
    classify: () => openClassify(),
    previewNote: () => void previewCurrentNote(),
    diffNote: () => void openNoteDiff(),
    trash: () => confirmTrash(),
    globalSearch: () => openSearch(),
    scan: () => void doScan(),
    notes: () => void doGenerateNotes(),
    auditNotes: () => void auditNotes(),
    findDupes: () => void findDupes(),
    verifyManifest: () => void verifyManifest(),
    nameAudit: () => void openNameAudit(),
    exportSettings: () => openExportSettings(),
    exportRun: () => doExport(),
    exportCancel: () => void doCancel(),
  };
}

// ---------------------------------------------------------------- 批量编辑

function openBatchEdit(): void {
  const p = project();
  const sel = selectedEntries(p);
  if (!sel.length) {
    toast('请先勾选条目');
    return;
  }
  const profileOptions = p.password_profiles
    .map((x) => `<option value="${x.id}">${x.usage || x.id}</option>`)
    .join('');

  openModal(
    `批量编辑（已选 ${sel.length} 条）`,
    `<p class="hint">留空的字段保持不变；选择「不修改」的下拉项保持不变。</p>
     <div class="field-grid">
         <label class="field"><span>类型</span>
           <select id="be-kind"><option value="">不修改</option>
             ${KINDS.map(([v, l]) => `<option value="${v}">${l}</option>`).join('')}
           </select></label>
       <label class="field"><span>密码方案</span>
         <select id="be-profile"><option value="">不修改</option>${profileOptions}
           <option value="__none__">（不加密）</option>
         </select></label>
       <label class="field"><span>艺人</span><input id="be-artist" /></label>
       <label class="field"><span>厂牌</span><input id="be-label" /></label>
       <label class="field"><span>出版方</span><input id="be-publisher" /></label>
       <label class="field"><span>发行日期</span><input id="be-date" /></label>
       <label class="field"><span>评级</span><input id="be-rank" /></label>
       <label class="field"><span>所属作品</span><input id="be-work" /></label>
       <label class="field"><span>分类</span><input id="be-category" /></label>
       <label class="field"><span>标签（追加，逗号分隔）</span><input id="be-tags" /></label>
     </div>
     <label class="check-row"><input type="checkbox" id="be-notes" /><span>同时为这些条目生成说明文件</span></label>`,
    [
      { label: '取消', onClick: (close) => close() },
      {
        label: '应用',
        primary: true,
        onClick: (close) => {
          const v = (id: string): string =>
            (el<HTMLInputElement>(id).value ?? '').trim();
          const kind = v('be-kind');
          const profile = v('be-profile');
          const artist = v('be-artist');
          const label = v('be-label');
          const publisher = v('be-publisher');
          const date = v('be-date');
          const rank = v('be-rank');
          const work = v('be-work');
          const category = v('be-category');
          const tags = v('be-tags')
            .split(/[、,，]/)
            .map((s) => s.trim())
            .filter(Boolean);
          const withNotes = el<HTMLInputElement>('be-notes').checked;

          for (const e of sel) {
            if (kind) {
              e.kind = kind;
              e.needs_review = false;
            }
            if (profile) e.password_profile = profile === '__none__' ? null : profile;
            if (artist) e.artist = artist;
            if (label) e.label = label;
            if (publisher) e.publisher = publisher;
            if (date) e.date = date;
            if (rank) e.rank = rank;
            if (work) e.work = work;
            if (category) e.category = category;
            if (tags.length) e.tags = [...new Set([...e.tags, ...tags])];
          }
          markDirty();
          close();
          render();
          toast(`已更新 ${sel.length} 条`);
          if (withNotes) void doGenerateNotes();
        },
      },
    ],
  );
}

function openExportSettings(): void {
  const p = project();
  readArchiveForm(p);
  openModal(
    '导出设置',
    `<div class="field-grid">
       <label class="field wide"><span>输出目录</span><input id="es-output" value="${p.output_root}" placeholder="D:\\18138-分享压缩包" /></label>
       <label class="field"><span>格式</span>
         <select id="es-format">
           <option value="rar" ${p.archive.format === 'rar' ? 'selected' : ''}>RAR5（WinRAR）</option>
           <option value="zip" ${p.archive.format === 'zip' ? 'selected' : ''}>ZIP（内置）</option>
         </select></label>
       <label class="field"><span>分卷 (MiB)</span><input id="es-volume" type="number" min="0" value="${p.archive.volume_mib}" /></label>
     </div>
     <label class="check-row"><input type="checkbox" id="es-comment" ${p.archive.comment ? 'checked' : ''} /><span>写入压缩注释（= 说明文件内容）</span></label>
     <label class="check-row"><input type="checkbox" id="es-encrypt" ${p.archive.encrypt_names ? 'checked' : ''} /><span>加密文件名（-hp，文件名含 [密码] 标签）</span></label>
     <label class="check-row"><input type="checkbox" id="es-overwrite" ${p.archive.overwrite ? 'checked' : ''} /><span>覆盖已存在的压缩包</span></label>
     <label class="check-row"><input type="checkbox" id="es-incremental" ${p.archive.incremental ? 'checked' : ''} /><span>增量跳过未变动单位（按导出清单）</span></label>
     <label class="check-row"><input type="checkbox" id="es-notes" ${p.archive.write_notes ? 'checked' : ''} /><span>导出前先生成说明文件</span></label>
     <p class="hint" id="es-format-hint"></p>
     <p class="hint">输出路径 = 输出目录 + 条目相对路径 + 单位名[密码].rar（与源结构一致）</p>`,
    [
      { label: '取消', onClick: (close) => close() },
      {
        label: '应用',
        primary: true,
        onClick: (close) => {
          p.output_root = el<HTMLInputElement>('es-output').value.trim();
          p.archive.format = el<HTMLSelectElement>('es-format').value;
          p.archive.volume_mib = Math.max(0, Number(el<HTMLInputElement>('es-volume').value) || 0);
          p.archive.comment = el<HTMLInputElement>('es-comment').checked;
          p.archive.encrypt_names = el<HTMLInputElement>('es-encrypt').checked;
          p.archive.overwrite = el<HTMLInputElement>('es-overwrite').checked;
          p.archive.incremental = el<HTMLInputElement>('es-incremental').checked;
          p.archive.write_notes = el<HTMLInputElement>('es-notes').checked;
            el<HTMLInputElement>('output-path').value = p.output_root;
            syncArchiveForm(p);
            markDirty();
            close();
            toast('导出设置已更新');
        },
      },
    ],
  );

  // ZIP 不支持分卷 / 加密
  const applyFmt = (): void => {
    const zip = el<HTMLSelectElement>('es-format').value === 'zip';
    el<HTMLInputElement>('es-volume').disabled = zip;
    el<HTMLInputElement>('es-encrypt').disabled = zip;
    el('es-format-hint').textContent = zip ? 'ZIP 使用内置压缩：不支持分卷与加密。' : '';
  };
  el<HTMLSelectElement>('es-format').addEventListener('change', applyFmt);
  applyFmt();
}

// ---------------------------------------------------------------- 密码方案

function addPassword(): void {
  const p = project();
  const pw = el<HTMLInputElement>('new-password-input').value.trim();
  const usage = el<HTMLInputElement>('new-usage-input').value.trim();
  if (!pw) {
    toast('请输入密码');
    return;
  }
  const active = p.password_profiles.find((x) => x.id === state.activeProfileId);
  if (active) {
    if (active.passwords.includes(pw)) {
      toast('该方案已包含此密码');
      return;
    }
    active.passwords.push(pw);
    toast(`已追加到「${active.usage || active.id}」`);
  } else {
    const profile: PasswordProfile = {
      id: `pw-${Date.now().toString(36)}`,
      usage: usage || `方案${p.password_profiles.length + 1}`,
      passwords: [pw],
    };
    p.password_profiles.push(profile);
    state.activeProfileId = profile.id;
    toast(`已创建方案「${profile.usage}」`);
  }
  el<HTMLInputElement>('new-password-input').value = '';
  markDirty();
  renderPasswords(state);
  refreshDetail(state);
}

function removePassword(id: string): void {
  const p = project();
  const idx = p.password_profiles.findIndex((x) => x.id === id);
  if (idx < 0) return;
  const [removed] = p.password_profiles.splice(idx, 1);
  for (const e of p.entries) {
    if (e.password_profile === id) e.password_profile = null;
  }
  if (state.activeProfileId === id) state.activeProfileId = null;
  markDirty();
  renderPasswords(state);
  refreshDetail(state);
  toast(`已删除方案「${removed.usage || removed.id}」`);
}

// ---------------------------------------------------------------- 进度事件

const noteStatusByRel = new Map<string, string>();

async function wireEvents(): Promise<void> {
  await onNotesProgress((p) => {
    if (p.phase === 'finished') {
      const proj = state.project;
      if (proj) {
        let changed = false;
        for (const [rel, status] of noteStatusByRel) {
          const entry = findEntry(proj, rel);
          if (entry && status === 'created' && !entry.has_notes) {
            entry.has_notes = true;
            changed = true;
          }
        }
        if (changed) markDirty();
      }
      noteStatusByRel.clear();
      setBusy(null);
      setProgress(100, `说明文件完成：写入 ${p.written} · 失败 ${p.failed}`);
      log(`说明文件完成：写入 ${p.written} · 失败 ${p.failed} · ${p.status}`, p.failed ? 'error' : 'ok');
      render();
      toast('说明文件生成完成');
      return;
    }
    if (p.status === 'created' || p.status === 'error') {
      noteStatusByRel.set(p.rel_path, p.status);
      if (p.status === 'error') log(`说明文件失败 ${p.rel_path}：${p.message}`, 'error');
    }
    const pct = p.total ? ((p.index + 1) / p.total) * 100 : 0;
    setProgress(pct, `生成说明文件 ${p.index + 1}/${p.total}`);
    el('unit-progress').textContent = notesStatus(p);
  });

  await onNoteAuditProgress((p) => {
    if (p.phase === 'finished') {
      finishAudit(p);
      return;
    }
    if (p.item) auditItems.push(p.item);
    const bar = document.getElementById('audit-bar');
    const idx = document.getElementById('audit-idx');
    const cur = document.getElementById('audit-cur');
    if (bar && p.total) bar.style.width = `${((p.index + 1) / p.total) * 100}%`;
    if (idx && p.total) idx.textContent = String(Math.min(p.index + 1, p.total));
    if (cur) cur.textContent = p.item ? p.item.rel_path : p.rel_path;
  });

  await onDupesProgress((p) => {
    if (p.phase === 'finished') {
      finishDupes(p);
      return;
    }
    const track = document.getElementById('dupes-track');
    const bar = document.getElementById('dupes-bar');
    const idx = document.getElementById('dupes-idx');
    const cur = document.getElementById('dupes-cur');
    if (p.phase === 'hash') {
      track?.classList.remove('indeterminate');
      if (bar && p.total) bar.style.width = `${(p.index / p.total) * 100}%`;
      if (idx) idx.textContent = `校验 ${p.index} / ${p.total}`;
    } else {
      track?.classList.add('indeterminate');
      if (idx) idx.textContent = `已扫描 ${p.index} 个文件`;
    }
    if (cur) cur.textContent = p.rel_path;
  });

  await onVerifyProgress((p) => {
    if (p.phase === 'finished') {
      finishVerify(p);
      return;
    }
    if (!verifyOpen) return;
    const bar = document.getElementById('verify-bar');
    const idx = document.getElementById('verify-idx');
    const total = document.getElementById('verify-total');
    const cur = document.getElementById('verify-cur');
    if (bar && p.total) bar.style.width = `${(p.index / p.total) * 100}%`;
    if (idx) idx.textContent = String(Math.min(p.index + 1, p.total || p.index + 1));
    if (total && p.total) total.textContent = String(p.total);
    if (cur) cur.textContent = p.rel_path;
  });

  await onExportProgress((p) => {
    const proj = state.project;
    if (p.phase === 'finished') {
      setBusy(null);
      setProgress(p.global_percent || 100, `导出完成：新建 ${p.created} · 跳过 ${p.skipped} · 失败 ${p.failed}`);
      log(
        `导出完成：新建 ${p.created} · 跳过 ${p.skipped} · 失败 ${p.failed} · ${p.status}`,
        p.failed ? 'error' : 'ok',
      );
      if (proj) {
        for (const [rel, status] of exportStatusByRel) {
          const entry = findEntry(proj, rel);
          if (entry) entry.status = status;
        }
      }
      exportStatusByRel.clear();
      render();
      toast(`导出完成：${p.created} 新建 · ${p.skipped} 跳过 · ${p.failed} 失败`);
      return;
    }

    if (p.phase === 'manifest') {
      const pct = p.total ? Math.min(100, (p.index / p.total) * 100) : 100;
      setProgress(pct, p.message || `导出清单 ${p.index}/${p.total}`);
      el('unit-progress').textContent = p.message || p.rel_path;
      if (p.status === 'error' || p.index >= p.total) {
        log(p.message || `导出清单：${p.rel_path}`, p.status === 'error' ? 'error' : 'ok');
        if (p.status === 'error') toast(p.message, 4000);
      }
      return;
    }

    if (p.phase === 'unit' && (p.status === 'created' || p.status === 'skipped' || p.status === 'error')) {
      exportStatusByRel.set(p.rel_path, p.status === 'created' ? 'done' : p.status === 'error' ? 'error' : 'skipped');
      if (p.status === 'error') log(`导出失败 ${p.rel_path}：${p.message}`, 'error');
      else if (p.status === 'skipped' && p.message)
        log(`已跳过：${p.rel_path}（${p.message}）`, 'info');
      else log(`${p.status === 'created' ? '已创建' : '已跳过'}：${p.rel_path}`, p.status === 'created' ? 'ok' : 'info');
      if (proj) {
        const entry = findEntry(proj, p.rel_path);
        if (entry) entry.status = p.status === 'created' ? 'done' : p.status === 'error' ? 'error' : 'skipped';
        renderTable(state);
        renderStats(state);
      }
    }
    setProgress(p.global_percent, `导出 ${p.index + 1}/${p.total}`);
    el('unit-progress').textContent = exportStatus(p);
  });
}

const exportStatusByRel = new Map<string, string>();

// ---------------------------------------------------------------- DOM 接线

function wireDom(): void {
  // 扫描 / 路径
  el('scan-btn').addEventListener('click', () => void doScan());
  el('pick-root-btn').addEventListener('click', async () => {
    const dir = await openDialog({ directory: true, multiple: false });
    if (typeof dir === 'string') el<HTMLInputElement>('root-path').value = dir;
  });
  el('pick-output-btn').addEventListener('click', async () => {
    const dir = await openDialog({ directory: true, multiple: false });
    if (typeof dir === 'string') {
      el<HTMLInputElement>('output-path').value = dir;
      project().output_root = dir;
      markDirty();
    }
  });
  el<HTMLInputElement>('root-path').addEventListener('change', () => {
    const next = el<HTMLInputElement>('root-path').value.trim();
    const prev = project().root;
    project().root = next;
    markDirty();
    // 根目录变更 → 已有识别结果全部失效
    if (state.tree.length && next !== prev) {
      clearScanResults();
      render();
      toast('根目录已变更，请重新扫描');
      log(`根目录变更：${prev || '（空）'} → ${next || '（空）'}，已清空识别结果`);
    }
  });
  el<HTMLInputElement>('output-path').addEventListener('change', () => {
    project().output_root = el<HTMLInputElement>('output-path').value.trim();
    markDirty();
  });

  // 项目
  el('project-new-btn').addEventListener('click', () => void newProject());
  el('project-open-btn').addEventListener('click', () => void openProjectFile());
  el('project-save-btn').addEventListener('click', () => void saveProjectFile());
  el('settings-btn').addEventListener('click', () =>
    openSettings({ applyArchiveDefaultsToProject: applyArchiveDefaults }),
  );

  // 任务
  el('notes-btn').addEventListener('click', () => void doGenerateNotes());
  el('export-btn').addEventListener('click', () => void doExport());
  el('export-cancel-btn').addEventListener('click', () => void doCancel());
  el('export-settings-btn').addEventListener('click', () => openExportSettings());

  // 过滤
  el<HTMLInputElement>('entry-filter').addEventListener('input', (e) => {
    state.filters.entryQuery = (e.target as HTMLInputElement).value;
    renderTable(state);
    renderStats(state);
  });
  el<HTMLSelectElement>('kind-filter').addEventListener('change', (e) => {
    state.filters.kind = (e.target as HTMLSelectElement).value;
    renderTable(state);
  });
  el<HTMLSelectElement>('status-filter').addEventListener('change', (e) => {
    state.filters.status = (e.target as HTMLSelectElement).value;
    renderTable(state);
  });
  el<HTMLInputElement>('tree-filter').addEventListener('input', (e) => {
    state.filters.treeQuery = (e.target as HTMLInputElement).value;
    renderTree(state);
  });
  el('tree-expand-all-btn').addEventListener('click', () => {
    const walk = (nodes: DetectNode[]): void => {
      for (const n of nodes) {
        if (!n.children.length) continue;
        state.expanded.add(n.rel_path);
        walk(n.children);
      }
    };
    walk(state.tree);
    renderTree(state);
  });
  el('tree-collapse-all-btn').addEventListener('click', () => {
    state.expanded.clear();
    renderTree(state);
  });
  el('filter-toggle').addEventListener('click', () => toggleAdvancedFilter());
  el<HTMLSelectElement>('year-filter').addEventListener('change', (e) => {
    state.filters.year = (e.target as HTMLSelectElement).value;
    applyFilters();
  });
  el<HTMLSelectElement>('tag-filter').addEventListener('change', (e) => {
    state.filters.tag = (e.target as HTMLSelectElement).value;
    applyFilters();
  });
  el<HTMLSelectElement>('profile-filter').addEventListener('change', (e) => {
    state.filters.profile = (e.target as HTMLSelectElement).value;
    applyFilters();
  });
  el<HTMLInputElement>('size-min').addEventListener('input', (e) => {
    state.filters.sizeMin = (e.target as HTMLInputElement).value;
    applyFilters();
  });
  el<HTMLInputElement>('size-max').addEventListener('input', (e) => {
    state.filters.sizeMax = (e.target as HTMLInputElement).value;
    applyFilters();
  });
  el('filter-clear').addEventListener('click', () => clearAllFilters());

  // 选择
  el('select-all-btn').addEventListener('click', () => {
    for (const e of visibleEntries(state)) e.selected = true;
    renderTable(state);
    renderStats(state);
  });
  el('select-none-btn').addEventListener('click', () => {
    for (const e of visibleEntries(state)) e.selected = false;
    renderTable(state);
    renderStats(state);
  });
  el('entry-check-all').addEventListener('change', (e) => {
    const checked = (e.target as HTMLInputElement).checked;
    for (const row of visibleEntries(state)) row.selected = checked;
    renderTable(state);
    renderStats(state);
  });
  el('batch-edit-btn').addEventListener('click', () => openBatchEdit());
  el('autofill-btn').addEventListener('click', () => void openAutofill(null));
  el('classify-btn').addEventListener('click', () => openClassify());
  el('log-clear-btn').addEventListener('click', () => clearLog());

  // 排序
  el('entry-table')
    .querySelectorAll<HTMLElement>('thead th[data-sort]')
    .forEach((th) => {
      th.addEventListener('click', () => {
        const key = th.dataset.sort as SortState['key'];
        if (state.sort.key === key) state.sort.dir = state.sort.dir === 1 ? -1 : 1;
        else {
          state.sort.key = key;
          state.sort.dir = 1;
        }
        renderTable(state);
      });
    });

  // 表格交互：单击选当前行 · Ctrl/⌘ 勾选 · Shift 范围选 · 双击进详情
  const sortedRows = (): Entry[] => sortEntries(visibleEntries(state), state.sort);

  el('entry-tbody').addEventListener('click', (e) => {
    const target = e.target as HTMLElement;
    // 空状态引导按钮
    const es = target.closest('[data-es]') as HTMLElement | null;
    if (es) {
      if (es.dataset.es === 'new') void newProject();
      else if (es.dataset.es === 'open') void openProjectFile();
      else if (es.dataset.es === 'clear') clearAllFilters();
      return;
    }
    const rows = sortedRows();
    if (target instanceof HTMLInputElement && target.classList.contains('row-check')) {
      const idx = rows.findIndex((r) => r.rel_path === target.dataset.rel);
      if (idx >= 0) {
        const anchorIdx = lastRowRel !== null ? rows.findIndex((r) => r.rel_path === lastRowRel) : -1;
        if (e.shiftKey && anchorIdx >= 0) {
          const a = Math.min(anchorIdx, idx);
          const b = Math.max(anchorIdx, idx);
          for (const r of rows.slice(a, b + 1)) r.selected = target.checked;
        } else {
          rows[idx].selected = target.checked;
        }
        lastRowRel = rows[idx].rel_path;
      }
      renderTable(state);
      renderStats(state);
      return;
    }
    const row = target.closest('tr[data-rel]');
    if (!row) return;
    const rel = row.getAttribute('data-rel') ?? '';
    const idx = rows.findIndex((r) => r.rel_path === rel);
    const anchorIdx = lastRowRel !== null ? rows.findIndex((r) => r.rel_path === lastRowRel) : -1;
    if (e.ctrlKey || e.metaKey) {
      if (idx >= 0) rows[idx].selected = !rows[idx].selected;
      renderStats(state);
    } else if (e.shiftKey && anchorIdx >= 0 && idx >= 0) {
      const a = Math.min(anchorIdx, idx);
      const b = Math.max(anchorIdx, idx);
      for (const r of rows.slice(a, b + 1)) r.selected = true;
      renderStats(state);
    }
    if (idx >= 0) lastRowRel = rel;
    state.selectedRel = rel;
    renderTable(state);
    refreshDetail(state);
    if (state.selectedRel) focusUnit(state.selectedRel);
  });

  el('entry-tbody').addEventListener('dblclick', (e) => {
    const target = e.target as HTMLElement;
    if (target.closest('[data-es]') || target.closest('input')) return;
    const row = target.closest('tr[data-rel]');
    if (!row) return;
    focusDetailEdit(row.getAttribute('data-rel') ?? '');
  });

  // 表格键盘导航：↑↓/Home/End 移动当前行 · Space 勾选 · Enter/双击进详情
  document.addEventListener('keydown', (e) => {
    if (!['ArrowUp', 'ArrowDown', 'Home', 'End', ' ', 'Enter'].includes(e.key)) return;
    const ae = document.activeElement as HTMLElement | null;
    const tag = ae?.tagName;
    // 行勾选框获得焦点时仍允许 ↑↓（Space 交给原生勾选）
    const isRowCheck = !!ae?.classList.contains('row-check');
    if ((tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') && !isRowCheck) return;
    if (tag === 'BUTTON' || tag === 'A') return;
    if (ae?.isContentEditable) return;
    if (!el('modal-overlay').classList.contains('hidden')) return;
    if (!el('palette-overlay').classList.contains('hidden')) return;
    if (!el('search-overlay').classList.contains('hidden')) return;
    const rows = sortedRows();
    if (!rows.length) return;
    let idx = rows.findIndex((r) => r.rel_path === state.selectedRel);
    const nav = (next: number): void => {
      idx = Math.max(0, Math.min(rows.length - 1, next));
      state.selectedRel = rows[idx].rel_path;
      lastRowRel = rows[idx].rel_path;
      renderTable(state);
      refreshDetail(state);
      document.querySelector('#entry-tbody tr.selected')?.scrollIntoView({ block: 'nearest' });
    };
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      nav(idx < 0 ? 0 : idx + 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      nav(idx < 0 ? 0 : idx - 1);
    } else if (e.key === 'Home') {
      e.preventDefault();
      nav(0);
    } else if (e.key === 'End') {
      e.preventDefault();
      nav(rows.length - 1);
    } else if (e.key === ' ') {
      if (idx < 0 || isRowCheck) return; // 勾选框焦点时 Space 交给原生
      e.preventDefault();
      rows[idx].selected = !rows[idx].selected;
      renderTable(state);
      renderStats(state);
    } else if (e.key === 'Enter') {
      if (idx < 0) return;
      e.preventDefault();
      focusDetailEdit(rows[idx].rel_path);
    }
  });

  // 目录树交互
  el('tree-view').addEventListener('click', (e) => {
    const target = e.target as HTMLElement;
    const node = target.closest('.tree-node') as HTMLElement | null;
    if (!node) return;
    const path = node.dataset.path ?? '';
    if (target.classList.contains('twisty')) {
      if (state.expanded.has(path)) state.expanded.delete(path);
      else state.expanded.add(path);
      renderTree(state);
      return;
    }
    if (node.dataset.unit === '1') {
      const entry = state.project?.entries.find((x) => x.rel_path === path);
      if (entry && !matchesVisible(entry)) {
        // 被任意筛选（含高级筛选）隐藏 → 全量清空后跳转，保证点得到就看得见
        clearAllFilters();
      }
      state.selectedRel = path;
      state.selectedNodeRel = path;
      renderTable(state);
      refreshDetail(state);
      renderTreeOps(state);
      focusUnit(path);
      const row = el('entry-tbody').querySelector<HTMLElement>(`tr[data-rel="${CSS.escape(path)}"]`);
      row?.scrollIntoView({ block: 'nearest' });
    } else {
      if (state.expanded.has(path)) state.expanded.delete(path);
      else state.expanded.add(path);
      state.selectedNodeRel = path;
      renderTree(state);
      renderTreeOps(state);
    }
  });

  // 详情表单
  el('detail-form').addEventListener('change', (e) => {
    const target = e.target as HTMLElement;
    const field = target.dataset.field;
    if (!field) return;
    const entry = findEntry(state.project, state.selectedRel);
    if (!entry) return;
    const value = (target as HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement).value;

    if (field === 'tags') {
      entry.tags = value
        .split(/[、,，]/)
        .map((s) => s.trim())
        .filter(Boolean);
    } else if (field === 'password_profile') {
      entry.password_profile = value || null;
    } else {
      (entry as unknown as Record<string, unknown>)[field] = value;
      if (field === 'kind') {
        entry.needs_review = false;
        renderDetail(state);
      }
    }
    markDirty();
    renderTable(state);
    renderStats(state);
  });

  el('detail-form').addEventListener('click', (e) => {
    const target = e.target as HTMLElement;
    if (target.closest('#detail-autofill-btn')) {
      const entry = findEntry(state.project, state.selectedRel);
      if (entry) void openAutofill([entry]);
      return;
    }
    if (target.closest('#detail-rename-btn')) {
      const entry = findEntry(state.project, state.selectedRel);
      if (entry) void renameTarget(joinRoot(entry.rel_path), entry.name);
      return;
    }
    if (target.closest('#detail-preview-btn')) {
      void previewCurrentNote();
      return;
    }
    if (target.closest('#detail-split-btn')) {
      const entry = findEntry(state.project, state.selectedRel);
      if (entry) forceContainer(entry.rel_path);
    }
  });

  // 文件面板
  const fileList = el('file-list');
  fileList.addEventListener('click', (e) => {
    const row = (e.target as HTMLElement).closest('.file-row') as HTMLElement | null;
    if (!row?.dataset.path) return;
    const path = row.dataset.path;
    const idx = state.fileItems.findIndex((f) => f.path === path);
    if (e.ctrlKey || e.metaKey) {
      if (state.fileSelection.has(path)) state.fileSelection.delete(path);
      else state.fileSelection.add(path);
      lastFileIndex = idx;
    } else if (e.shiftKey && lastFileIndex !== null && lastFileIndex >= 0 && idx >= 0) {
      const a = Math.min(lastFileIndex, idx);
      const b = Math.max(lastFileIndex, idx);
      state.fileSelection = new Set(state.fileItems.slice(a, b + 1).map((f) => f.path));
    } else {
      state.fileSelection = new Set([path]);
      lastFileIndex = idx;
    }
    renderFileList(state);
  });
  fileList.addEventListener('dblclick', (e) => {
    const row = (e.target as HTMLElement).closest('.file-row') as HTMLElement | null;
    if (!row?.dataset.path) return;
    if (row.dataset.dir === '1') {
      state.cwd = row.dataset.path;
      state.fileSelection.clear();
      void refreshFiles();
    } else {
      void doOpenSelected(false);
    }
  });
  el('file-up-btn').addEventListener('click', () => {
    const parent = state.cwd ? parentDir(state.cwd) : null;
    if (!parent) {
      toast('已在根目录');
      return;
    }
    state.cwd = parent;
    state.fileSelection.clear();
    void refreshFiles();
  });
  el('file-refresh-btn').addEventListener('click', () => void refreshFiles());
  el('file-open-btn').addEventListener('click', () => void doOpenSelected(false));
  el('file-reveal-btn').addEventListener('click', () => void doOpenSelected(true));
  el<HTMLInputElement>('file-show-all').addEventListener('change', (e) => {
    state.showAllFiles = (e.target as HTMLInputElement).checked;
    void refreshFiles();
  });
  el('file-mkdir-btn').addEventListener('click', () => {
    if (!state.cwd) {
      toast('先选择目录');
      return;
    }
    openPrompt('新建文件夹', '文件夹名', '', '例如：画师作品集', async (name) => {
      try {
        await makeDirectory(project().root, `${state.cwd}\\${name}`);
        await refreshFiles();
        toast(`已创建「${name}」`);
        log(`新建文件夹：${relOf(`${state.cwd}\\${name}`)}`, 'ok');
      } catch (err) {
        toast(String(err), 4000);
      }
    });
  });
  el('file-copy-btn').addEventListener('click', () => fileTransfer('copy'));
  el('file-move-btn').addEventListener('click', () => fileTransfer('move'));
  el('file-rename-btn').addEventListener('click', () => {
    const target = [...state.fileSelection][0];
    if (!target) {
      toast('先在文件列表中选择一项');
      return;
    }
    const name = target.split(/[\\/]/).filter(Boolean).pop() ?? '';
    void renameTarget(target, name);
  });
  el('file-trash-btn').addEventListener('click', () => confirmTrash());

  // 目录树手动覆盖
  el('force-unit-btn').addEventListener('click', () => {
    const rel = state.selectedNodeRel;
    if (rel) forceUnit(rel);
  });
  el('force-container-btn').addEventListener('click', () => {
    const rel = state.selectedNodeRel;
    if (rel) forceContainer(rel);
  });
  el('merge-up-btn').addEventListener('click', () => {
    const rel = state.selectedNodeRel;
    if (rel) confirmMergeUp(rel);
  });
  el('restore-auto-btn').addEventListener('click', () => {
    const rel = state.selectedNodeRel;
    if (rel) restoreAuto(rel);
  });

  // 密码方案
  el('add-password-btn').addEventListener('click', () => addPassword());
  el<HTMLInputElement>('new-password-input').addEventListener('keydown', (e) => {
    if (e.key === 'Enter') addPassword();
  });
  el('password-list').addEventListener('click', (e) => {
    const del = (e.target as HTMLElement).closest('.pw-del') as HTMLElement | null;
    if (del) {
      removePassword(del.dataset.id ?? '');
      return;
    }
    const row = (e.target as HTMLElement).closest('.pw-row') as HTMLElement | null;
    if (row) {
      state.activeProfileId = state.activeProfileId === row.dataset.id ? null : row.dataset.id ?? null;
      renderPasswords(state);
    }
  });

  // 压缩参数
  ['archive-format', 'volume-size', 'comment-toggle', 'encrypt-names-toggle', 'overwrite-toggle', 'incremental-toggle'].forEach(
    (id) => {
      el(id).addEventListener('change', () => {
        if (state.project) readArchiveForm(state.project);
        if (id === 'archive-format') updateFormatAvailability();
        markDirty();
      });
    },
  );

  // 面板折叠（点击面板标题）
  el('workspace').addEventListener('click', (e) => {
    const title = (e.target as HTMLElement).closest('.panel-title');
    if (!title) return;
    title.parentElement?.classList.toggle('collapsed');
  });

  // 快捷键：菜单级（Ctrl+N/S/E、F5、Delete…）统一由 menu.ts 按 data-keys 分发

  // Esc 关闭弹窗（document 级注册一次；仅遮罩可见时响应）
  document.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape') return;
    const overlay = el('modal-overlay');
    if (overlay.classList.contains('hidden')) return;
    closeModal();
  });

  // 点击遮罩关闭
  el('modal-overlay').addEventListener('click', (e) => {
    if (e.target === e.currentTarget) closeModal();
  });
}

function matchesVisible(entry: Entry): boolean {
  return visibleEntries(state).some((v) => v.rel_path === entry.rel_path);
}

// ---------------------------------------------------------------- 启动

async function boot(): Promise<void> {
  const savedRoot = localStorage.getItem('vinepress.root');
  const savedOutput = localStorage.getItem('vinepress.output');
  if (savedRoot) el<HTMLInputElement>('root-path').value = savedRoot;
  if (savedOutput) el<HTMLInputElement>('output-path').value = savedOutput;

  state.project = emptyProject();
  const defaults = getPrefs().archiveDefaults;
  if (defaults) state.project.archive = { ...defaults };
  if (savedRoot) state.project.root = savedRoot;
  if (savedOutput) state.project.output_root = savedOutput;
  syncArchiveForm(state.project);

  wireDom();
  initSplitters();
  initMenubar(menuActions());
  initSearch();
  setRecentProjects(recentList());
  await wireEvents();
  clearDirty();
  void wireClosePrompt();
  render();

  try {
    tools = await probeTools();
    el('tool-status').textContent = tools.rar ? 'RAR 就绪' : '未找到 Rar.exe';
    el('tool-status').className = tools.rar ? 'ok' : 'err';
    log(tools.rar ? `RAR：${tools.rar}` : '未找到 WinRAR，导出将不可用', tools.rar ? 'ok' : 'error');
  } catch (err) {
    el('tool-status').textContent = '工具探测失败';
    log(`工具探测失败：${String(err)}`, 'error');
  }

  setProgress(0, '就绪');
}

document.addEventListener('DOMContentLoaded', () => void boot());

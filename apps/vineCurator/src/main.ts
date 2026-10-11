import {
  addItemCategories,
  addRoot,
  cancelJob,
  getDbInfo,
  getOverview,
  listCategories,
  listCollections,
  listRules,
  onAutocatProgress,
  onDupeProgress,
  onHashProgress,
  onMetaProgress,
  onOcrProgress,
  onRuleProgress,
  onScanProgress,
  probeTools,
  sessionUser,
  setItemFields,
  setItemTags,
  startAutocatJob,
  startRuleJob,
  type AuthUser,
  type AutoCatProgress,
  type DupeProgress,
  type HashProgress,
  type MetaProgress,
  type OcrProgress,
  type RuleProgress,
  type ScanProgress,
} from './api';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import {
  deleteSelectedCategory,
  moveSelectedCategory,
  newCategory,
  renameSelectedCategory,
} from './features/tree';
import { mergeTagsDialog, saveCurrentAsCollection } from './features/collections';
import { loadLayout, saveLayout, type LayoutPrefs } from './layout';
import {
  bindDeclaredShortcuts,
  commandList,
  initPalette,
  installShortcutListener,
  registerCommand,
  togglePalette,
} from './menu';
import {
  initSettings,
  refreshCollabView,
  refreshRoots,
  refreshRootsView,
  setToast,
} from './settings';
import { resetChunks, resetList, state } from './state';
import {
  focusSearch,
  initListScroll,
  initViewToggle,
  onSearchInput,
  reloadList,
  renderDetail,
  renderHeader,
  renderTree,
} from './ui';

function el<T extends HTMLElement>(id: string): T {
  return document.getElementById(id) as T;
}

const dbStatus = el<HTMLDivElement>('db-status');
const toastStack = el<HTMLDivElement>('toast');
const jobPanel = el<HTMLElement>('job-panel');
const jobFill = el<HTMLDivElement>('job-fill');
const jobStatus = el<HTMLDivElement>('job-status');
const jobCancelBtn = el<HTMLButtonElement>('job-cancel');
const jobChip = el<HTMLButtonElement>('job-chip');
const jobChipLabel = el<HTMLSpanElement>('job-chip-label');
const jobChipFill = el<HTMLSpanElement>('job-chip-fill');
const searchInput = el<HTMLInputElement>('search-input');
const viewBrowse = el<HTMLElement>('view-browse');
const viewSettings = el<HTMLElement>('view-settings');
const treePanel = el<HTMLElement>('tree-panel');
const detailPanel = el<HTMLElement>('detail-panel');
const palette = el<HTMLElement>('palette');

let layout: LayoutPrefs = loadLayout();
state.viewMode = layout.viewMode;

function toast(msg: string, ok = true): void {
  const item = document.createElement('div');
  item.className = `toast-item ${ok ? 'ok' : 'err'}`;
  item.textContent = msg;
  toastStack.appendChild(item);
  window.setTimeout(() => item.remove(), ok ? 2500 : 4500);
}
setToast(toast);

// ---------------------------------------------------------------- 视图切换

function setView(v: 'browse' | 'settings'): void {
  state.view = v;
  viewBrowse.hidden = v !== 'browse';
  viewSettings.hidden = v !== 'settings';
  document.querySelectorAll('.nav-btn').forEach((b) => {
    const cmd = (b as HTMLElement).dataset.cmd;
    b.classList.toggle('active', cmd === `view-${v}`);
  });
  if (v === 'browse') {
    renderTree();
    renderViewportSafe();
  } else {
    // 会话/用户/审计可能在浏览期变化（任务审计持续写入）
    void refreshCollabView();
  }
}

function renderViewportSafe(): void {
  // 触发一次虚拟滚动渲染（reloadList 内部已渲染）
  void reloadList();
}

// ---------------------------------------------------------------- 任务进度

function beginJob(label: string): void {
  state.jobRunning = true;
  jobPanel.hidden = false;
  jobStatus.textContent = `${label} …`;
  jobFill.style.width = '0%';
  jobFill.classList.remove('indeterminate');
  // 顶栏全局指示器（任何视图都能看到任务状态）
  jobChip.hidden = false;
  jobChip.classList.remove('done');
  jobChipLabel.textContent = `${label} …`;
  jobChipFill.style.width = '0%';
  jobChipFill.classList.remove('indeterminate');
  refreshRootsView();
}

/// pct 省略 = 总数未知 → 不定进度流光动画
function updateJob(text: string, pct?: number): void {
  jobStatus.textContent = text;
  jobChipLabel.textContent = text;
  if (pct === undefined) {
    jobFill.classList.add('indeterminate');
    jobChipFill.classList.add('indeterminate');
  } else {
    const v = Math.max(0, Math.min(100, pct));
    jobFill.classList.remove('indeterminate');
    jobFill.style.width = `${v}%`;
    jobChipFill.classList.remove('indeterminate');
    jobChipFill.style.width = `${v}%`;
  }
}

function endJob(text: string): void {
  state.jobRunning = false;
  jobStatus.textContent = text;
  jobFill.classList.remove('indeterminate');
  jobFill.style.width = '100%';
  jobChip.classList.add('done');
  jobChipLabel.textContent = text;
  jobChipFill.classList.remove('indeterminate');
  jobChipFill.style.width = '100%';
  refreshRootsView();
  window.setTimeout(() => {
    if (!state.jobRunning) {
      jobPanel.hidden = true;
      jobChip.hidden = true;
    }
  }, 3000);
}

async function refreshAll(): Promise<void> {
  state.overview = await getOverview(null);
  state.categories = await listCategories();
  state.collections = await listCollections();
  renderTree();
  resetChunks(); // 扫描/规则/打标后列表必须失效重拉（否则 chunks 命中早退，视图不更新）
  await reloadList();
}

window.addEventListener('vc:begin-job', (e) => {
  beginJob((e as CustomEvent<string>).detail);
});
window.addEventListener('vc:end-job', (e) => {
  endJob((e as CustomEvent<string>).detail);
});
window.addEventListener('vc:roots-changed', () => {
  void refreshAll();
});
window.addEventListener('vc:toast', (e) => {
  const [msg, ok] = (e as CustomEvent<[string, boolean?]>).detail;
  toast(msg, ok ?? true);
});
window.addEventListener('vc:goto-roots', () => {
  setView('settings');
  window.setTimeout(() => {
    document.getElementById('roots-panel')?.scrollIntoView({ behavior: 'smooth', block: 'start' });
    el<HTMLButtonElement>('root-browse').focus();
  }, 60);
});
// 顶栏任务指示器：点击跳设置页任务面板
jobChip.addEventListener('click', () => {
  setView('settings');
  window.setTimeout(() => {
    document.getElementById('job-panel')?.scrollIntoView({ behavior: 'smooth', block: 'center' });
  }, 60);
});
window.addEventListener('vc:user-changed', () => {
  void sessionUser()
    .then(renderUserChip)
    .catch(() => undefined);
});
// 视图模式（列表/网格）持久化
window.addEventListener('vc:view-mode', (e) => {
  layout.viewMode = (e as CustomEvent<'list' | 'grid'>).detail;
  saveLayout(layout);
});

function renderUserChip(u: AuthUser | null): void {
  const chip = el<HTMLButtonElement>('user-chip');
  chip.textContent = u ? `${u.username} · ${u.role}` : '未登录';
  chip.classList.toggle('logged', u !== null);
  chip.title = u ? '已登录，点击查看用户设置' : '本地模式，点击登录 / 创建管理员';
}

async function onScanProgressEvent(p: ScanProgress): Promise<void> {
  if (p.status === 'running') {
    const label = p.phase === 'walk' ? '扫描' : '收尾';
    const idx = p.total > 0 ? `${p.index}/${p.total}` : String(p.index);
    const pct = p.total > 0 ? (p.index / p.total) * 100 : undefined;
    updateJob(`${label} ${idx} ${p.rel_path}`.trim(), pct);
  } else if (p.status === 'done' || p.status === 'cancelled') {
    endJob(p.message);
    await refreshRoots();
    await refreshAll();
    if (p.status === 'done') scheduleAutoClassify();
  } else if (p.status === 'error') {
    endJob(`错误：${p.message}`);
  }
}

async function onHashProgressEvent(p: HashProgress): Promise<void> {
  if (p.status === 'running') {
    const pct = p.total > 0 ? (p.index / p.total) * 100 : undefined;
    updateJob(`哈希 ${p.index}/${p.total} ${p.rel_path}`.trim(), pct);
  } else if (p.status === 'done' || p.status === 'cancelled') {
    endJob(p.message);
  } else if (p.status === 'error') {
    if (p.phase === 'finished') endJob(`错误：${p.message}`);
    else updateJob(p.message);
  }
}

async function onDupeProgressEvent(p: DupeProgress): Promise<void> {
  if (p.status === 'running') {
    const pct = p.total > 0 ? (p.index / p.total) * 100 : undefined;
    updateJob(`部分哈希 ${p.index}/${p.total} ${p.rel_path}`.trim(), pct);
  } else if (p.status === 'done' || p.status === 'cancelled') {
    endJob(p.message);
    window.dispatchEvent(new CustomEvent('vc:dupes-done'));
  } else if (p.status === 'error') {
    endJob(`错误：${p.message}`);
  }
}

async function onOcrProgressEvent(p: OcrProgress): Promise<void> {
  if (p.status === 'running') {
    const pct = p.total > 0 ? (p.index / p.total) * 100 : undefined;
    updateJob(`OCR ${p.index}/${p.total} ${p.rel_path}`.trim(), pct);
    if (p.message) updateJob(p.message, pct);
  } else if (p.status === 'done' || p.status === 'cancelled') {
    endJob(p.message);
    window.dispatchEvent(new CustomEvent('vc:ocr-done'));
  } else if (p.status === 'error') {
    endJob(`错误：${p.message}`);
  }
}

async function onMetaProgressEvent(p: MetaProgress): Promise<void> {
  if (p.status === 'running') {
    const pct = p.total > 0 ? (p.index / p.total) * 100 : undefined;
    const label = p.phase === 'probe' ? '探测' : '导入';
    updateJob(`${label} ${p.index}/${p.total} ${p.rel_path}`.trim(), pct);
    if (p.message) updateJob(p.message, pct);
  } else if (p.status === 'done' || p.status === 'cancelled') {
    endJob(p.message);
    await refreshAll();
    if (p.status === 'done') scheduleAutoRules();
  } else if (p.status === 'error') {
    if (p.phase === 'finished') endJob(`错误：${p.message}`);
    else updateJob(p.message);
  }
}

// ---------------------------------------------------------------- 规则任务

async function onRuleProgressEvent(p: RuleProgress): Promise<void> {
  if (p.status === 'running') {
    const pct = p.total > 0 ? (p.index / p.total) * 100 : undefined;
    let text = `规则 ${p.index}/${p.total} ${p.rule_name}`.trim();
    const { rules_run, matched, tagged, categorized } = p.stats;
    if (rules_run > 0 || matched > 0 || tagged > 0 || categorized > 0) {
      text += ` · 规则 ${rules_run} · 命中 ${matched} · 打标 ${tagged} · 归类 ${categorized}`;
    }
    updateJob(text, pct);
  } else if (p.status === 'done' || p.status === 'cancelled') {
    const s = p.stats;
    endJob(
      p.status === 'cancelled'
        ? `规则已取消 · 打标 ${s.tagged} · 归类 ${s.categorized}`
        : `规则完成 · 规则 ${s.rules_run} · 命中 ${s.matched} · 打标 ${s.tagged} · 归类 ${s.categorized}`,
    );
    await refreshAll();
  } else if (p.status === 'error') {
    endJob(`错误：${p.message}`);
  }
}

async function onAutocatProgressEvent(p: AutoCatProgress): Promise<void> {
  if (p.status === 'running') {
    const s = p.stats;
    updateJob(`自动分类 ${p.message} · 归类 ${s.linked}`);
  } else if (p.status === 'done' || p.status === 'cancelled' || p.status === 'error') {
    const s = p.stats;
    endJob(
      p.status === 'error'
        ? `自动分类错误：${p.message}`
        : p.status === 'cancelled'
          ? `自动分类已取消 · 归类 ${s.linked}`
          : `自动分类完成 · 条目 ${s.items} · 归类 ${s.linked} · 移除 ${s.unlinked}`,
    );
    await refreshAll();
    if (autocatThenRules) {
      autocatThenRules = false;
      scheduleAutoRules();
    }
  }
}

let autoRulesTimer: number | null = null;
let autoClassifyTimer: number | null = null;
let autocatThenRules = false;

/// 扫描完成后：先目录镜像自动分类，结束后再跑规则（规则条件可用 category 过滤）
function scheduleAutoClassify(): void {
  if (autoClassifyTimer !== null) window.clearTimeout(autoClassifyTimer);
  autoClassifyTimer = window.setTimeout(() => {
    autoClassifyTimer = null;
    void (async () => {
      try {
        autocatThenRules = true;
        await startAutocatJob(null);
        window.dispatchEvent(new CustomEvent('vc:begin-job', { detail: '自动分类' }));
      } catch {
        // 互斥任务占用 → 直接走规则
        autocatThenRules = false;
        scheduleAutoRules();
      }
    })();
  }, 600);
}

/// 扫描/元数据导入完成后自动跑规则（延迟等 JobState 互斥释放；失败静默）
function scheduleAutoRules(): void {
  if (autoRulesTimer !== null) window.clearTimeout(autoRulesTimer);
  autoRulesTimer = window.setTimeout(() => {
    autoRulesTimer = null;
    void (async () => {
      try {
        if ((await listRules()).every((r) => !r.enabled)) return;
      } catch {
        return;
      }
      for (let attempt = 0; attempt < 2; attempt++) {
        try {
          await startRuleJob(null);
          window.dispatchEvent(new CustomEvent('vc:begin-job', { detail: '规则' }));
          return;
        } catch {
          // 互斥任务尚未释放 → 稍后重试一次
          await new Promise((r) => window.setTimeout(r, 1500));
        }
      }
    })();
  }, 600);
}

// ---------------------------------------------------------------- 面板宽度

function applyLayout(): void {
  treePanel.style.width = `${layout.treeWidth}px`;
  detailPanel.style.width = `${layout.detailWidth}px`;
}

function initSplitters(): void {
  document.querySelectorAll<HTMLElement>('.splitter').forEach((sp) => {
    sp.addEventListener('mousedown', (ev) => {
      ev.preventDefault();
      const which = sp.dataset.split;
      const startX = ev.clientX;
      const startW =
        which === 'tree'
          ? treePanel.getBoundingClientRect().width
          : detailPanel.getBoundingClientRect().width;
      const move = (e: MouseEvent): void => {
        const dx = e.clientX - startX;
        if (which === 'tree') layout.treeWidth = Math.max(140, Math.min(480, startW + dx));
        else layout.detailWidth = Math.max(200, Math.min(640, startW - dx));
        applyLayout();
      };
      const up = (): void => {
        window.removeEventListener('mousemove', move);
        window.removeEventListener('mouseup', up);
        saveLayout(layout);
      };
      window.addEventListener('mousemove', move);
      window.addEventListener('mouseup', up);
    });
  });
}

// ---------------------------------------------------------------- 命令

function registerAllCommands(): void {
  registerCommand({
    id: 'view-browse',
    label: '切换到浏览视图',
    keys: [],
    run: () => setView('browse'),
  });
  registerCommand({
    id: 'view-settings',
    label: '切换到设置视图',
    keys: [],
    run: () => setView('settings'),
  });
  registerCommand({
    id: 'focus-search',
    label: '全局搜索',
    keys: [],
    run: () => {
      setView('browse');
      focusSearch();
    },
  });
  registerCommand({
    id: 'palette',
    label: '打开命令面板',
    keys: [],
    run: () => togglePalette(),
  });
  registerCommand({
    id: 'refresh-view',
    label: '刷新当前视图',
    keys: [],
    run: () => refreshAll().catch((err) => toast(String(err), false)),
  });
  registerCommand({
    id: 'cancel-job',
    label: '取消当前任务',
    keys: [],
    run: () =>
      cancelJob()
        .then(() => toast('已请求取消'))
        .catch((err) => toast(String(err), false)),
  });
  registerCommand({
    id: 'reprobe',
    label: '重新探测外部工具',
    keys: [],
    run: () =>
      probeTools()
        .then(() => toast('已重新探测'))
        .catch((err) => toast(String(err), false)),
  });
  registerCommand({
    id: 'goto-top',
    label: '列表跳到顶部',
    keys: [],
    run: () => {
      el<HTMLDivElement>('list-scroll').scrollTo({ top: 0 });
    },
  });
  registerCommand({
    id: 'reload-list',
    label: '重新加载列表',
    keys: [],
    run: () => {
      resetList();
      void reloadList();
    },
  });
  registerCommand({
    id: 'category-new',
    label: '新建分类（选中分类下建子级）',
    keys: [],
    run: () => newCategory(refreshAll),
  });
  registerCommand({
    id: 'category-rename',
    label: '重命名选中分类',
    keys: [],
    run: () => renameSelectedCategory(refreshAll),
  });
  registerCommand({
    id: 'category-move',
    label: '移动选中分类',
    keys: [],
    run: () => moveSelectedCategory(refreshAll),
  });
  registerCommand({
    id: 'category-delete',
    label: '删除选中分类（级联）',
    keys: [],
    run: () => deleteSelectedCategory(refreshAll),
  });
  registerCommand({
    id: 'save-collection',
    label: '把当前筛选另存为智能集合',
    keys: [],
    run: () => saveCurrentAsCollection().catch((err) => toast(String(err), false)),
  });
  registerCommand({
    id: 'merge-tags',
    label: '合并标签（同义词归并）',
    keys: [],
    run: () => mergeTagsDialog().catch((err) => toast(String(err), false)),
  });
}

// ---------------------------------------------------------------- bootstrap

// ---------------------------------------------------------------- 主题（深/浅）

const THEME_KEY = 'vinecurator.prefs.theme';
const ICON_SUN =
  '<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"><circle cx="8" cy="8" r="3"/><path d="M8 1.5v1.6M8 12.9v1.6M1.5 8h1.6M12.9 8h1.6M3.4 3.4l1.1 1.1M11.5 11.5l1.1 1.1M12.6 3.4l-1.1 1.1M4.5 11.5l-1.1 1.1"/></svg>';
const ICON_MOON =
  '<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M13.5 9.6A5.8 5.8 0 0 1 6.4 2.5a5.8 5.8 0 1 0 7.1 7.1Z"/></svg>';

function applyTheme(t: 'dark' | 'light'): void {
  document.documentElement.dataset.theme = t;
  const btn = el<HTMLButtonElement>('theme-toggle');
  // 显示「将切换到」的主题图标
  btn.innerHTML = t === 'dark' ? ICON_SUN : ICON_MOON;
  btn.title = t === 'dark' ? '切换到浅色主题' : '切换到深色主题';
}

function initTheme(): void {
  let saved: string | null = null;
  try {
    saved = localStorage.getItem(THEME_KEY);
  } catch {
    /* localStorage 不可用（隐私模式等）→ 回落系统偏好 */
  }
  const initial: 'dark' | 'light' =
    saved === 'light' || saved === 'dark'
      ? saved
      : window.matchMedia('(prefers-color-scheme: light)').matches
        ? 'light'
        : 'dark';
  applyTheme(initial);
  el<HTMLButtonElement>('theme-toggle').addEventListener('click', () => {
    const next = document.documentElement.dataset.theme === 'light' ? 'dark' : 'light';
    applyTheme(next);
    try {
      localStorage.setItem(THEME_KEY, next);
    } catch {
      /* 忽略写入失败 */
    }
  });
}

// 拖放文件夹 → 直接添加媒体根目录（Tauri OS 拖放事件；overlay 提示可放置）
function initDropTargets(): void {
  const overlay = el<HTMLElement>('drop-overlay');
  try {
    void getCurrentWebview()
      .onDragDropEvent((ev) => {
        const p = ev.payload;
        if (p.type === 'enter' || p.type === 'over') {
          if (overlay.hidden) overlay.hidden = false;
        } else if (p.type === 'leave') {
          overlay.hidden = true;
        } else if (p.type === 'drop') {
          overlay.hidden = true;
          void handleDroppedPaths(p.paths);
        }
      })
      .catch(() => undefined); // 浏览器 dev 环境无 OS 拖放事件
  } catch {
    /* 浏览器 dev 环境 getCurrentWebview 可能同步抛错 */
  }
}

async function handleDroppedPaths(paths: string[]): Promise<void> {
  if (paths.length === 0) return;
  let ok = 0;
  let firstErr = '';
  for (const p of paths) {
    try {
      await addRoot(p, null);
      ok += 1;
    } catch (err) {
      if (!firstErr) firstErr = String(err);
    }
  }
  if (ok > 0) {
    toast(`已添加 ${ok} 个根目录`);
    await refreshRoots();
    window.dispatchEvent(new CustomEvent('vc:roots-changed'));
  }
  if (ok === 0 && firstErr) toast(firstErr, false);
}

async function bootstrap(): Promise<void> {
  initTheme();
  applyLayout();
  initSplitters();
  renderHeader();
  initListScroll();
  initViewToggle();
  initDropTargets();

  try {
    const dbInfo = await getDbInfo();
    dbPathEl().textContent = dbInfo.path;
    dbStatus.textContent = '已连接';
    dbStatus.classList.add('ok');
  } catch (err) {
    dbStatus.textContent = '初始化失败';
    dbStatus.classList.add('err');
    toast(String(err), false);
  }

  await initSettings();
  await refreshRoots();
  void sessionUser()
    .then(renderUserChip)
    .catch(() => undefined);

  registerAllCommands();
  initPalette();
  const declared = bindDeclaredShortcuts();
  installShortcutListener(declared);

  // data-cmd 点击绑定（导航 + 额外命令源）
  document.querySelectorAll<HTMLElement>('[data-cmd]').forEach((b) => {
    b.addEventListener('click', () => {
      const cmdId = b.dataset.cmd;
      // settings.ts 自行绑定的 reprobe 按钮跳过
      if (cmdId === 'reprobe' && b.id === 'reprobe') return;
      const c = commandList().find((x) => x.id === cmdId);
      if (c) void c.run();
    });
  });

  searchInput.addEventListener('input', () => onSearchInput(searchInput.value));
  searchInput.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      searchInput.value = '';
      onSearchInput('');
      searchInput.blur();
    }
  });

  // 分类树工具栏（与命令面板同源：命令已注册，这里只接点击）
  const catBtn = (id: string, cmdId: string): void => {
    el<HTMLButtonElement>(id).addEventListener('click', () => {
      const c = commandList().find((x) => x.id === cmdId);
      if (c) void c.run();
    });
  };
  catBtn('cat-new', 'category-new');
  catBtn('cat-rename', 'category-rename');
  catBtn('cat-move', 'category-move');
  catBtn('cat-delete', 'category-delete');

  // 批量打标条
  const batchBar = el<HTMLElement>('batch-bar');
  const batchCount = el<HTMLElement>('batch-count');
  const batchTags = el<HTMLInputElement>('batch-tags');
  window.addEventListener('vc:selection-changed', () => {
    const n = state.selectedIds.size;
    batchBar.hidden = n < 2;
    if (n >= 2) batchCount.textContent = `已选 ${n} 条`;
  });
  el<HTMLButtonElement>('batch-tag-clear').addEventListener('click', () => {
    state.selectedIds.clear();
    state.selectedId = null;
    batchBar.hidden = true;
    void reloadList();
  });
  // 批量设状态（多选 → 一次写入，行本地修补 + 树计数刷新）
  el<HTMLButtonElement>('batch-status-apply').addEventListener('click', () => {
    const v = el<HTMLSelectElement>('batch-status').value;
    if (!v || state.selectedIds.size === 0) return;
    const ids = [...state.selectedIds];
    void setItemFields(ids, { status: v })
      .then((n) => {
        toast(`已更新 ${n} 条状态`);
        el<HTMLSelectElement>('batch-status').value = '';
        for (const rows of state.chunks.values()) {
          for (const r of rows) if (state.selectedIds.has(r.id)) r.status = v;
        }
        void reloadList();
        void getOverview(null).then((o) => {
          state.overview = o;
          renderTree();
        });
        void renderDetail(state.selectedId);
      })
      .catch((err) => toast(String(err), false));
  });
  el<HTMLButtonElement>('batch-cat-apply').addEventListener('click', () => {
    const path = el<HTMLInputElement>('batch-cat').value.trim();
    if (!path || state.selectedIds.size === 0) return;
    const ids = [...state.selectedIds];
    void addItemCategories(ids, path)
      .then((n) => {
        toast(n > 0 ? `已归入「${path}」· 新增 ${n} 条关联` : `已在「${path}」中`);
        el<HTMLInputElement>('batch-cat').value = '';
        window.dispatchEvent(new CustomEvent('vc:roots-changed'));
        void renderDetail(state.selectedId);
      })
      .catch((err) => toast(String(err), false));
  });
  el<HTMLButtonElement>('batch-tag-apply').addEventListener('click', () => {
    const names = batchTags.value
      .split(/[,，]/)
      .map((s) => s.trim())
      .filter(Boolean);
    if (names.length === 0 || state.selectedIds.size === 0) return;
    const ids = [...state.selectedIds];
    void setItemTags(ids, names)
      .then((n) => {
        toast(`已给 ${ids.length} 条打标（新增 ${n} 条关联）`);
        batchTags.value = '';
        // 打标可能改变智能集合成员 → 保留多选，只失效列表重拉
        resetChunks();
        void reloadList();
        void renderDetail(state.selectedId);
      })
      .catch((err) => toast(String(err), false));
  });

  jobCancelBtn.addEventListener('click', () => {
    void cancelJob()
      .then(() => toast('已请求取消'))
      .catch((err) => toast(String(err), false));
  });

  // 面板折叠快捷（双击分隔线）
  document.querySelectorAll<HTMLElement>('.splitter').forEach((sp) => {
    sp.addEventListener('dblclick', () => {
      if (sp.dataset.split === 'tree') {
        layout.treeWidth = layout.treeWidth > 0 ? 0 : 220;
      } else {
        layout.detailWidth = layout.detailWidth > 0 ? 0 : 300;
      }
      const panel = sp.dataset.split === 'tree' ? treePanel : detailPanel;
      panel.hidden = layout.treeWidth === 0 || layout.detailWidth === 0;
      applyLayout();
      saveLayout(layout);
    });
  });

  // 顶栏账号 chip + 设置页分区导航
  el<HTMLButtonElement>('user-chip').addEventListener('click', () => {
    setView('settings');
    window.setTimeout(() => {
      document
        .getElementById('users-panel')
        ?.scrollIntoView({ behavior: 'smooth', block: 'start' });
    }, 60);
  });
  document.querySelectorAll<HTMLButtonElement>('#settings-nav button').forEach((b) => {
    b.addEventListener('click', () => {
      document
        .getElementById(b.dataset.target ?? '')
        ?.scrollIntoView({ behavior: 'smooth', block: 'start' });
    });
  });

  await onScanProgress((p) => {
    void onScanProgressEvent(p);
  });
  await onHashProgress((p) => {
    void onHashProgressEvent(p);
  });
  await onMetaProgress((p) => {
    void onMetaProgressEvent(p);
  });
  await onRuleProgress((p) => {
    void onRuleProgressEvent(p);
  });
  await onAutocatProgress((p) => {
    void onAutocatProgressEvent(p);
  });
  await onDupeProgress((p) => {
    void onDupeProgressEvent(p);
  });
  await onOcrProgress((p) => {
    void onOcrProgressEvent(p);
  });

  await refreshAll();
}

function dbPathEl(): HTMLElement {
  return el<HTMLElement>('db-path');
}

void bootstrap();

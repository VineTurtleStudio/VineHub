import type {
  DetectNode,
  Entry,
  ExportProgress,
  FileItem,
  NotesProgress,
  PasswordProfile,
  Project,
  Proposal,
} from './api';
import {
  filtersActive,
  findEntry,
  findNode,
  formatSize,
  KINDS,
  kindLabel,
  matchesEntry,
  matchesNode,
  overrideKind,
  sortEntries,
  statusLabel,
  type AppState,
  type SortState,
} from './state';

/** 可自动填充字段（与 Rust meta::FIELDS 一致） */
export const FIELD_LABELS: [string, string][] = [
  ['title', '标题'],
  ['artist', '艺人'],
  ['date', '发行日期'],
  ['label', '唱片公司'],
  ['catalog', '规格品番'],
  ['publisher', '出版方'],
  ['rank', '评级'],
  ['tracklist', '曲目列表'],
  ['tags', '标签'],
  ['notes', '备注'],
  ['work', '所属作品'],
  ['category', '分类'],
];

// ---------------------------------------------------------------- 基础工具

export function el<T extends HTMLElement = HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`缺少元素 #${id}`);
  return node as T;
}

export function esc(value: string): string {
  return value
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

export function toast(message: string, ms = 2600): void {
  const host = el('toast');
  const err = /失败|错误|无法|被拒绝|不可/.test(message);
  const item = document.createElement('div');
  item.className = `toast-item${err ? ' err' : ''}`;
  item.textContent = message;
  host.appendChild(item);
  while (host.children.length > 3) host.removeChild(host.firstChild!);
  host.classList.remove('hidden');
  const dur = err ? Math.max(ms, 4000) : ms;
  window.setTimeout(() => {
    item.remove();
    if (!host.children.length) host.classList.add('hidden');
  }, dur);
}

// ---------------------------------------------------------------- 目录树

export function renderTree(state: AppState): void {
  const host = el('tree-view');
  const q = state.filters.treeQuery.trim();

  if (!state.tree.length) {
    host.innerHTML = '<p class="hint">尚未扫描。填写根目录后点击「扫描识别」。</p>';
    el('tree-stats').textContent = '';
    return;
  }

  const html = state.tree.map((n) => nodeHtml(n, 0, q, state)).join('');
  host.innerHTML = html;

  const units = state.tree.reduce(
    (acc, n) => {
      const r = countUnits(n);
      acc.units += r.units;
      acc.containers += r.containers;
      acc.ignored += r.ignored;
      return acc;
    },
    { units: 0, containers: 0, ignored: 0 },
  );
  el('tree-stats').textContent = `单位 ${units.units} · 容器 ${units.containers} · 忽略 ${units.ignored}`;
}

/** 目录树选中节点的手动覆盖操作条 */
export function renderTreeOps(state: AppState): void {
  const name = el('tree-ops-name');
  const unitBtn = el<HTMLButtonElement>('force-unit-btn');
  const containerBtn = el<HTMLButtonElement>('force-container-btn');
  const upBtn = el<HTMLButtonElement>('merge-up-btn');
  const autoBtn = el<HTMLButtonElement>('restore-auto-btn');
  const node = findNode(state.tree, state.selectedNodeRel);

  if (!node) {
    name.textContent = '未选择节点（点击树中的目录）';
    name.title = '';
    unitBtn.disabled = true;
    containerBtn.disabled = true;
    upBtn.disabled = true;
    autoBtn.disabled = true;
    return;
  }

  const lock = overrideKind(state.project, node.rel_path);
  const depth = node.rel_path.split('/').length;
  const parent = depth > 1 ? node.rel_path.split('/').slice(0, -1).join('/') : '';
  const lockText = lock === 'unit' ? ' · 已强制为单位' : lock === 'container' ? ' · 已强制为容器' : '';
  name.textContent = `${node.rel_path}${lockText}`;
  name.title = node.rel_path;
  unitBtn.disabled = lock === 'unit';
  containerBtn.disabled = !node.is_unit || lock === 'container';
  // 并入上级仅对单位节点有意义，且上级已是单位时无需重复
  upBtn.disabled = !node.is_unit || depth < 2 || overrideKind(state.project, parent) === 'unit';
  autoBtn.disabled = lock === null;
}

function countUnits(node: DetectNode): { units: number; containers: number; ignored: number } {
  if (node.is_unit) return { units: 1, containers: 0, ignored: 0 };
  if (!node.children.length) return { units: 0, containers: 0, ignored: 1 };
  const acc = { units: 0, containers: 1, ignored: 0 };
  for (const c of node.children) {
    const r = countUnits(c);
    acc.units += r.units;
    acc.containers += r.containers;
    acc.ignored += r.ignored;
  }
  return acc;
}

function nodeHtml(node: Parameters<typeof matchesNode>[0], depth: number, q: string, state: AppState): string {
  if (!matchesNode(node, q)) return '';

  const hasChildren = node.children.length > 0;
  const open = state.expanded.has(node.rel_path) || (q && hasChildren);
  const classes = ['tree-node'];
  if (node.is_unit) classes.push('is-unit');
  if (open) classes.push('open');
  if (node.rel_path === state.selectedNodeRel) classes.push('node-selected');

  const lock = overrideKind(state.project, node.rel_path);
  const lockBadge = lock
    ? `<span class="tree-badge lock" title="${lock === 'unit' ? '手动：强制为单位' : '手动：强制为容器'}">锁</span>`
    : '';
  const badge = node.is_unit
    ? `<span class="tree-badge">${esc(kindLabel(node.kind))}</span>${
        node.needs_review
          ? '<span class="tree-badge review" title="判定需人工确认（低置信或音视频混合）">复核</span>'
          : ''
      }`
    : '';
  const meta = node.is_unit && node.stats
    ? `<span class="tree-meta">${formatSize(node.stats.total_size)} · ${node.stats.file_count}个</span>`
    : hasChildren
      ? `<span class="tree-meta">${node.children.length}</span>`
      : '<span class="tree-meta dim">忽略</span>';

  const twisty = hasChildren ? '<span class="twisty" aria-hidden="true">▾</span>' : '<span class="twisty leaf"></span>';

  let html = `<div class="${classes.join(' ')}" data-path="${esc(node.rel_path)}" data-unit="${node.is_unit ? '1' : '0'}" style="padding-left:${8 + depth * 14}px">${twisty}<span class="tree-name">${esc(node.name)}</span>${badge}${lockBadge}${meta}</div>`;

  if (hasChildren && open) {
    html += node.children.map((c) => nodeHtml(c, depth + 1, q, state)).join('');
  }
  return html;
}

// ---------------------------------------------------------------- 表格

export function visibleEntries(state: AppState): Entry[] {
  if (!state.project) return [];
  return state.project.entries.filter((e) => matchesEntry(e, state.filters));
}

/** 高级筛选下拉与条目集合同步：选项集变化才重建；选中值失效时过滤器归零（render 每帧调用，廉价比对） */
export function syncFilterOptions(state: AppState): void {
  const years = new Set<string>();
  const tags = new Set<string>();
  const profileLabels = new Map<string, string>();
  if (state.project) {
    for (const p of state.project.password_profiles) profileLabels.set(p.id, p.usage || p.id);
    for (const e of state.project.entries) {
      const y = e.date.trim().slice(0, 4);
      if (/^\d{4}$/.test(y)) years.add(y);
      for (const t of e.tags) if (t) tags.add(t);
      if (e.password_profile && !profileLabels.has(e.password_profile)) profileLabels.set(e.password_profile, e.password_profile);
    }
  }
  const sortedYears = [...years].sort().reverse();
  const sortedTags = [...tags].sort();
  const fill = (id: string, allLabel: string, opts: [string, string][], current: string): string => {
    const sel = document.getElementById(id) as HTMLSelectElement | null;
    if (!sel) return current;
    const want: [string, string][] = [['', allLabel], ...opts];
    const same =
      sel.options.length === want.length &&
      want.every(([v, t], i) => sel.options[i].value === v && sel.options[i].textContent === t);
    const effective = want.some(([v]) => v === current) ? current : '';
    if (!same) {
      sel.innerHTML = want.map(([v, t]) => `<option value="${esc(v)}">${esc(t)}</option>`).join('');
      sel.value = effective;
    } else if (sel.value !== effective) {
      sel.value = effective;
    }
    return effective;
  };
  state.filters.year = fill('year-filter', '全部年份', sortedYears.map((y) => [y, y]), state.filters.year);
  state.filters.tag = fill('tag-filter', '全部标签', sortedTags.map((t) => [t, t]), state.filters.tag);
  const profileOpts: [string, string][] = [...profileLabels.entries()].map(([id, label]) => [id, label]);
  profileOpts.push(['__none__', '（不加密）']);
  state.filters.profile = fill('profile-filter', '全部密码方案', profileOpts, state.filters.profile);
}

export function renderTable(state: AppState): void {
  const host = el('entry-tbody');
  const rows = sortEntries(visibleEntries(state), state.sort);
  const allSelected = rows.length > 0 && rows.every((r) => r.selected);

  // 选择计数（表格工具栏）
  const counter = el('sel-counter');
  if (state.project) {
    const sel = state.project.entries.filter((e) => e.selected);
    const bytes = sel.reduce((a, e) => a + e.stats.total_size, 0);
    counter.textContent =
      rows.length !== state.project.entries.length
        ? `已选 ${sel.length}/${state.project.entries.length} · 显示 ${rows.length} · ${formatSize(bytes)}`
        : `已选 ${sel.length}/${state.project.entries.length} · ${formatSize(bytes)}`;
  } else {
    counter.textContent = '';
  }

  // 排序指示
  el('entry-table').querySelectorAll<HTMLElement>('thead th[data-sort]').forEach((th) => {
    th.classList.remove('sort-asc', 'sort-desc');
    if (th.dataset.sort === state.sort.key) {
      th.classList.add(state.sort.dir === 1 ? 'sort-asc' : 'sort-desc');
    }
  });

  if (!rows.length) {
    const proj = state.project;
    const fresh = !proj || proj.entries.length === 0;
    host.innerHTML = fresh
      ? `<tr><td colspan="8" class="empty">${proj ? '项目为空：尚未扫描到条目' : '尚未载入项目'}
           <div class="empty-actions">
             <button type="button" class="btn-sm" data-es="new">新建项目</button>
             <button type="button" class="btn-sm" data-es="open">打开项目…</button>
           </div></td></tr>`
      : `<tr><td colspan="8" class="empty">无匹配条目
           ${filtersActive(state.filters) ? '<div class="empty-actions"><button type="button" class="btn-sm" data-es="clear">清除筛选</button></div>' : ''}
         </td></tr>`;
  } else {
    host.innerHTML = rows
      .map((e) => {
        const selected = e.rel_path === state.selectedRel ? ' selected' : '';
        return `<tr data-rel="${esc(e.rel_path)}" class="${selected.trim()}">
          <td class="col-check"><input type="checkbox" class="row-check" data-rel="${esc(e.rel_path)}" ${e.selected ? 'checked' : ''} /></td>
          <td class="col-kind"><span class="badge kind-${esc(e.kind)}">${esc(kindLabel(e.kind))}</span>${
            e.needs_review
              ? '<span class="badge review" title="类型判定需人工确认（低置信或音视频混合）">复核</span>'
              : ''
          }</td>
          <td class="col-title" title="${esc(e.title || e.name)}">${esc(e.title || e.name)}${
            e.stats.subdirs.length > 1
              ? `<span class="badge subdirs" title="含 ${e.stats.subdirs.length} 个子目录，可在详情面板「拆分为子目录」让它们各自成为条目">${e.stats.subdirs.length} 子目录</span>`
              : ''
          }</td>
          <td class="col-path" title="${esc(e.rel_path)}">${esc(e.rel_path)}</td>
          <td class="col-size">${formatSize(e.stats.total_size)}</td>
          <td class="col-files">${e.stats.file_count}</td>
          <td class="col-status"><span class="badge status-${esc(e.status)}">${esc(statusLabel(e.status))}</span></td>
          <td class="col-fill"></td>
        </tr>`;
      })
      .join('');
  }

  const checkAll = el<HTMLInputElement>('entry-check-all');
  checkAll.checked = allSelected;
  checkAll.indeterminate = !allSelected && rows.some((r) => r.selected);
}

// ---------------------------------------------------------------- 详情表单

export function renderDetail(state: AppState): void {
  const host = el('detail-form');
  const entry = findEntry(state.project, state.selectedRel);
  const hint = el('detail-hint');

  if (!entry) {
    hint.textContent = '未选中';
    host.innerHTML = '<p class="hint">在左侧表格中选择一个条目进行编辑。</p>';
    return;
  }

  hint.textContent = entry.rel_path;
  const project = state.project!;

  const text = (field: string, label: string, value: string, width?: number) => `
    <label class="field">
      <span>${label}</span>
      <input type="text" data-field="${field}" value="${esc(value)}" ${width ? `style="width:${width}"` : ''} />
    </label>`;

  host.innerHTML = `
    <div id="detail-cover-box" class="detail-cover-box"><div class="detail-cover-empty">…</div></div>
    <div class="field-grid">
      <div class="field">
        <span>目录名</span>
        <span class="ro-value" title="压缩包名 = 目录名，通过下方按钮在磁盘上重命名">${esc(entry.name)}</span>
      </div>
      <div class="field">
        <span>操作</span>
        <div class="row">
          <button id="detail-rename-btn" class="btn-sm" title="在磁盘上重命名该目录，并同步条目相对路径">重命名目录…</button>
          <button id="detail-preview-btn" class="btn-sm" title="预览将要写入的说明文件内容">预览说明</button>
          <button id="detail-autofill-btn" class="btn-sm" title="扫描该单位目录，识别并填充元数据">自动填充此条</button>
        </div>
      </div>
      ${text('title', '标题', entry.title)}
      ${text('artist', '艺人', entry.artist)}
      ${text('date', '发行日期', entry.date)}
      ${text('label', '厂牌', entry.label)}
      ${text('catalog', '目录号', entry.catalog)}
      ${text('publisher', '出版方', entry.publisher)}
      ${text('rank', '评级', entry.rank)}
      ${text('work', '所属作品', entry.work)}
      ${text('category', '分类', entry.category)}
      <label class="field">
        <span>类型${
          entry.needs_review
            ? ' <span class="badge review" title="低置信或音视频混合判定，人工确认类型后自动消除">复核</span>'
            : ''
        }</span>
        <select data-field="kind">
          ${KINDS.map(
            ([k, label]) =>
              `<option value="${k}" ${k === entry.kind ? 'selected' : ''}>${label}</option>`,
          ).join('')}
        </select>
      </label>
      <label class="field">
        <span>密码方案</span>
        <select data-field="password_profile">
          <option value="">（不加密）</option>
          ${project.password_profiles
            .map(
              (p) =>
                `<option value="${esc(p.id)}" ${p.id === entry.password_profile ? 'selected' : ''}>${esc(p.usage || p.id)}</option>`,
            )
            .join('')}
        </select>
      </label>
      <label class="field">
        <span>说明文件</span>
        <span class="ro-value">${entry.has_notes ? '已生成' : '未生成'}</span>
      </label>
    </div>
    ${
      entry.stats.subdirs.length
        ? `<div class="detail-subs">
      <div class="detail-subs-head">
        <span>含 ${entry.stats.subdirs.length} 个子目录</span>
        <button id="detail-split-btn" class="btn-sm" title="强制为容器（拆分）：让每个子目录各自成为独立条目，说明文件可一目录一文件；可用「恢复自动识别」撤销">拆分为子目录…</button>
      </div>
      <ul class="detail-subs-list">${entry.stats.subdirs
        .slice(0, 12)
        .map(
          (d) =>
            `<li><span class="ds-name">${esc(d.name)}</span><span class="ds-meta">${formatSize(d.total_size)} · ${d.file_count} 个</span></li>`,
        )
        .join('')}${
          entry.stats.subdirs.length > 12
            ? `<li class="ds-more">…等 ${entry.stats.subdirs.length} 个</li>`
            : ''
        }</ul>
      ${(entry.stats.direct_files ?? []).length
        ? `<div class="detail-subs-warn">直属散件 ${(entry.stats.direct_files ?? []).length} 个（${esc(
            (entry.stats.direct_files ?? []).slice(0, 6).join('、'),
          )}${(entry.stats.direct_files ?? []).length > 6 ? ' …' : ''}）：拆分后不属于任何子单位，导出不会包含，建议先移入子目录</div>`
        : ''}
    </div>`
        : ''
    }
    <label class="field wide">
      <span>标签（逗号分隔）</span>
      <input type="text" data-field="tags" value="${esc(entry.tags.join('、'))}" />
    </label>
    <label class="field wide">
      <span>备注</span>
      <textarea data-field="notes" rows="3">${esc(entry.notes)}</textarea>
    </label>
    <p class="hint">${entry.stats.file_count} 个文件 · ${formatSize(entry.stats.total_size)} · 图片 ${entry.stats.image_count} · 音频 ${entry.stats.audio_count} · 视频 ${entry.stats.video_count}</p>
    <p class="hint">${esc(entry.rel_path)}</p>
  `;
}

// ---------------------------------------------------------------- 文件面板

export function renderFileList(state: AppState): void {
  const cwdHost = el('file-cwd');
  const host = el('file-list');
  cwdHost.textContent = state.cwd ?? '未选择目录';

  if (!state.cwd) {
    host.innerHTML = '<p class="hint" style="padding:6px 10px">在表格中选择条目后可在此浏览、整理文件。</p>';
    return;
  }
  if (!state.fileItems.length) {
    host.innerHTML = '<p class="hint" style="padding:6px 10px">（空目录）</p>';
    return;
  }

  host.innerHTML = state.fileItems
    .map((f: FileItem) => {
      const selected = state.fileSelection.has(f.path) ? ' selected' : '';
      const mtime = f.mtime_ms ? new Date(f.mtime_ms).toLocaleString('zh-CN', { hour12: false }) : '—';
      return `<div class="file-row${selected}" data-path="${esc(f.path)}" data-dir="${f.is_dir ? '1' : '0'}" title="${esc(f.path)}">
        <span class="file-name ${f.is_dir ? 'is-dir' : 'is-file'}">${f.is_dir ? '▸' : '·'} ${esc(f.name)}</span>
        <span class="file-meta">${f.is_dir ? '目录' : formatSize(f.size)}</span>
        <span class="file-meta">${esc(f.ext)}</span>
        <span class="file-meta">${esc(mtime)}</span>
      </div>`;
    })
    .join('');
}

// ---------------------------------------------------------------- 自动识别填充

export function autofillFieldsHtml(includeScope: boolean, entryCount: number, selectedCount: number): string {
  const checks = FIELD_LABELS.map(
    ([id, label]) =>
      `<label class="check-row compact"><input type="checkbox" class="af-field-check" value="${id}" checked /><span>${label}</span></label>`,
  ).join('');
  const scope = includeScope
    ? `<p class="hint">填充范围</p>
       <label class="check-row"><input type="radio" name="af-scope" value="selected" ${selectedCount ? 'checked' : ''} ${selectedCount ? '' : 'disabled'} /><span>已选条目（${selectedCount}）</span></label>
       <label class="check-row"><input type="radio" name="af-scope" value="all" ${selectedCount ? '' : 'checked'} /><span>全部条目（${entryCount}）</span></label>`
    : '';
  return `<p class="hint">选择要识别填充的字段。数据来源：目录名、已有说明文件、CUE、音视频/图片内嵌标签（FLAC/MP3/WAV）等。</p>
    <div class="af-fields">${checks}</div>
    ${scope}`;
}

export function checkedAutofillFields(): string[] {
  return [...document.querySelectorAll<HTMLInputElement>('.af-field-check:checked')].map((x) => x.value);
}

export function autofillScope(): 'selected' | 'all' {
  const r = document.querySelector<HTMLInputElement>('input[name="af-scope"]:checked');
  return r?.value === 'all' ? 'all' : 'selected';
}

function proposalValueHtml(p: Proposal): string {
  const single = p.value.replace(/\n+/g, ' / ');
  return `<span class="af-old" title="${esc(p.old)}">${esc(p.old || '（空）')}</span>
    <span class="af-arrow">→</span>
    <span class="af-new" title="${esc(p.value)}">${esc(shorten(single, 220))}</span>`;
}

function shorten(s: string, n: number): string {
  return [...s].length > n ? `${[...s].slice(0, n).join('')}…` : s;
}

/** 提案确认表：逐条勾选后应用 */
export function openProposalModal(proposals: Proposal[], onApply: (checked: Proposal[]) => void): void {
  const rows = proposals
    .map(
      (p, i) => `<label class="af-row" title="${esc(p.rel_path)}">
        <input type="checkbox" class="af-check" data-i="${i}" checked />
        <span class="af-path">${esc(p.rel_path)}</span>
        <span class="af-field">${esc(FIELD_LABELS.find(([id]) => id === p.field)?.[1] ?? p.field)}</span>
        <span class="af-values">${proposalValueHtml(p)}</span>
        <span class="af-src">${esc(p.source)}</span>
      </label>`,
    )
    .join('');

  openModal(
    `自动识别填充 · ${proposals.length} 条提案`,
    `<div class="af-toolbar">
       <button class="btn-sm" id="af-all">全选</button>
       <button class="btn-sm" id="af-none">全不选</button>
       <span class="hint" id="af-count">已勾选 ${proposals.length} / ${proposals.length}</span>
     </div>
     <div class="af-list">${rows}</div>`,
    [
      { label: '取消', onClick: (close) => close() },
      {
        label: '应用所选',
        primary: true,
        onClick: (close) => {
          const checked = [...document.querySelectorAll<HTMLInputElement>('.af-check:checked')].map(
            (x) => proposals[Number(x.dataset.i)],
          );
          close();
          onApply(checked);
        },
      },
    ],
    true,
  );

  const list = el('modal').querySelector('.af-list');
  const count = el('af-count');
  const update = (): void => {
    const n = list?.querySelectorAll<HTMLInputElement>('.af-check:checked').length ?? 0;
    count.textContent = `已勾选 ${n} / ${proposals.length}`;
  };
  list?.addEventListener('change', update);
  el('af-all').addEventListener('click', () => {
    list?.querySelectorAll<HTMLInputElement>('.af-check').forEach((c) => (c.checked = true));
    update();
  });
  el('af-none').addEventListener('click', () => {
    list?.querySelectorAll<HTMLInputElement>('.af-check').forEach((c) => (c.checked = false));
    update();
  });
}

// ---------------------------------------------------------------- 统计

export function renderStats(state: AppState): void {
  const host = el('stats-box');
  const scan = state.scan;
  const project = state.project;
  if (!scan && !project) {
    host.textContent = '尚未扫描';
    return;
  }
  const rows: [string, string][] = [];
  if (scan) {
    rows.push(['容器目录', `${scan.container_count} 个`]);
    rows.push(['忽略目录', `${scan.ignored_count} 个`]);
    rows.push(['文件总数', `${scan.total_files} 个`]);
    rows.push(['总大小', formatSize(scan.total_size)]);
  }
  if (project) {
    const sel = project.entries.filter((e) => e.selected);
    rows.push(['识别单位', `${project.entries.length} 个`]);
    rows.push(['已选条目', `${sel.length} / ${project.entries.length}`]);
    rows.push(['已选大小', formatSize(sel.reduce((a, e) => a + e.stats.total_size, 0))]);
    rows.push(['说明文件', `${project.entries.filter((e) => e.has_notes).length} 已生成`]);
  }
  host.innerHTML = rows.map(([k, v]) => `<div class="stat-row"><span>${k}</span><b>${v}</b></div>`).join('');
}

// ---------------------------------------------------------------- 密码方案

export function renderPasswords(state: AppState): void {
  const host = el('password-list');
  const project = state.project;
  if (!project || !project.password_profiles.length) {
    host.innerHTML = '<p class="hint">尚无密码方案。</p>';
    return;
  }
  host.innerHTML = project.password_profiles
    .map((p: PasswordProfile) => {
      const active = p.id === state.activeProfileId ? ' active' : '';
      const used = project.entries.filter((e) => e.password_profile === p.id).length;
      return `<div class="pw-row${active}" data-id="${esc(p.id)}">
        <div class="pw-main">
          <b>${esc(p.usage || p.id)}</b>
          <span class="hint">${p.passwords.length} 个密码 · 用于 ${used} 条</span>
        </div>
        <div class="pw-passwords">${p.passwords.map((w) => `<code>${esc(w)}</code>`).join('')}</div>
        <button class="btn-sm pw-del" data-id="${esc(p.id)}" title="删除方案">✕</button>
      </div>`;
    })
    .join('');
}

// ---------------------------------------------------------------- 密码表单 ↔ 项目

export function syncArchiveForm(project: Project): void {
  el<HTMLSelectElement>('archive-format').value = project.archive.format;
  el<HTMLInputElement>('volume-size').value = String(project.archive.volume_mib);
  el<HTMLInputElement>('comment-toggle').checked = project.archive.comment;
  el<HTMLInputElement>('encrypt-names-toggle').checked = project.archive.encrypt_names;
  el<HTMLInputElement>('overwrite-toggle').checked = project.archive.overwrite;
  el<HTMLInputElement>('incremental-toggle').checked = project.archive.incremental;
  updateFormatAvailability();
}

/** ZIP 不支持分卷 / 加密：切换格式时禁用相关控件并给出提示 */
export function updateFormatAvailability(): void {
  const zip = el<HTMLSelectElement>('archive-format').value === 'zip';
  el<HTMLInputElement>('volume-size').disabled = zip;
  el<HTMLInputElement>('encrypt-names-toggle').disabled = zip;
  const hint = document.getElementById('format-hint');
  if (hint) {
    hint.textContent = zip ? 'ZIP 使用内置压缩：不支持密码加密与分卷，密码方案会被忽略。' : '';
  }
}

export function readArchiveForm(project: Project): void {
  project.archive.format = el<HTMLSelectElement>('archive-format').value;
  project.archive.volume_mib = Math.max(0, Number(el<HTMLInputElement>('volume-size').value) || 0);
  project.archive.comment = el<HTMLInputElement>('comment-toggle').checked;
  project.archive.encrypt_names = el<HTMLInputElement>('encrypt-names-toggle').checked;
  project.archive.overwrite = el<HTMLInputElement>('overwrite-toggle').checked;
  project.archive.incremental = el<HTMLInputElement>('incremental-toggle').checked;
}

// ---------------------------------------------------------------- 模态框

export interface ModalAction {
  label: string;
  primary?: boolean;
  /** 可返回 Promise：执行期间按钮自动禁用，reject 后弹窗保持可重试 */
  onClick: (close: () => void) => void | Promise<void>;
}

/** 当前弹窗的关闭兜底（点 ✕ / Esc 走 closeModal 时触发；打开新弹窗会替换） */
let modalCloseHook: (() => void) | null = null;

export function openModal(
  title: string,
  bodyHtml: string,
  actions: ModalAction[],
  wide = false,
  onClose?: () => void,
): void {
  const overlay = el('modal-overlay');
  const modal = el('modal');
  modalCloseHook = onClose ?? null;

  modal.classList.toggle('wide', wide);
  modal.innerHTML = `
    <div class="modal-head">
      <h3>${esc(title)}</h3>
      <button class="btn-sm" data-close>✕</button>
    </div>
    <div class="modal-body">${bodyHtml}</div>
    <div class="modal-foot">
      ${actions.map((a, i) => `<button class="btn ${a.primary ? 'primary' : ''}" data-action="${i}">${esc(a.label)}</button>`).join('')}
    </div>`;

  const close = (): void => closeModal();
  modal.querySelector('[data-close]')?.addEventListener('click', close);
  overlay.classList.remove('hidden');

  const actionButtons = [...modal.querySelectorAll<HTMLButtonElement>('[data-action]')];
  const setDisabled = (disabled: boolean): void => {
    actionButtons.forEach((b) => (b.disabled = disabled));
  };

  actionButtons.forEach((btn) => {
    btn.addEventListener('click', () => {
      const action = actions[Number(btn.dataset.action)];
      if (!action || btn.disabled) return;
      const result = action.onClick(close);
      if (result instanceof Promise) {
        setDisabled(true);
        void result
          .catch((err) => {
            toast(String(err), 4000);
          })
          .finally(() => {
            if (overlay.classList.contains('hidden')) return;
            setDisabled(false);
          });
      }
    });
  });

  // 打开时聚焦第一个可聚焦元素
  const focusable = modal.querySelector<HTMLElement>(
    '.modal-body input:not([type=hidden]), .modal-body select, .modal-body textarea, .modal-body button, .modal-foot button',
  );
  focusable?.focus({ preventScroll: true });
}

export function closeModal(): void {
  const hook = modalCloseHook;
  modalCloseHook = null;
  hook?.();
  el('modal').classList.remove('wide');
  el('modal-overlay').classList.add('hidden');
}

/** 单输入框确认框；onOk 可为异步，出错时保持弹窗可重试 */
export function openPrompt(
  title: string,
  label: string,
  value: string,
  placeholder: string,
  onOk: (value: string) => void | Promise<void>,
  datalist?: string[],
  allowEmpty = false,
): void {
  const listHtml = datalist?.length
    ? `<datalist id="prompt-datalist">${datalist.map((d) => `<option value="${esc(d)}"></option>`).join('')}</datalist>`
    : '';
  let busy = false;
  const run = async (close: () => void): Promise<void> => {
    if (busy) return;
    const v = el<HTMLInputElement>('prompt-input').value.trim();
    if (!v && !allowEmpty) return;
    busy = true;
    try {
      await onOk(v);
      close();
    } catch (err) {
      toast(String(err), 4000);
    } finally {
      busy = false;
    }
  };

  openModal(
    title,
    `<label class="field"><span>${esc(label)}</span>
       <input id="prompt-input" value="${esc(value)}" placeholder="${esc(placeholder)}" ${datalist?.length ? 'list="prompt-datalist"' : ''} />
     </label>${listHtml}`,
    [
      { label: '取消', onClick: (close) => close() },
      { label: '确定', primary: true, onClick: (close) => run(close) },
    ],
  );
  const input = el<HTMLInputElement>('prompt-input');
  input.focus();
  input.select();
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      void run(closeModal);
    }
  });
}

// ---------------------------------------------------------------- 进度 / 日志

export function setProgress(percent: number, text: string): void {
  el('progress-track').classList.remove('indeterminate');
  el('progress-fill').style.width = `${Math.max(0, Math.min(100, percent))}%`;
  el('progress-text').textContent = text;
}

/** 只更新进度文案（保持/不触发确定性进度样式），用于扫描等无百分比事件的长任务 */
export function setProgressText(text: string): void {
  el('progress-text').textContent = text;
}

const logs: string[] = [];

export function log(line: string, kind: 'info' | 'ok' | 'error' = 'info'): void {
  const time = new Date().toLocaleTimeString('zh-CN', { hour12: false });
  logs.push(`[${time}] ${line}`);
  if (logs.length > 400) logs.shift();
  const host = el('log-view');
  if (host.dataset.ready !== '1') {
    host.innerHTML = '';
    host.dataset.ready = '1';
  }
  const atBottom = host.scrollTop + host.clientHeight >= host.scrollHeight - 4;
  const div = document.createElement('div');
  div.className = `log-line ${kind}`;
  div.textContent = line;
  host.appendChild(div);
  if (atBottom) host.scrollTop = host.scrollHeight;
  el('log-count').textContent = `${logs.length} 条`;
}

export function clearLog(): void {
  logs.length = 0;
  const host = el('log-view');
  host.innerHTML = '<span class="hint">暂无日志</span>';
  delete host.dataset.ready;
  el('log-count').textContent = '';
}

export function notesStatus(p: NotesProgress): string {
  return `${p.phase} ${p.index + 1}/${p.total} ${p.rel_path} ${p.status}`;
}

export function exportStatus(p: ExportProgress): string {
  if (p.phase === 'running') {
    return `${p.index + 1}/${p.total} ${p.rel_path} ${p.unit_percent}%`;
  }
  if (p.phase === 'finished') {
    return `完成：新建 ${p.created} · 跳过 ${p.skipped} · 失败 ${p.failed}`;
  }
  return p.message;
}

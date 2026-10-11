import { paneVisible, resetLayout, togglePane, type PaneKey } from './layout';
import { esc, openModal } from './ui';

/** 由 main.ts 提供的命令实现；快捷键与勾选状态由本模块统一维护 */
export interface MenuActions {
  newProject(): void;
  openProject(): void;
  openRecent(path: string): void;
  saveProject(): void;
  saveProjectAs(): void;
  settings(): void;
  quit(): void;
  selectAll(): void;
  selectNone(): void;
  batchEdit(): void;
  autofill(): void;
  classify(): void;
  previewNote(): void;
  diffNote(): void;
  auditNotes(): void;
  findDupes(): void;
  verifyManifest(): void;
  nameAudit(): void;
  trash(): void;
  globalSearch(): void;
  scan(): void;
  notes(): void;
  exportSettings(): void;
  exportRun(): void;
  exportCancel(): void;
}

/** 输入框内让给浏览器默认行为的组合键 */
const EDITABLE_PASSTHROUGH = new Set(['Ctrl+A', 'Ctrl+C', 'Ctrl+X', 'Ctrl+V', 'Ctrl+Z', 'Ctrl+Y']);

let actions: MenuActions;
let menus: HTMLElement[] = [];
let items: HTMLButtonElement[] = [];

function closeMenus(): void {
  menus.forEach((m) => m.classList.remove('open'));
}

function allMenuItems(): HTMLButtonElement[] {
  return [...document.querySelectorAll<HTMLButtonElement>('#menubar .menu-item')];
}

function refreshChecks(): void {
  allMenuItems().forEach((it) => {
    const pane = it.dataset.check as PaneKey | undefined;
    if (pane) it.classList.toggle('checked', paneVisible(pane));
  });
}

/** 任务进行中：data-busy="block" 禁用、data-busy="only" 反用（仅任务中可用） */
export function setMenuBusy(busy: boolean): void {
  allMenuItems().forEach((it) => {
    const mode = it.dataset.busy;
    if (mode === 'block') it.disabled = busy;
    else if (mode === 'only') it.disabled = !busy;
  });
}

function run(item: HTMLButtonElement): void {
  if (item.disabled) return;
  closePalette();
  const pane = item.dataset.pane as PaneKey | undefined;
  if (pane) {
    togglePane(pane);
    refreshChecks();
    return;
  }
  if (item.dataset.layoutReset) {
    resetLayout();
    refreshChecks();
    return;
  }
  const cmd = item.dataset.cmd;
  if (cmd === 'shortcuts') {
    openShortcuts();
    return;
  }
  if (cmd === 'about') {
    openAbout();
    return;
  }
  if (cmd === 'palette') {
    const overlay = document.getElementById('modal-overlay');
    if (!overlay || overlay.classList.contains('hidden')) openPalette();
    return;
  }
  // openRecent 带参数、经 data-recent 按钮分发，这里跳过
  if (cmd && cmd !== 'openRecent' && cmd in actions) {
    (actions as unknown as Record<string, () => void>)[cmd]();
  }
}

function comboOf(e: KeyboardEvent): string {
  const parts: string[] = [];
  if (e.ctrlKey) parts.push('Ctrl');
  if (e.altKey) parts.push('Alt');
  if (e.shiftKey) parts.push('Shift');
  let key = e.key;
  if (key === ' ') key = 'Space';
  else if (key.length === 1) key = key.toUpperCase();
  parts.push(key);
  return parts.join('+');
}

function openShortcuts(): void {
  closeMenus();
  const rows = menus
    .map((menu) => {
      const title = menu.querySelector('.menu-title')?.textContent ?? '';
      const lines = [...menu.querySelectorAll<HTMLButtonElement>('.menu-item:not([data-recent])')]
        .map((it) => {
          const label = it.querySelector('.mi-label')?.textContent ?? '';
          const keys = it.dataset.keys ?? '';
          return `<tr><td>${esc(label)}</td><td>${keys ? `<kbd>${esc(keys)}</kbd>` : '<span class="hint">—</span>'}</td></tr>`;
        })
        .join('');
      return `<tr><th colspan="2" class="sc-group">${esc(title)}</th></tr>${lines}`;
    })
    .join('');
  openModal(
    '快捷键',
    `<table class="sc-table"><tbody>${rows}</tbody></table>
     <p class="hint">菜单打开时可用 ↑↓ 选择、←→ 切换菜单、Esc 关闭；Delete 只作用于底部文件面板。</p>`,
    [{ label: '关闭', onClick: (close) => close() }],
    true,
  );
}

function openAbout(): void {
  closeMenus();
  openModal(
    '关于 vinePress',
    `<div class="about-box">
       <div class="brand">vinePress</div>
       <p>版本 0.1.0 · 面向个人媒体库的文件归档工具</p>
       <p class="hint">技术栈：Tauri 2.x + Rust + TypeScript + Vite（零框架前端）。</p>
       <p class="hint">偏好保存在 localStorage：<code>vinepress.prefs</code>（外观/缩放/默认压缩参数）、
         <code>vinepress.layout</code>（面板尺寸与显隐）、<code>vinepress.root</code> / <code>vinepress.output</code>（最近路径）、
         <code>vinepress.recent</code>（最近打开的项目）；
         项目数据写入 <code>vinePress-project.json</code>。</p>
       <p class="hint">导出依赖本机 WinRAR（Rar.exe）；删除操作只走系统回收站，绝不永久删除。</p>
     </div>`,
    [{ label: '关闭', onClick: (close) => close() }],
  );
}

// ---------------------------------------------------------------- 最近打开的项目

let recentPaths: string[] = [];

function renderRecent(): void {
  const box = document.getElementById('recent-menu');
  const sep = document.getElementById('recent-sep');
  if (!box || !sep) return;
  const has = recentPaths.length > 0;
  box.hidden = !has;
  sep.hidden = !has;
  if (!has) return;
  box.innerHTML =
    `<div class="menu-recent-title">最近打开</div>` +
    recentPaths
      .map((p) => {
        const name = p.split(/[\\/]/).pop() || p;
        return `<button type="button" class="menu-item" data-recent="${esc(p)}" title="${esc(p)}" data-busy="block"><span class="mi-label">${esc(name)}</span></button>`;
      })
      .join('');
  box.querySelectorAll<HTMLButtonElement>('.menu-item').forEach((btn) => {
    btn.addEventListener('click', () => {
      closeMenus();
      actions.openRecent(btn.dataset.recent ?? '');
    });
  });
}

/** main.ts 在打开/保存成功后调用；路径列表按最新在前去重 */
export function setRecentProjects(paths: string[]): void {
  recentPaths = paths;
  renderRecent();
}

// ---------------------------------------------------------------- 命令面板（Ctrl+K）

interface PaletteEntry {
  label: string;
  menu: string;
  keys: string;
  el: HTMLButtonElement;
  score: number;
  marks: number[];
}

/** 子串优先、子序列兜底；返回得分与命中下标，不匹配返回 null */
function matchCommand(q: string, text: string): { score: number; marks: number[] } | null {
  if (!q) return { score: 0, marks: [] };
  const t = text.toLowerCase();
  const at = t.indexOf(q);
  if (at >= 0) {
    return { score: 1000 - at, marks: Array.from({ length: q.length }, (_, i) => at + i) };
  }
  const marks: number[] = [];
  let ti = 0;
  let gaps = 0;
  for (const ch of q) {
    const found = t.indexOf(ch, ti);
    if (found < 0) return null;
    if (marks.length) gaps += found - ti;
    marks.push(found);
    ti = found + 1;
  }
  return { score: 500 - gaps, marks };
}

function highlightLabel(label: string, marks: number[]): string {
  if (!marks.length) return esc(label);
  const set = new Set(marks);
  let out = '';
  for (let i = 0; i < label.length; i++) {
    out += set.has(i) ? `<mark>${esc(label[i])}</mark>` : esc(label[i]);
  }
  return out;
}

let paletteSelection = 0;
let paletteHits: PaletteEntry[] = [];

function paletteData(): PaletteEntry[] {
  const out: PaletteEntry[] = [];
  for (const menu of menus) {
    const mtitle = menu.querySelector('.menu-title')?.textContent ?? '';
    for (const it of menu.querySelectorAll<HTMLButtonElement>('.menu-item')) {
      const label = it.querySelector('.mi-label')?.textContent ?? '';
      if (!label) continue;
      out.push({ label, menu: mtitle, keys: it.dataset.keys ?? '', el: it, score: 0, marks: [] });
    }
  }
  return out;
}

function renderPalette(): void {
  const input = document.getElementById('palette-input') as HTMLInputElement | null;
  const list = document.getElementById('palette-list');
  if (!input || !list) return;
  const q = input.value.trim().toLowerCase();
  paletteHits = paletteData()
    .map((e) => {
      const m = matchCommand(q, `${e.menu} ${e.label}`.toLowerCase());
      if (!m) return null;
      e.score = m.score;
      const lAt = e.label.toLowerCase().indexOf(q);
      e.marks =
        q && lAt >= 0
          ? Array.from({ length: q.length }, (_, i) => lAt + i)
          : [];
      return e;
    })
    .filter((e): e is PaletteEntry => e !== null)
    .sort((a, b) => b.score - a.score)
    .slice(0, 60);

  if (paletteSelection >= paletteHits.length) paletteSelection = Math.max(0, paletteHits.length - 1);
  list.innerHTML = paletteHits.length
    ? paletteHits
        .map(
          (e, i) => `<div class="palette-item${i === paletteSelection ? ' active' : ''}${e.el.disabled ? ' disabled' : ''}" data-idx="${i}">
            <span class="pi-label">${highlightLabel(e.label, e.marks)}</span>
            <span class="pi-menu">${esc(e.menu)}</span>
            ${e.keys ? `<kbd>${esc(e.keys)}</kbd>` : ''}
          </div>`,
        )
        .join('')
    : `<div class="palette-empty hint">没有匹配的命令</div>`;
  list.querySelector('.palette-item.active')?.scrollIntoView({ block: 'nearest' });
  list.querySelectorAll<HTMLElement>('.palette-item').forEach((row) => {
    row.addEventListener('click', () => {
      paletteSelection = Number(row.dataset.idx);
      activatePalette();
    });
    row.addEventListener('mouseenter', () => {
      paletteSelection = Number(row.dataset.idx);
      list.querySelectorAll('.palette-item').forEach((r, i) => r.classList.toggle('active', i === paletteSelection));
    });
  });
}

function activatePalette(): void {
  const hit = paletteHits[paletteSelection];
  if (!hit || hit.el.disabled) return;
  closePalette();
  hit.el.click();
}

function closePalette(): void {
  document.getElementById('palette-overlay')?.classList.add('hidden');
}

export function openPalette(): void {
  closeMenus();
  const overlay = document.getElementById('palette-overlay');
  const input = document.getElementById('palette-input') as HTMLInputElement | null;
  if (!overlay || !input) return;
  overlay.classList.remove('hidden');
  input.value = '';
  paletteSelection = 0;
  renderPalette();
  input.focus();
}

function initPalette(): void {
  const overlay = document.getElementById('palette-overlay');
  const input = document.getElementById('palette-input') as HTMLInputElement | null;
  if (!overlay || !input) return;

  input.addEventListener('input', () => {
    paletteSelection = 0;
    renderPalette();
  });
  input.addEventListener('keydown', (e) => {
    const list = document.getElementById('palette-list');
    const count = list?.querySelectorAll('.palette-item').length ?? 0;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (count) paletteSelection = (paletteSelection + 1) % count;
      renderPalette();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (count) paletteSelection = (paletteSelection - 1 + count) % count;
      renderPalette();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      activatePalette();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      closePalette();
    }
  });
  overlay.addEventListener('click', (e) => {
    if (e.target === overlay) closePalette();
  });
}

export function initMenubar(list: MenuActions): void {
  actions = list;
  const root = document.getElementById('menubar');
  if (!root) throw new Error('缺少元素 #menubar');
  menus = [...root.querySelectorAll<HTMLElement>(':scope > .menu')];
  items = [...root.querySelectorAll<HTMLButtonElement>('.menu-item')];
  refreshChecks();
  setMenuBusy(false);
  initPalette();
  renderRecent();

  menus.forEach((menu) => {
    const title = menu.querySelector<HTMLElement>('.menu-title');
    title?.addEventListener('click', (e) => {
      e.stopPropagation();
      const wasOpen = menu.classList.contains('open');
      closeMenus();
      if (!wasOpen) {
        menu.classList.add('open');
        refreshChecks();
        menu.querySelector<HTMLButtonElement>('.menu-item:not(:disabled)')?.focus();
      }
    });
    title?.addEventListener('mouseenter', () => {
      if (menus.some((m) => m.classList.contains('open')) && !menu.classList.contains('open')) {
        closeMenus();
        menu.classList.add('open');
        refreshChecks();
      }
    });
  });

  items.forEach((item) => {
    item.addEventListener('click', () => {
      run(item);
      closeMenus();
    });
  });

  // 打开菜单内的键盘导航
  root.addEventListener('keydown', (e) => {
    const open = menus.find((m) => m.classList.contains('open'));
    if (!open) return;
    const visible = [...open.querySelectorAll<HTMLButtonElement>('.menu-item')].filter((b) => !b.disabled);
    const active = document.activeElement instanceof HTMLButtonElement ? document.activeElement : null;
    const idx = active ? visible.indexOf(active) : -1;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      visible[(idx + 1) % visible.length]?.focus();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      visible[(idx - 1 + visible.length) % visible.length]?.focus();
    } else if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
      e.preventDefault();
      const at = menus.indexOf(open);
      const next = (at + (e.key === 'ArrowRight' ? 1 : -1) + menus.length) % menus.length;
      closeMenus();
      menus[next].classList.add('open');
      refreshChecks();
      menus[next].querySelector<HTMLElement>('.menu-title')?.focus();
    }
  });

  document.addEventListener('click', (e) => {
    if (!root.contains(e.target as Node)) closeMenus();
  });

  // 全局快捷键：data-keys 为唯一来源（与快捷键弹窗同源）
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      closeMenus();
      return;
    }
    const combo = comboOf(e);
    const item = items.find((it) => it.dataset.keys === combo);
    if (!item) return;
    const target = e.target as HTMLElement | null;
    const editable =
      !!target &&
      (target.tagName === 'INPUT' ||
        target.tagName === 'TEXTAREA' ||
        target.tagName === 'SELECT' ||
        target.isContentEditable);
    const isFnKey = /^F\d{1,2}$/.test(e.key);
    if (editable && !e.ctrlKey && !e.altKey && !isFnKey) return;
    if (editable && EDITABLE_PASSTHROUGH.has(combo)) return;
    if (item.disabled) return;
    e.preventDefault();
    run(item);
  });
}

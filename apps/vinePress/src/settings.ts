import type { ArchiveSettings } from './api';
import {
  getLayout,
  paneSize,
  paneVisible,
  resetLayout,
  restoreLayout,
  setPaneSize,
  togglePane,
  type LayoutState,
  type PaneKey,
} from './layout';
import { el, openModal, toast } from './ui';

export type ThemePref = 'system' | 'light' | 'dark';

export interface Prefs {
  /** 主题：跟随系统 / 浅色 / 深色 */
  theme: ThemePref;
  /** 界面缩放倍率（:root --zoom） */
  zoom: number;
  /** 减少动画 */
  reduceMotion: boolean;
  /** 「新建项目」采用的压缩参数 */
  archiveDefaults: ArchiveSettings | null;
}

export interface SettingsHandlers {
  /** 把本页「默认压缩参数」应用到当前项目（同步右侧压缩参数面板） */
  applyArchiveDefaultsToProject(): void;
}

const STORE_KEY = 'vinepress.prefs';
const PANE_LABEL: Record<PaneKey, string> = {
  left: '目录树面板',
  right: '详情面板',
  file: '文件面板',
};
const PANE_BOUNDS: Record<PaneKey, [number, number]> = {
  left: [220, 560],
  right: [260, 640],
  file: [96, 640],
};
const ZOOMS = [0.9, 1, 1.1, 1.25, 1.5];

function builtinArchiveDefaults(): ArchiveSettings {
  return {
    format: 'rar',
    volume_mib: 900,
    comment: true,
    encrypt_names: true,
    overwrite: false,
    write_notes: true,
    incremental: false,
  };
}

function defaults(): Prefs {
  return { theme: 'system', zoom: 1, reduceMotion: false, archiveDefaults: null };
}

function load(): Prefs {
  try {
    const raw = localStorage.getItem(STORE_KEY);
    if (!raw) return defaults();
    const p = { ...defaults(), ...(JSON.parse(raw) as Partial<Prefs>) };
    if (p.theme !== 'system' && p.theme !== 'light' && p.theme !== 'dark') p.theme = 'system';
    if (!ZOOMS.includes(p.zoom)) p.zoom = 1;
    return p;
  } catch {
    return defaults();
  }
}

let prefs: Prefs = load();
const systemQuery = window.matchMedia('(prefers-color-scheme: light)');

export function getPrefs(): Prefs {
  return prefs;
}

function applyTheme(): void {
  const theme = prefs.theme === 'system' ? (systemQuery.matches ? 'light' : 'dark') : prefs.theme;
  document.documentElement.dataset.theme = theme;
}

function applyZoom(): void {
  document.documentElement.style.setProperty('--zoom', String(prefs.zoom));
}

function applyMotion(): void {
  document.documentElement.classList.toggle('reduce-motion', prefs.reduceMotion);
}

function applyAll(): void {
  applyTheme();
  applyZoom();
  applyMotion();
}

function persist(): void {
  localStorage.setItem(STORE_KEY, JSON.stringify(prefs));
}

function setPrefs(patch: Partial<Prefs>): void {
  prefs = { ...prefs, ...patch };
  applyAll();
  persist();
}

/** main.ts 模块加载即调用：首帧前套用主题/缩放，避免闪烁 */
export function initPrefs(): void {
  systemQuery.addEventListener('change', () => {
    if (prefs.theme === 'system') applyTheme();
  });
  applyAll();
}

export function openSettings(handlers: SettingsHandlers): void {
  let committed = false;
  const snapPrefs: Prefs = JSON.parse(JSON.stringify(prefs)) as Prefs;
  const snapLayout: LayoutState = getLayout();

  const revert = (): void => {
    if (committed) return;
    prefs = JSON.parse(JSON.stringify(snapPrefs)) as Prefs;
    applyAll();
    persist();
    restoreLayout(snapLayout);
  };
  const commit = (): void => {
    prefs.archiveDefaults = readArchivePage();
    committed = true;
    persist();
  };

  const ad = prefs.archiveDefaults ?? builtinArchiveDefaults();

  openModal(
    '设置',
    `<div class="settings">
       <nav class="settings-nav">
         <button type="button" class="active" data-page="appearance">外观</button>
         <button type="button" data-page="layout">布局</button>
         <button type="button" data-page="archive">默认压缩参数</button>
         <button type="button" data-page="about">关于</button>
       </nav>
       <div class="settings-pages">

         <section class="settings-page active" data-page="appearance">
           <div class="field-grid">
             <div class="field wide"><span>主题</span>
               <div class="radio-row">
                 <label class="check-row"><input type="radio" name="st-theme" value="system" ${prefs.theme === 'system' ? 'checked' : ''} /><span>跟随系统</span></label>
                 <label class="check-row"><input type="radio" name="st-theme" value="light" ${prefs.theme === 'light' ? 'checked' : ''} /><span>浅色</span></label>
                 <label class="check-row"><input type="radio" name="st-theme" value="dark" ${prefs.theme === 'dark' ? 'checked' : ''} /><span>深色</span></label>
               </div>
             </div>
             <label class="field"><span>界面缩放</span>
               <select id="st-zoom">
                 ${ZOOMS.map((z) => `<option value="${z}" ${prefs.zoom === z ? 'selected' : ''}>${Math.round(z * 100)}%</option>`).join('')}
               </select>
             </label>
             <div class="field wide"><span>动效</span>
               <label class="check-row"><input type="checkbox" id="st-motion" ${prefs.reduceMotion ? 'checked' : ''} /><span>减少动画（进度条/弹窗即时切换）</span></label>
             </div>
           </div>
           <p class="hint">主题与缩放立即生效并保存；「跟随系统」会随操作系统的深浅色自动切换。</p>
         </section>

         <section class="settings-page" data-page="layout">
           <div class="field wide"><span>显示面板</span>
             <div class="radio-row">
               ${(Object.keys(PANE_LABEL) as PaneKey[])
                 .map(
                   (key) =>
                     `<label class="check-row"><input type="checkbox" data-pane-show="${key}" ${paneVisible(key) ? 'checked' : ''} /><span>${PANE_LABEL[key]}</span></label>`,
                 )
                 .join('')}
             </div>
           </div>
           <div class="field-grid">
             ${(Object.keys(PANE_LABEL) as PaneKey[])
               .map(
                 (key) =>
                   `<label class="field"><span>${key === 'file' ? '文件面板高度' : key === 'left' ? '目录树宽度' : '详情宽度'} (px)</span>
                      <input type="number" data-pane-size="${key}" min="${PANE_BOUNDS[key][0]}" max="${PANE_BOUNDS[key][1]}" value="${paneSize(key)}" /></label>`,
               )
               .join('')}
           </div>
           <button type="button" class="btn-sm" id="st-reset-layout">恢复默认布局</button>
           <p class="hint">也可以直接拖动面板之间的分隔条，双击分隔条复位该侧尺寸；快捷键 Ctrl+1 / Ctrl+2 / Ctrl+3 切换面板显隐。</p>
         </section>

         <section class="settings-page" data-page="archive">
           <div class="field-grid">
             <label class="field"><span>格式</span>
               <select id="st-format">
                 <option value="rar" ${ad.format === 'rar' ? 'selected' : ''}>RAR5（WinRAR）</option>
                 <option value="zip" ${ad.format === 'zip' ? 'selected' : ''}>ZIP（内置）</option>
               </select></label>
             <label class="field"><span>分卷 (MiB)</span><input type="number" id="st-volume" min="0" value="${ad.volume_mib}" /></label>
           </div>
           <label class="check-row"><input type="checkbox" id="st-comment" ${ad.comment ? 'checked' : ''} /><span>写入压缩注释（= 说明文件内容）</span></label>
           <label class="check-row"><input type="checkbox" id="st-encrypt" ${ad.encrypt_names ? 'checked' : ''} /><span>加密文件名（-hp）</span></label>
           <label class="check-row"><input type="checkbox" id="st-overwrite" ${ad.overwrite ? 'checked' : ''} /><span>覆盖已存在的压缩包</span></label>
            <label class="check-row"><input type="checkbox" id="st-writenotes" ${ad.write_notes ? 'checked' : ''} /><span>导出前先生成说明文件</span></label>
            <label class="check-row"><input type="checkbox" id="st-incremental" ${ad.incremental ? 'checked' : ''} /><span>增量跳过未变动单位（按导出清单）</span></label>
           <div class="settings-actions">
             <button type="button" class="btn-sm" id="st-apply-project">应用到当前项目</button>
           </div>
           <p class="hint">「确定/应用」保存为默认值，新建项目自动采用；ZIP 不支持分卷与加密。</p>
         </section>

         <section class="settings-page" data-page="about">
           <div class="brand">vinePress</div>
           <p>版本 0.1.0 · 面向个人媒体库的文件归档工具</p>
           <p class="hint">技术栈：Tauri 2.x + Rust + TypeScript + Vite（零框架前端）。</p>
           <p class="hint">偏好存于 localStorage（<code>vinepress.prefs</code> / <code>vinepress.layout</code>），项目数据写入 <code>vinePress-project.json</code>。</p>
           <p class="hint">完整快捷键见「帮助 → 快捷键列表」。</p>
         </section>

       </div>
     </div>`,
    [
      { label: '取消', onClick: (close) => { revert(); close(); } },
      {
        label: '应用',
        onClick: () => {
          commit();
          toast('设置已保存');
        },
      },
      {
        label: '确定',
        primary: true,
        onClick: (close) => {
          commit();
          close();
        },
      },
    ],
    false,
    // Esc / ✕ 走 closeModal：未确定过则视为取消
    () => revert(),
  );

  const modal = el('modal');

  // 分类导航
  modal.querySelectorAll<HTMLButtonElement>('.settings-nav button').forEach((btn) => {
    btn.addEventListener('click', () => {
      modal.querySelectorAll('.settings-nav button').forEach((b) => b.classList.remove('active'));
      modal.querySelectorAll('.settings-page').forEach((p) => p.classList.remove('active'));
      btn.classList.add('active');
      modal.querySelector(`.settings-page[data-page="${btn.dataset.page}"]`)?.classList.add('active');
    });
  });

  // 外观
  modal.querySelectorAll<HTMLInputElement>('input[name="st-theme"]').forEach((radio) => {
    radio.addEventListener('change', () => {
      if (radio.checked) setPrefs({ theme: radio.value as ThemePref });
    });
  });
  modal.querySelector<HTMLSelectElement>('#st-zoom')?.addEventListener('change', (e) => {
    setPrefs({ zoom: Number((e.target as HTMLSelectElement).value) });
  });
  modal.querySelector<HTMLInputElement>('#st-motion')?.addEventListener('change', (e) => {
    setPrefs({ reduceMotion: (e.target as HTMLInputElement).checked });
  });

  // 布局
  modal.querySelectorAll<HTMLInputElement>('[data-pane-show]').forEach((box) => {
    box.addEventListener('change', () => {
      const key = box.dataset.paneShow as PaneKey;
      if (box.checked !== paneVisible(key)) togglePane(key);
    });
  });
  const syncPaneInputs = (): void => {
    modal.querySelectorAll<HTMLInputElement>('[data-pane-size]').forEach((input) => {
      input.value = String(paneSize(input.dataset.paneSize as PaneKey));
    });
  };
  modal.querySelectorAll<HTMLInputElement>('[data-pane-size]').forEach((input) => {
    input.addEventListener('change', () => {
      const key = input.dataset.paneSize as PaneKey;
      setPaneSize(key, Number(input.value));
      input.value = String(paneSize(key));
    });
  });
  modal.querySelector('#st-reset-layout')?.addEventListener('click', () => {
    resetLayout();
    syncPaneInputs();
    modal.querySelectorAll<HTMLInputElement>('[data-pane-show]').forEach((box) => {
      box.checked = paneVisible(box.dataset.paneShow as PaneKey);
    });
    toast('布局已恢复默认');
  });

  // 默认压缩参数
  const applyFmt = (): void => {
    const zip = modal.querySelector<HTMLSelectElement>('#st-format')?.value === 'zip';
    const volume = modal.querySelector<HTMLInputElement>('#st-volume');
    const encrypt = modal.querySelector<HTMLInputElement>('#st-encrypt');
    if (volume) volume.disabled = !!zip;
    if (encrypt) encrypt.disabled = !!zip;
  };
  modal.querySelector<HTMLSelectElement>('#st-format')?.addEventListener('change', applyFmt);
  applyFmt();

  modal.querySelector('#st-apply-project')?.addEventListener('click', () => {
    prefs.archiveDefaults = readArchivePage();
    persist();
    handlers.applyArchiveDefaultsToProject();
  });

  function readArchivePage(): ArchiveSettings {
    const num = (sel: string): number => Number(modal.querySelector<HTMLInputElement>(sel)?.value);
    const chk = (sel: string): boolean => !!modal.querySelector<HTMLInputElement>(sel)?.checked;
    return {
      format: modal.querySelector<HTMLSelectElement>('#st-format')?.value ?? 'rar',
      volume_mib: Math.max(0, num('#st-volume') || 900),
      comment: chk('#st-comment'),
      encrypt_names: chk('#st-encrypt'),
      overwrite: chk('#st-overwrite'),
      write_notes: chk('#st-writenotes'),
      incremental: chk('#st-incremental'),
    };
  }
}

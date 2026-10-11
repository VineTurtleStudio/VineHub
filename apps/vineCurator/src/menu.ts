/// 命令注册 + 快捷键（index.html 的 data-keys 为唯一来源）+ Ctrl+K 命令面板

export interface Command {
  id: string;
  label: string;
  keys: string[]; // 如 ['Ctrl+K']
  run: () => void | Promise<void>;
}

const commands = new Map<string, Command>();

export function registerCommand(cmd: Command): void {
  commands.set(cmd.id, cmd);
}

export function commandList(): Command[] {
  return [...commands.values()];
}

/// 解析 data-keys（"Ctrl+Shift+F" → 规范化键串）
function eventCombo(e: KeyboardEvent): string {
  const parts: string[] = [];
  if (e.ctrlKey) parts.push('Ctrl');
  if (e.altKey) parts.push('Alt');
  if (e.shiftKey) parts.push('Shift');
  if (e.metaKey) parts.push('Meta');
  const key = e.key.length === 1 ? e.key.toUpperCase() : e.key;
  if (key !== 'Control' && key !== 'Shift' && key !== 'Alt' && key !== 'Meta') parts.push(key);
  return parts.join('+');
}

function parseKeys(spec: string): string[] {
  return spec
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean)
    .map((combo) =>
      combo
        .split('+')
        .map((t) => t.trim())
        .map((t) => (t.length === 1 ? t.toUpperCase() : t))
        .join('+'),
    );
}

let declaredCache: Map<Command, string[]> = new Map();

/// 从 index.html 收集 data-cmd + data-keys 声明，绑定到已注册命令
export function bindDeclaredShortcuts(): Map<Command, string[]> {
  const declared = new Map<Command, string[]>();
  document.querySelectorAll<HTMLElement>('[data-cmd][data-keys]').forEach((el) => {
    const cmd = commands.get(el.dataset.cmd ?? '');
    if (!cmd) return;
    const keys = parseKeys(el.dataset.keys ?? '');
    if (keys.length > 0) declared.set(cmd, keys);
  });
  declaredCache = declared;
  return declared;
}

function keysOf(cmd: Command): string[] | undefined {
  if (declaredCache.size === 0) bindDeclaredShortcuts();
  return declaredCache.get(cmd);
}

export function installShortcutListener(declared: Map<Command, string[]>): void {
  const lookup = new Map<string, Command>();
  for (const [cmd, keys] of declared) {
    for (const k of keys) lookup.set(k, cmd);
  }
  window.addEventListener('keydown', (e) => {
    const target = e.target as HTMLElement | null;
    // 输入框中不拦截普通键，但 Ctrl+K / Ctrl+Shift+F 仍生效
    const combo = eventCombo(e);
    const cmd = lookup.get(combo);
    if (!cmd) return;
    const inInput =
      target &&
      (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable);
    if (inInput && !(e.ctrlKey && (e.key === 'k' || e.key === 'K' || e.key === 'F'))) return;
    e.preventDefault();
    void cmd.run();
  });
}

// ---------------------------------------------------------------- 命令面板

const paletteEl = document.getElementById('palette') as HTMLElement;
const paletteInput = document.getElementById('palette-input') as HTMLInputElement;
const paletteList = document.getElementById('palette-list') as HTMLElement;

let paletteIndex = 0;
let paletteFiltered: Command[] = [];

function renderPalette(): void {
  paletteList.innerHTML = '';
  paletteFiltered.forEach((cmd, i) => {
    const row = document.createElement('div');
    row.className = `palette-item${i === paletteIndex ? ' active' : ''}`;
    const label = document.createElement('span');
    label.textContent = cmd.label;
    const keys = document.createElement('span');
    keys.className = 'palette-keys';
    const declared = keysOf(cmd);
    keys.textContent = declared ? declared.join(' / ') : '';
    row.append(label, keys);
    row.addEventListener('click', () => {
      closePalette();
      void cmd.run();
    });
    paletteList.appendChild(row);
  });
}

function filterPalette(): void {
  const q = paletteInput.value.trim().toLowerCase();
  paletteFiltered = commandList().filter((c) => !q || c.label.toLowerCase().includes(q));
  paletteIndex = 0;
  renderPalette();
}

function openPalette(): void {
  paletteEl.hidden = false;
  paletteInput.value = '';
  filterPalette();
  paletteInput.focus();
}

function closePalette(): void {
  paletteEl.hidden = true;
}

export function togglePalette(): void {
  if (paletteEl.hidden) openPalette();
  else closePalette();
}

export function initPalette(): void {
  paletteInput.addEventListener('input', filterPalette);
  paletteInput.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      paletteIndex = Math.min(paletteIndex + 1, paletteFiltered.length - 1);
      renderPalette();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      paletteIndex = Math.max(paletteIndex - 1, 0);
      renderPalette();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const cmd = paletteFiltered[paletteIndex];
      if (cmd) {
        closePalette();
        void cmd.run();
      }
    } else if (e.key === 'Escape') {
      e.preventDefault();
      closePalette();
    }
  });
  paletteEl.addEventListener('click', (e) => {
    if (e.target === paletteEl) closePalette();
  });
}

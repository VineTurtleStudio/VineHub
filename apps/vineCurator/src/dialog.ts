/// 轻量模态对话框（零框架）：prompt / choose / confirm
/// WebView2 不支持 window.prompt，这里自绘；Promise 化方便 async 接线。

interface Option {
  value: string;
  label: string;
}

function overlay(): { root: HTMLElement; box: HTMLElement } {
  const root = document.createElement('div');
  root.className = 'dlg-overlay';
  const box = document.createElement('div');
  box.className = 'dlg-box';
  root.appendChild(box);
  document.body.appendChild(root);
  return { root, box };
}

function cleanup(root: HTMLElement): void {
  root.remove();
}

/// 单行文本输入；返回 null = 取消
export function promptText(title: string, initial = ''): Promise<string | null> {
  return new Promise((resolve) => {
    const { root, box } = overlay();
    const h = document.createElement('h3');
    h.className = 'dlg-title';
    h.textContent = title;
    const input = document.createElement('input');
    input.type = 'text';
    input.className = 'dlg-input';
    input.value = initial;
    const err = document.createElement('p');
    err.className = 'dlg-err';
    const row = document.createElement('div');
    row.className = 'dlg-actions';
    const ok = document.createElement('button');
    ok.type = 'button';
    ok.className = 'btn';
    ok.textContent = '确定';
    const cancel = document.createElement('button');
    cancel.type = 'button';
    cancel.className = 'btn-ghost';
    cancel.textContent = '取消';
    row.append(ok, cancel);
    box.append(h, input, err, row);

    let done = false;
    const finish = (v: string | null): void => {
      if (done) return;
      done = true;
      cleanup(root);
      resolve(v);
    };
    ok.addEventListener('click', () => finish(input.value));
    cancel.addEventListener('click', () => finish(null));
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') finish(input.value);
      else if (e.key === 'Escape') finish(null);
    });
    root.addEventListener('click', (e) => {
      if (e.target === root) finish(null);
    });
    input.focus();
    input.select();
  });
}

/// 单选列表；返回 null = 取消
export function chooseFrom(title: string, options: Option[]): Promise<string | null> {
  return new Promise((resolve) => {
    const { root, box } = overlay();
    const h = document.createElement('h3');
    h.className = 'dlg-title';
    h.textContent = title;
    box.append(h);
    let done = false;
    const finish = (v: string | null): void => {
      if (done) return;
      done = true;
      cleanup(root);
      resolve(v);
    };
    for (const opt of options) {
      const row = document.createElement('button');
      row.type = 'button';
      row.className = 'dlg-option';
      row.textContent = opt.label;
      row.addEventListener('click', () => finish(opt.value));
      box.appendChild(row);
    }
    const cancel = document.createElement('button');
    cancel.type = 'button';
    cancel.className = 'btn-ghost dlg-cancel';
    cancel.textContent = '取消';
    cancel.addEventListener('click', () => finish(null));
    box.appendChild(cancel);
    root.addEventListener('click', (e) => {
      if (e.target === root) finish(null);
    });
    root.tabIndex = -1;
    root.focus();
    root.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') finish(null);
    });
  });
}

/// 二选一确认；返回 false = 取消
export function confirmBox(title: string, detail = ''): Promise<boolean> {
  return new Promise((resolve) => {
    const { root, box } = overlay();
    const h = document.createElement('h3');
    h.className = 'dlg-title';
    h.textContent = title;
    box.appendChild(h);
    if (detail) {
      const p = document.createElement('p');
      p.className = 'dlg-detail';
      p.textContent = detail;
      box.appendChild(p);
    }
    const row = document.createElement('div');
    row.className = 'dlg-actions';
    const ok = document.createElement('button');
    ok.type = 'button';
    ok.className = 'btn danger';
    ok.textContent = '确认';
    const cancel = document.createElement('button');
    cancel.type = 'button';
    cancel.className = 'btn-ghost';
    cancel.textContent = '取消';
    row.append(ok, cancel);
    box.appendChild(row);
    let done = false;
    const finish = (v: boolean): void => {
      if (done) return;
      done = true;
      cleanup(root);
      resolve(v);
    };
    ok.addEventListener('click', () => finish(true));
    cancel.addEventListener('click', () => finish(false));
    root.addEventListener('click', (e) => {
      if (e.target === root) finish(false);
    });
    root.tabIndex = -1;
    root.focus();
    root.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') finish(false);
    });
  });
}

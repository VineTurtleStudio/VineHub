/// 分类树操作：新建 / 重命名 / 移动 / 删除（拖拽移动在 ui.ts 树渲染中接线）
import { chooseFrom, confirmBox, promptText } from '../dialog';
import {
  createCategory,
  deleteCategory,
  moveCategory,
  renameCategory,
  type CategoryNode,
} from '../api';
import { state } from '../state';

export type TreeRefresh = () => Promise<void>;

function requireSelectedCategoryId(): number | null {
  return state.sel.kind === 'category' ? state.sel.id : null;
}

function flatten(nodes: CategoryNode[], depth = 0): { id: number; label: string }[] {
  const out: { id: number; label: string }[] = [];
  for (const n of nodes) {
    out.push({ id: n.id, label: `${'　'.repeat(depth)}${n.name}` });
    out.push(...flatten(n.children, depth + 1));
  }
  return out;
}

/// 在选中分类下新建（未选分类则建顶层）
export async function newCategory(refresh: TreeRefresh): Promise<void> {
  const parentId = requireSelectedCategoryId();
  const name = await promptText(parentId ? '在选中分类下新建子分类' : '新建顶层分类');
  if (name === null) return;
  try {
    const id = await createCategory(parentId, name.trim());
    state.sel = { kind: 'category', id };
    await refresh();
  } catch (err) {
    window.dispatchEvent(new CustomEvent('vc:toast', { detail: [String(err), false] }));
  }
}

export async function renameSelectedCategory(refresh: TreeRefresh): Promise<void> {
  const id = requireSelectedCategoryId();
  if (id === null) return;
  const current = findNode(state.categories, id);
  const name = await promptText('重命名分类', current?.name ?? '');
  if (name === null) return;
  try {
    await renameCategory(id, name.trim());
    await refresh();
  } catch (err) {
    window.dispatchEvent(new CustomEvent('vc:toast', { detail: [String(err), false] }));
  }
}

/// 移动选中分类到另一父级（列出除自身与子孙外的全部节点）
export async function moveSelectedCategory(refresh: TreeRefresh): Promise<void> {
  const id = requireSelectedCategoryId();
  if (id === null) return;
  const excluded = new Set(descendantIds(state.categories, id));
  excluded.add(id);
  const options = flatten(state.categories).filter((n) => !excluded.has(n.id));
  options.unshift({ id: 0, label: '（顶层）' });
  const picked = await promptMoveChoice(options);
  if (picked === null) return;
  try {
    await moveCategory(id, picked === 0 ? null : picked);
    await refresh();
  } catch (err) {
    window.dispatchEvent(new CustomEvent('vc:toast', { detail: [String(err), false] }));
  }
}

async function promptMoveChoice(options: { id: number; label: string }[]): Promise<number | null> {
  const picked = await chooseFrom(
    '移动到',
    options.map((o) => ({ value: String(o.id), label: o.label })),
  );
  return picked === null ? null : Number(picked);
}

export async function deleteSelectedCategory(refresh: TreeRefresh): Promise<void> {
  const id = requireSelectedCategoryId();
  if (id === null) return;
  const node = findNode(state.categories, id);
  const ok = await confirmBox(
    `删除分类「${node?.name ?? id}」？`,
    '子分类与条目关联一并删除（级联），磁盘文件不受影响。',
  );
  if (!ok) return;
  try {
    await deleteCategory(id);
    if (state.sel.kind === 'category' && state.sel.id === id) state.sel = { kind: 'all' };
    await refresh();
  } catch (err) {
    window.dispatchEvent(new CustomEvent('vc:toast', { detail: [String(err), false] }));
  }
}

/// 拖拽移动：把 srcId 挂到 targetId（null = 顶层）
export async function dropCategory(
  srcId: number,
  targetId: number | null,
  refresh: TreeRefresh,
): Promise<void> {
  if (srcId === targetId) return;
  if (targetId !== null && descendantIds(state.categories, srcId).has(targetId)) {
    window.dispatchEvent(
      new CustomEvent('vc:toast', { detail: ['不能移动到自己的子孙下', false] }),
    );
    return;
  }
  try {
    await moveCategory(srcId, targetId);
    await refresh();
  } catch (err) {
    window.dispatchEvent(new CustomEvent('vc:toast', { detail: [String(err), false] }));
  }
}

function findNode(nodes: CategoryNode[], id: number): CategoryNode | null {
  for (const n of nodes) {
    if (n.id === id) return n;
    const hit = findNode(n.children, id);
    if (hit) return hit;
  }
  return null;
}

function descendantIds(nodes: CategoryNode[], id: number): Set<number> {
  const node = findNode(nodes, id);
  const out = new Set<number>();
  if (!node) return out;
  const walk = (n: CategoryNode): void => {
    for (const c of n.children) {
      out.add(c.id);
      walk(c);
    }
  };
  walk(node);
  return out;
}

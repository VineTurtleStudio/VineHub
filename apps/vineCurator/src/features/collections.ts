/// 智能集合：把当前树筛选「另存为集合」+ 详情面板标签编辑
import { chooseFrom, confirmBox, promptText } from '../dialog';
import { mergeTags, removeItemTags, saveCollection, listTags, setItemTags } from '../api';
import { state } from '../state';

/// 数据变更后请求全局刷新（main.ts 监听）
function requestRefresh(): void {
  window.dispatchEvent(new CustomEvent('vc:roots-changed'));
}

/// 当前树选择 → query_json（与 currentFilter 语义一致）
export function currentQueryJson(): string {
  const filter: Record<string, unknown> = {};
  const all: unknown[] = [];
  if (state.sel.kind === 'type') all.push({ field: 'type', op: 'eq', value: state.sel.value });
  // root / category 无法表达为 query_json 字段（category 可以：id）
  if (state.sel.kind === 'category') {
    all.push({ field: 'category', op: 'eq', value: state.sel.id });
  }
  if (all.length > 0) filter.all = all;
  const sortField =
    state.sort === 'type_name'
      ? 'type'
      : ['file_name', 'size', 'mtime', 'year', 'rating', 'status', 'rel_path'].includes(state.sort)
        ? state.sort
        : 'file_name';
  return JSON.stringify({
    ...(Object.keys(filter).length > 0 ? { filter } : {}),
    sort: [{ field: sortField, dir: state.desc ? 'desc' : 'asc' }],
    limit: 1000,
  });
}

/// 把当前筛选另存为智能集合
export async function saveCurrentAsCollection(): Promise<void> {
  const name = await promptText('保存为智能集合，输入名称');
  if (name === null) return;
  try {
    await saveCollection(null, name.trim(), currentQueryJson(), 0);
    requestRefresh();
    window.dispatchEvent(new CustomEvent('vc:toast', { detail: [`已保存集合「${name.trim()}」`] }));
  } catch (err) {
    window.dispatchEvent(new CustomEvent('vc:toast', { detail: [String(err), false] }));
  }
}

// ---------------------------------------------------------------- 详情标签编辑

/// 详情面板：为单条追加标签（逗号分隔）
export async function addTagsToItem(itemId: number, raw: string): Promise<boolean> {
  const names = raw
    .split(/[,，]/)
    .map((s) => s.trim())
    .filter(Boolean);
  if (names.length === 0) return false;
  await setItemTags([itemId], names);
  return true;
}

/// 详情面板：移除一个标签
export async function removeTagFromItem(itemId: number, tagId: number): Promise<void> {
  await removeItemTags([itemId], [tagId]);
}

/// 标签词表（供输入联想/管理）
export async function tagNames(): Promise<string[]> {
  const tags = await listTags();
  return tags.map((t) => t.name);
}

/// 合并标签对话框（src → dst）
export async function mergeTagsDialog(): Promise<void> {
  const tags = await listTags();
  if (tags.length < 2) {
    window.dispatchEvent(
      new CustomEvent('vc:toast', { detail: ['标签少于 2 个，无法合并', false] }),
    );
    return;
  }
  const src = await chooseFrom(
    '合并：选择被并入的标签（源）',
    tags.map((t) => ({ value: String(t.id), label: `${t.name}（${t.item_count}）` })),
  );
  if (src === null) return;
  const dst = await chooseFrom(
    '合并到（目标）',
    tags
      .filter((t) => String(t.id) !== src)
      .map((t) => ({ value: String(t.id), label: `${t.name}（${t.item_count}）` })),
  );
  if (dst === null) return;
  const ok = await confirmBox('合并标签？', '源标签的打标与同义词全部并入目标，源标签删除。');
  if (!ok) return;
  try {
    const n = await mergeTags(Number(src), Number(dst));
    window.dispatchEvent(new CustomEvent('vc:toast', { detail: [`已合并，影响 ${n} 处打标`] }));
    requestRefresh();
  } catch (err) {
    window.dispatchEvent(new CustomEvent('vc:toast', { detail: [String(err), false] }));
  }
}

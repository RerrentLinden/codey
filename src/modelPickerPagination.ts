export const MODEL_PICKER_PAGE_SIZE = 200;

// 同一份模型数组在连续输入时会被反复筛选。按数组引用缓存小写形式，
// 内容变化会产生新数组，因此不会把旧的小写结果套到新列表上。
const loweredModelCache = new WeakMap<readonly string[], readonly string[]>();

function loweredModels(models: readonly string[]): readonly string[] {
  const cached = loweredModelCache.get(models);
  if (cached) return cached;
  const lowered = models.map((model) => model.toLowerCase());
  loweredModelCache.set(models, lowered);
  return lowered;
}

export function filterModelOptions(
  models: readonly string[],
  query: string,
): readonly string[] {
  const normalizedQuery = query.trim().toLowerCase();
  if (!normalizedQuery) return models;
  const lowered = loweredModels(models);
  const matches: string[] = [];
  for (let index = 0; index < models.length; index += 1) {
    if (lowered[index].includes(normalizedQuery)) matches.push(models[index]);
  }
  return matches;
}

export function visibleModelOptions<T>(
  models: readonly T[],
  visibleCount: number,
): readonly T[] {
  return models.slice(0, Math.max(0, visibleCount));
}

export function nextVisibleModelCount(
  currentCount: number,
  totalCount: number,
): number {
  return Math.min(
    Math.max(0, currentCount) + MODEL_PICKER_PAGE_SIZE,
    Math.max(0, totalCount),
  );
}

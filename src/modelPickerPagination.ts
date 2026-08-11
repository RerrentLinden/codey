export const MODEL_PICKER_PAGE_SIZE = 200;

export function filterModelOptions(
  models: readonly string[],
  query: string,
): readonly string[] {
  const normalizedQuery = query.trim().toLowerCase();
  if (!normalizedQuery) return models;
  return models.filter((model) =>
    model.toLowerCase().includes(normalizedQuery)
  );
}

export function visibleModelOptions(
  models: readonly string[],
  visibleCount: number,
): readonly string[] {
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

export type ModelPage = { queryable: boolean; models: string[]; nextCursor: string | null };

export async function loadRequestLogModels(
  fetchPage: (afterModel?: string) => Promise<ModelPage>,
  active: () => boolean,
): Promise<string[] | null> {
  const models = new Set<string>();
  const cursors = new Set<string>();
  let cursor: string | undefined;
  while (active()) {
    const page = await fetchPage(cursor);
    if (!active()) return null;
    if (!page.queryable) return [];
    for (const model of page.models) models.add(model);
    if (!page.nextCursor) return [...models];
    if (cursors.has(page.nextCursor)) throw new Error("模型候选分页游标未前进");
    cursors.add(page.nextCursor);
    cursor = page.nextCursor;
  }
  return null;
}

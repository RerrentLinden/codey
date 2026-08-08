export const modelKey = (model: string) => model.trim().toLowerCase();

export const modelIdsEqual = (left: string, right: string) =>
  modelKey(left) === modelKey(right);

export const includesModelId = (
  models: readonly string[],
  expected: string,
) => {
  const expectedKey = modelKey(expected);
  return Boolean(expectedKey) && models.some((model) => modelKey(model) === expectedKey);
};

export const uniqueModelIds = (models: readonly string[]) => {
  const seenKeys = new Set<string>();
  return models.reduce<string[]>((unique, model) => {
    const normalized = model.trim();
    const key = modelKey(normalized);
    if (key && !seenKeys.has(key)) {
      seenKeys.add(key);
      unique.push(normalized);
    }
    return unique;
  }, []);
};

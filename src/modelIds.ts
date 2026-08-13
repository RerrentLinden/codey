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

export const withoutModelId = (
  models: readonly string[],
  excluded: string,
) => {
  const excludedKey = modelKey(excluded);
  return models.filter((model) => modelKey(model) !== excludedKey);
};

export const partitionModelIdsByKey = (
  models: readonly string[],
  matchingKeys: ReadonlySet<string>,
) => {
  const matching: string[] = [];
  const remaining: string[] = [];
  for (const model of models) {
    (matchingKeys.has(modelKey(model)) ? matching : remaining).push(model);
  }
  return { matching, remaining };
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

import type { Config, ModelState, Profile } from "./App.types";
import { modelKey, uniqueModelIds } from "./modelIds";
import { routeModelAlias, routeProviderId } from "./modelRoutes";
import { routeDisplayPrefix } from "./routeShortNames";

const THIRD_PARTY_REASONING_EFFORTS = ["low", "medium", "high", "xhigh"];

export type SubagentModelOption = {
  value: string;
  label: string;
  modelId: string;
  routeId: string;
  providerId: string;
  routeName: string;
  routePrefix: string;
  official: boolean;
  supportedReasoningEfforts: string[];
  defaultReasoningEffort: string;
};

function enabledModelsForRoute(
  config: Config,
  modelState: ModelState,
  profile: Profile,
) {
  const providerId = routeProviderId(profile);
  const configuredModels = config.selectedModelsByProvider[providerId] || [];
  if (profile.authMode !== "officialAccount") {
    return uniqueModelIds([
      ...configuredModels,
      ...(config.declaredOfficialModelsByProvider[providerId] || []),
    ]);
  }

  const fallbackModels = uniqueModelIds([
    ...modelState.officialModelIds,
    ...modelState.officialModels.map((model) => model.slug),
  ]);
  return uniqueModelIds(configuredModels.length > 0 ? configuredModels : fallbackModels);
}

export function buildSubagentModelOptions(
  config: Config | null,
  modelState: ModelState,
  officialAccountAvailable: boolean,
) {
  if (!config) return [];

  const officialMetadata = new Map(
    modelState.officialModels.map((model) => [modelKey(model.slug), model]),
  );
  const seenAliases = new Set<string>();
  const options: SubagentModelOption[] = [];

  for (const profile of config.profiles) {
    const official = profile.authMode === "officialAccount";
    if (official && !officialAccountAvailable) continue;

    const providerId = routeProviderId(profile);
    for (const modelId of enabledModelsForRoute(config, modelState, profile)) {
      const value = routeModelAlias(profile, modelId);
      const valueKey = modelKey(value);
      if (seenAliases.has(valueKey)) continue;
      seenAliases.add(valueKey);

      const metadata = officialMetadata.get(modelKey(modelId));
      const usesOfficialMetadata = official;
      const efforts = usesOfficialMetadata && metadata
        ? metadata.supportedReasoningEfforts
        : THIRD_PARTY_REASONING_EFFORTS;
      const supportedReasoningEfforts = efforts.length > 0 ? efforts : ["low"];
      const requestedDefaultEffort =
        usesOfficialMetadata && metadata
          ? metadata.defaultReasoningEffort || supportedReasoningEfforts[0]
          : "low";
      options.push({
        value,
        label: usesOfficialMetadata && metadata ? metadata.displayName : modelId,
        modelId,
        routeId: profile.id,
        providerId,
        routeName: profile.name.trim() || providerId,
        routePrefix: routeDisplayPrefix(profile),
        official,
        supportedReasoningEfforts,
        defaultReasoningEffort:
          supportedReasoningEfforts.includes(requestedDefaultEffort)
            ? requestedDefaultEffort
            : supportedReasoningEfforts[0],
      });
    }
  }

  return options;
}

export function resolveSubagentModelOption(
  options: readonly SubagentModelOption[],
  requestedValue: string,
  preferredProviderId?: string,
) {
  const requested = requestedValue.trim();
  if (!requested) return undefined;
  const requestedKey = modelKey(requested);

  let preferredRouteMatch: SubagentModelOption | undefined;
  let legacyMatch: SubagentModelOption | undefined;
  let legacyMatchIsUnique = true;
  for (const option of options) {
    if (modelKey(option.value) === requestedKey) return option;
    if (modelKey(option.modelId) !== requestedKey) continue;
    if (!preferredRouteMatch && option.providerId === preferredProviderId) {
      preferredRouteMatch = option;
    }
    if (legacyMatch) {
      legacyMatchIsUnique = false;
    } else {
      legacyMatch = option;
    }
  }
  return preferredRouteMatch || (legacyMatchIsUnique ? legacyMatch : undefined);
}

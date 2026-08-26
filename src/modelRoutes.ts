import type { Config, Profile } from "./App.types";
import { modelIdsEqual } from "./modelIds";

export function routeProviderId(profile: Profile) {
  return profile.sourceProviderId || profile.id;
}

function encodeRouteComponent(value: string) {
  const bytes = new TextEncoder().encode(value.trim());
  return Array.from(bytes, (byte) => {
    const char = String.fromCharCode(byte);
    return /[A-Za-z0-9._-]/.test(char)
      ? char
      : `%${byte.toString(16).toUpperCase().padStart(2, "0")}`;
  }).join("");
}

export function routeModelAlias(profile: Profile, model: string) {
  const normalized = model.trim();
  return `${encodeRouteComponent(routeProviderId(profile))}/${normalized}`;
}

export function globalDefaultForRoute(
  config: Config,
  profile: Profile,
  models: string[],
) {
  return models.find((model) =>
    modelIdsEqual(routeModelAlias(profile, model), config.defaultModel),
  ) || "";
}

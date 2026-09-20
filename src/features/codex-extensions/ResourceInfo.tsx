import type { EditorDraft, Inventory } from "./types";
import { configurationLabel, dependencyStatus, scopeLabel } from "./state";

export function ResourceInfo({
  entry,
  inventory,
}: {
  entry: NonNullable<EditorDraft["entry"]>;
  inventory?: Inventory | null;
}) {
  const scope = entry.scope ?? inventory?.scope.kind;
  const dependents =
    "transport" in entry
      ? (inventory?.skills.filter(
          (skill) =>
            skill.scope === scope &&
            skill.dependencies?.some(
              (dependency) =>
                dependency.type.toLowerCase() === "mcp" &&
                (dependency.name === entry.name ||
                  dependency.name === entry.id),
            ),
        ) ?? [])
      : [];
  return (
    <section
      className="space-y-2 rounded-lg border border-default p-3 text-xs"
      aria-label="资源信息"
    >
      <h4 className="m-0 text-sm">资源信息</h4>
      <p className="m-0 break-all">
        来源：
        {"origin" in entry && entry.origin ? entry.origin : entry.sourcePath}
      </p>
      <p className="m-0">
        范围：{scopeLabel(scope)} · 配置：
        {configurationLabel(entry.configurationStatus)}
      </p>
      {entry.reason && <p className="m-0 text-muted">{entry.reason}</p>}
      {"version" in entry && entry.version && (
        <p className="m-0">版本：{entry.version}</p>
      )}
      {entry.updatedAt && (
        <p className="m-0">
          更新时间：{new Date(entry.updatedAt).toLocaleString("zh-CN")}
        </p>
      )}
      <p className="m-0 font-medium">
        {"dependencies" in entry || "ownership" in entry
          ? "声明的依赖"
          : "引用此 MCP 的 Skill"}
      </p>
      <p className="m-0 text-muted">
        仅展示当前范围内的资源声明，不代表会话中的实际调用关系。
      </p>
      {"ownership" in entry ? (
        <>
          {entry.dependencies?.length ? (
            <ul className="pl-4">
              {entry.dependencies.map((dependency, index) => (
                <li key={index} className="break-words">
                  {dependency.type} · {dependency.name}：
                  {dependencyStatus(dependency, scope, inventory)}
                </li>
              ))}
            </ul>
          ) : (
            <p className="m-0 text-muted">未声明依赖。</p>
          )}
          {entry.dependencyWarnings?.map((warning, index) => (
            <p key={index} className="m-0 text-warning">
              {warning}
            </p>
          ))}
        </>
      ) : dependents.length ? (
        <ul className="pl-4">
          {dependents.map((skill) => (
            <li key={skill.id} className="break-words">
              {skill.name} ·{" "}
              {skill.enabledKnown === false
                ? "状态待确认"
                : skill.enabled
                  ? "已启用"
                  : "已禁用"}
            </li>
          ))}
        </ul>
      ) : (
        <p className="m-0 text-muted">当前范围未发现 Skill 声明此依赖。</p>
      )}
    </section>
  );
}

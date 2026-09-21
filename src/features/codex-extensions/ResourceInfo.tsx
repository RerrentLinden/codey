import type { EditorDraft, Inventory } from "./types";
import { configurationLabel, dependencyStatus, scopeLabel } from "./state";
import { IconInfoCircle } from "@tabler/icons-react";

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
      className="space-y-2.5 rounded-xl border border-black/[0.08] bg-black/[0.02] p-3.5 text-xs dark:border-white/[0.08] dark:bg-white/[0.03]"
      aria-label="资源信息"
    >
      <div className="flex items-center gap-1.5 font-semibold text-foreground">
        <IconInfoCircle size={15} className="text-blue-600 dark:text-blue-400" />
        <span>资源信息</span>
      </div>
      <div className="space-y-1 text-muted">
        <p className="m-0 break-all">
          <span className="font-medium text-foreground">来源：</span>
          {"origin" in entry && entry.origin ? entry.origin : entry.sourcePath}
        </p>
        <p className="m-0">
          <span className="font-medium text-foreground">范围：</span>{scopeLabel(scope)} ·{" "}
          <span className="font-medium text-foreground">配置：</span>
          {configurationLabel(entry.configurationStatus)}
        </p>
        {entry.reason && <p className="m-0 text-muted">{entry.reason}</p>}
        {"version" in entry && entry.version && (
          <p className="m-0">
            <span className="font-medium text-foreground">版本：</span>{entry.version}
          </p>
        )}
        {entry.updatedAt && (
          <p className="m-0">
            <span className="font-medium text-foreground">更新时间：</span>
            {new Date(entry.updatedAt).toLocaleString("zh-CN")}
          </p>
        )}
      </div>
      <div className="border-t border-black/[0.06] pt-2 dark:border-white/[0.06]">
        <p className="m-0 font-medium text-foreground">
          {"dependencies" in entry || "ownership" in entry
            ? "声明的依赖"
            : "引用此 MCP 的 Skill"}
        </p>
        <p className="m-0 text-[11px] text-muted">
          仅展示当前范围内的资源声明，不代表会话中的实际调用关系。
        </p>
        {"ownership" in entry ? (
          <>
            {entry.dependencies?.length ? (
              <ul className="mt-1.5 space-y-1 pl-4 text-muted">
                {entry.dependencies.map((dependency, index) => (
                  <li key={index} className="break-words">
                    <span className="font-mono text-foreground">
                      {dependency.type} · {dependency.name}
                    </span>
                    ：{dependencyStatus(dependency, scope, inventory)}
                  </li>
                ))}
              </ul>
            ) : (
              <p className="m-0 mt-1 text-muted">未声明依赖。</p>
            )}
            {entry.dependencyWarnings?.map((warning, index) => (
              <p key={index} className="m-0 mt-1 text-warning">
                {warning}
              </p>
            ))}
          </>
        ) : dependents.length ? (
          <ul className="mt-1.5 space-y-1 pl-4 text-muted">
            {dependents.map((skill) => (
              <li key={skill.id} className="break-words">
                <span className="font-medium text-foreground">{skill.name}</span> ·{" "}
                {skill.enabledKnown === false
                  ? "状态待确认"
                  : skill.enabled
                    ? "已启用"
                    : "已禁用"}
              </li>
            ))}
          </ul>
        ) : (
          <p className="m-0 mt-1 text-muted">当前范围未发现 Skill 声明此依赖。</p>
        )}
      </div>
    </section>
  );
}

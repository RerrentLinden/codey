import { useEffect, useRef } from "react";
import { IconAlertCircle } from "@tabler/icons-react";

export function ExtensionError({
  message,
  draftPreserved = false,
}: {
  message: string;
  draftPreserved?: boolean;
}) {
  const element = useRef<HTMLDivElement>(null);
  useEffect(() => {
    element.current?.scrollIntoView({ block: "nearest" });
  }, [message]);
  return (
    <div
      ref={element}
      role="alert"
      className="rounded-xl border border-danger/25 bg-danger/10 p-3.5 text-xs text-danger"
    >
      <div className="flex items-start gap-2">
        <IconAlertCircle size={16} className="mt-0.5 shrink-0" />
        <div className="flex-1 space-y-1">
          <div className="font-medium">{message}</div>
          {draftPreserved && (
            <p className="m-0 text-[11px] opacity-80">
              草稿已保留，处理上述问题后可以继续编辑。
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

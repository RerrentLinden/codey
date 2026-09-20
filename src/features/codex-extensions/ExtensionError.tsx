import { useEffect, useRef } from "react";

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
      className="rounded-lg border border-danger/30 p-3 text-sm text-danger"
    >
      {message}
      {draftPreserved && (
        <p className="mb-0 text-xs">草稿已保留，处理上述问题后可以继续编辑。</p>
      )}
    </div>
  );
}

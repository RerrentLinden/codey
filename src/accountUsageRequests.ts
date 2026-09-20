import type { AccountUsageSnapshot } from "./quotaEstimate";

type UsageRequest = {
  forceRefresh: boolean;
  promise: Promise<AccountUsageSnapshot>;
};

// 同一账号只执行一次读取；强制刷新排在普通读取之后，不复用旧结果。
export function createAccountUsageReader(
  query: (accountId: string | undefined, forceRefresh: boolean) => Promise<AccountUsageSnapshot>,
) {
  const requests = new Map<string, UsageRequest>();
  return (accountId: string | undefined, forceRefresh: boolean) => {
    const key = accountId ?? "";
    const pending = requests.get(key);
    if (pending && (!forceRefresh || pending.forceRefresh)) return pending.promise;

    const run = () => query(accountId, forceRefresh);
    const request: UsageRequest = {
      forceRefresh,
      promise: pending
        ? pending.promise.then(run, run)
        : Promise.resolve().then(run),
    };
    requests.set(key, request);
    const clear = () => {
      if (requests.get(key) === request) requests.delete(key);
    };
    void request.promise.then(clear, clear);
    return request.promise;
  };
}

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

test("maintenance metrics cross the backend boundary as structured fields", async () => {
  const [launcher, types, operationsPanel] = await Promise.all([
    readFile(new URL("backend/src/launcher.rs", root), "utf8"),
    readFile(new URL("src/App.types.ts", root), "utf8"),
    readFile(new URL("src/OperationsPanel.tsx", root), "utf8"),
  ]);

  for (const [rustField, wireField] of [
    ["session_files_fixed", "sessionFilesFixed"],
    ["sqlite_rows_updated", "sqliteRowsUpdated"],
    ["ghost_tasks_pruned", "ghostTasksPruned"],
  ]) {
    assert.match(launcher, new RegExp(`pub ${rustField}: usize`));
    assert.match(types, new RegExp(`${wireField}\\?: number`));
    assert.match(operationsPanel, new RegExp(`maintenance\\?\\.${wireField}`));
  }

  assert.doesNotMatch(operationsPanel, /sessionDetailStr\.match/);
  assert.doesNotMatch(operationsPanel, /\.match\(\/修复/);
  assert.doesNotMatch(operationsPanel, /\.match\(\/更新/);
  assert.doesNotMatch(operationsPanel, /\.match\(\/清理/);
});

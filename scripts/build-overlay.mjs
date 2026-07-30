import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const vite = join(root, "node_modules", "vite", "bin", "vite.js");

const result = spawnSync(
  process.execPath,
  [vite, "build", "--config", "vite.overlay.config.ts"],
  {
    cwd: root,
    stdio: "inherit",
  },
);
if (result.error) throw result.error;
if (result.status !== 0) process.exit(result.status ?? 1);

import { spawnSync } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const debugExe = join(root, "target", "debug", "codey.exe").toLowerCase();
const releaseExe = join(root, "target", "release", "codey.exe").toLowerCase();

function stopLockedCodeyOnWindows() {
  if (process.platform !== "win32") return;

  // Windows locks the running EXE, so cargo cannot overwrite target/debug/codey.exe.
  const listed = spawnSync(
    "powershell.exe",
    [
      "-NoProfile",
      "-Command",
      [
        "$paths = @(",
        `  '${debugExe.replace(/'/g, "''")}',`,
        `  '${releaseExe.replace(/'/g, "''")}'`,
        ")",
        "Get-CimInstance Win32_Process -Filter \"Name = 'codey.exe'\" -ErrorAction SilentlyContinue |",
        "  Where-Object {",
        "    $_.ExecutablePath -and",
        "    $paths -contains ([string]$_.ExecutablePath).ToLowerInvariant()",
        "  } |",
        "  ForEach-Object {",
        "    Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue",
        "    Write-Output $_.ProcessId",
        "  }",
      ].join(" "),
    ],
    { encoding: "utf8" },
  );

  const stopped = (listed.stdout || "")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  if (stopped.length > 0) {
    console.log(
      `[dev] 已结束仍在运行的 Codey 进程（避免锁定 codey.exe）：${stopped.join(", ")}`,
    );
  }
}

stopLockedCodeyOnWindows();

const cargo = spawnSync(
  "cargo",
  ["run", "--manifest-path", join(root, "Cargo.toml"), ...process.argv.slice(2)],
  { cwd: root, stdio: "inherit", shell: process.platform === "win32" },
);

process.exit(cargo.status ?? 1);
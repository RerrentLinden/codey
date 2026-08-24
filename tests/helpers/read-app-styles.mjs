import { readFile } from "node:fs/promises";

const APP_STYLE_FILES = [
  "src/styles.css",
  "src/styles.operations.css",
  "src/styles.models.css",
  "src/styles.features.css",
  "src/styles.diagnostics.css",
  "src/styles.responsive.css",
];

export async function readAppStyles(root) {
  const sources = await Promise.all(
    APP_STYLE_FILES.map((file) => readFile(new URL(file, root), "utf8")),
  );
  return sources.join("\n");
}

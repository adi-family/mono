import { existsSync } from "node:fs";

// The behavior suite has its own small install; existing docs installs also work locally.
export async function launchElementsBrowser() {
  let chromium;
  for (const path of ["./elements-tests/node_modules/playwright/index.mjs", "playwright", "../apps/docs/node_modules/playwright/index.mjs"]) {
    try {
      ({ chromium } = await import(path));
      break;
    } catch (error) {
      if (error.code !== "ERR_MODULE_NOT_FOUND") throw error;
    }
  }
  if (!chromium) throw new Error("Install browser tests with: npm ci --prefix scripts/elements-tests");
  const localChrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
  const executablePath = process.env.ELEMENTS_CHROME || (existsSync(localChrome) ? localChrome : undefined);
  return chromium.launch({
    executablePath,
    headless: true,
    args: ["--allow-file-access-from-files"],
  });
}

import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";

rmSync("./dist", { recursive: true, force: true });
mkdirSync("./dist", { recursive: true });

const res = await Bun.build({
  entrypoints: ["./src/app.ts"],
  outdir: "./dist",
  minify: true,
  target: "browser",
});

if (!res.success) {
  console.error("Web build failed:", res.logs);
  process.exit(1);
}

const bundleBytes = readFileSync("./dist/app.js");
const hash = createHash("sha256").update(bundleBytes).digest("hex").slice(0, 10);
const hashedBundleName = `app.${hash}.js`;

writeFileSync(`./dist/${hashedBundleName}`, bundleBytes);

if (existsSync("../assets/logo.svg")) {
  copyFileSync("../assets/logo.svg", "./dist/logo.svg");
}
if (existsSync("../assets/icon-512.png")) {
  copyFileSync("../assets/icon-512.png", "./dist/apple-touch-icon.png");
}

const html = readFileSync("./index.html", "utf8")
  .replace("/app.js", `/${hashedBundleName}`)
  .replace("/logo.svg", `/logo.svg?v=${hash}`)
  .replace("/apple-touch-icon.png", `/apple-touch-icon.png?v=${hash}`);

writeFileSync("./dist/index.html", html, "utf8");
console.log(`Built Coda Web Console -> web/dist/ (${hashedBundleName})`);

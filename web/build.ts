import { mkdirSync, copyFileSync, existsSync } from "node:fs";

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

copyFileSync("./index.html", "./dist/index.html");
if (existsSync("../assets/logo.svg")) {
  copyFileSync("../assets/logo.svg", "./dist/logo.svg");
}
if (existsSync("../assets/icon-512.png")) {
  copyFileSync("../assets/icon-512.png", "./dist/apple-touch-icon.png");
}
console.log("Built Coda Web Console -> web/dist/");

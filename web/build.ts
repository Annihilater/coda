import { mkdirSync, copyFileSync } from "node:fs";

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
console.log("Built Coda Web Console -> web/dist/");

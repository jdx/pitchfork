import { copyFileSync, readFileSync, writeFileSync } from "node:fs";
import sharp from "sharp";
import { socialCard } from "./social-images.mjs";

// Keep every public logo export derived from the approved artwork.
const root = new URL("../../", import.meta.url);
const source = new URL("docs/public/img/logo.png", root);
const data = readFileSync(source);
const render = (size) => sharp(data).resize(size, size).png().toBuffer();

for (const path of ["logo.png", "docs/public/img/small.png"]) {
  copyFileSync(source, new URL(path, root));
}

const exports = new Map([
  [16, ["docs/public/img/favicon-16x16.png"]],
  [32, ["docs/public/img/favicon-32x32.png", "ui/public/favicon.png"]],
  [
    180,
    ["docs/public/img/apple-touch-icon.png", "ui/public/apple-touch-icon.png"],
  ],
  [
    192,
    [
      "docs/public/img/android-chrome-192x192.png",
      "ui/public/pwa-192.png",
      "ui/public/img/logo.png",
    ],
  ],
  [256, ["docs/public/img/favicon.png"]],
  [512, ["docs/public/img/android-chrome-512x512.png", "ui/public/pwa-512.png"]],
]);
for (const [size, paths] of exports) {
  const png = await render(size);
  for (const path of paths) writeFileSync(new URL(path, root), png);
}

// ICO supports PNG frames. Include native small sizes and a 48px fallback.
const sizes = [16, 32, 48];
const frames = await Promise.all(sizes.map(render));
const directory = Buffer.alloc(6 + sizes.length * 16);
directory.writeUInt16LE(1, 2);
directory.writeUInt16LE(sizes.length, 4);
let offset = directory.length;
frames.forEach((png, i) => {
  const entry = 6 + i * 16;
  directory[entry] = sizes[i];
  directory[entry + 1] = sizes[i];
  directory.writeUInt16LE(1, entry + 4);
  directory.writeUInt16LE(32, entry + 6);
  directory.writeUInt32LE(png.length, entry + 8);
  directory.writeUInt32LE(offset, entry + 12);
  offset += png.length;
});
writeFileSync(
  new URL("docs/public/img/favicon.ico", root),
  Buffer.concat([directory, ...frames]),
);
writeFileSync(
  new URL("docs/public/img/og.png", root),
  socialCard("Daemons with DX").png,
);
console.log("Rendered branding assets from docs/public/img/logo.png");

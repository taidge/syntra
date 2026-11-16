// Package browser PDF dependencies and the OFL CJK font with the normal web output.
import { cp, copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';
import subsetFont from 'subset-font';
import fontkit from '@pdf-lib/fontkit';

const root = dirname(fileURLToPath(import.meta.url));
const output = process.argv[2];
if (!output) throw new Error('Pass the browser assets output directory.');
const vendor = join(resolve(output), 'pdf-vendor');
await mkdir(vendor, { recursive: true });
for (const [source, target] of [
  ['pdfjs-dist/legacy/build/pdf.mjs', 'pdf.mjs'],
  ['pdfjs-dist/legacy/build/pdf.worker.mjs', 'pdf.worker.mjs'],
  ['pdfjs-dist/web/pdf_viewer.css', 'pdf_viewer.css'],
  ['pdfjs-dist/LICENSE', 'PDFJS-LICENSE.txt'],
  ['pdf-lib/dist/pdf-lib.esm.min.js', 'pdf-lib.esm.min.js'],
  ['pdf-lib/LICENSE.md', 'PDF-LIB-LICENSE.txt'],
  ['@pdf-lib/fontkit/README.md', 'FONTKIT-README.txt'],
]) await copyFile(join(root, 'node_modules', source), join(vendor, target));
// fontkit's published ESM leaves pako external. Bundle it for native browser
// modules, which cannot resolve npm bare specifiers without a runtime CDN.
await build({ entryPoints: [join(root, 'node_modules/@pdf-lib/fontkit/dist/fontkit.es.js')], bundle: true, format: 'esm', platform: 'browser', target: 'es2022', minify: true, outfile: join(vendor, 'fontkit.es.min.js'), legalComments: 'eof' });
for (const directory of ['cmaps', 'standard_fonts', 'wasm']) {
  await cp(join(root, 'node_modules/pdfjs-dist', directory), join(vendor, directory), { recursive: true });
}
const ocr = join(vendor, 'tesseract');
await mkdir(join(ocr, 'langs'), { recursive: true });
for (const file of ['tesseract.esm.min.js', 'worker.min.js']) {
  await copyFile(join(root, 'node_modules/tesseract.js/dist', file), join(ocr, file));
}
await copyFile(join(root, 'node_modules/tesseract.js/LICENSE.md'), join(ocr, 'LICENSE.txt'));
await cp(join(root, 'node_modules/tesseract.js-core'), join(ocr, 'core'), { recursive: true });
for (const language of ['eng', 'chi_sim']) {
  await copyFile(join(root, `node_modules/@tesseract.js-data/${language}/4.0.0_best_int/${language}.traineddata.gz`), join(ocr, 'langs', `${language}.traineddata.gz`));
}

// Pin both source commit and bytes; this font is fetched at build time only.
// No PDF, annotation, or document data is sent to the font source.
const revision = 'f8d157532fbfaeda587e826d4cd5b21a49186f7c';
const source = `https://raw.githubusercontent.com/notofonts/noto-cjk/${revision}/Sans/`;
const fontOutput = join(vendor, 'office-cjk.ttf');
const staticHash = '4026d33af9acba66dbd6ae2cc11d015a204ccce96cfb1584ed706674fba56276';
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
if (!await readFile(fontOutput).then(bytes => digest(bytes) === staticHash, () => false)) {
  const response = await fetch(source + 'Variable/TTF/Subset/NotoSansSC-VF.ttf', { signal: AbortSignal.timeout(120000) });
  if (!response.ok) throw new Error(`Unable to fetch bundled PDF font: HTTP ${response.status}`);
  const raw = Buffer.from(await response.arrayBuffer());
  if (digest(raw) !== 'd68bafcb48a2707749396aa12bbbd833cb70401f3a9a689fd2902c7e0d295964') throw new Error('Bundled PDF font source checksum mismatch');
  // pdf-lib's fontkit cannot reliably subset variable/composite CJK outlines.
  // Pin the font to a regular, static TrueType instance using HarfBuzz WASM.
  // Shared glyphs must not advertise lower-codepoint radical/compatibility
  // aliases: Chrome may choose those aliases in an exported PDF's ToUnicode map.
  // Ideographic description characters U+2FF0..2FFB are distinct and retained.
  const compatibilityAlias = point => (point >= 0x2e80 && point <= 0x2fdf)
    || (point >= 0xf900 && point <= 0xfaff) || (point >= 0x2f800 && point <= 0x2fa1f);
  const canonical = fontkit.create(raw).characterSet.filter(point => !compatibilityAlias(point));
  const regular = await subsetFont(raw, canonical.map(point => String.fromCodePoint(point)).join(''), { targetFormat: 'sfnt', variationAxes: { wght: 400 } });
  if (digest(regular) !== staticHash) throw new Error('Bundled static PDF font checksum mismatch');
  await writeFile(fontOutput, regular);
}
for (const [path, name, sha256] of [
  ['LICENSE', 'OFL.txt', '6a73f9541c2de74158c0e7cf6b0a58ef774f5a780bf191f2d7ec9cc53efe2bf2'],
]) {
  const target = join(vendor, name);
  const valid = bytes => createHash('sha256').update(bytes).digest('hex') === sha256;
  if (await readFile(target).then(valid, () => false)) continue;
  const response = await fetch(source + path, { signal: AbortSignal.timeout(120000) });
  if (!response.ok) throw new Error(`Unable to fetch bundled PDF font: HTTP ${response.status}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  if (!valid(bytes)) throw new Error(`Bundled PDF font checksum mismatch: ${name}`);
  await writeFile(target, bytes);
}
console.log(`PDF assets: ${vendor}`);

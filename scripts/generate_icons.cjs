#!/usr/bin/env node
/**
 * Generates a minimal set of placeholder icons for the Tauri bundler.
 * This script uses only Node's built-in modules and produces valid PNGs
 * with the BeeAPI accent color. The bundler also accepts an .ico file,
 * which we synthesize as a 32x32 single-image ICO wrapping the PNG bytes.
 *
 * Run:  node scripts/generate_icons.js
 *
 * Output goes into src-tauri/icons/.
 * Replace these later with a designer-provided icon set.
 */
const fs = require("node:fs");
const path = require("node:path");
const zlib = require("node:zlib");

const OUT_DIR = path.resolve(__dirname, "..", "src-tauri", "icons");

// BeeAPI accent color.
const ACCENT = { r: 0xf5, g: 0xb4, b: 0x00 };
const DARK = { r: 0x0f, g: 0x11, b: 0x15 };

function makePngBuffer(size) {
  const w = size;
  const h = size;
  const raw = Buffer.alloc(h * (1 + w * 4));

  // A rounded square + centered "B" stroke approximation.
  const r = Math.floor(size * 0.22);
  const stroke = Math.max(1, Math.floor(size * 0.08));
  const cx = Math.floor(size / 2);
  const cy = Math.floor(size / 2);
  const innerR = Math.floor(size * 0.32);

  for (let y = 0; y < h; y++) {
    raw[y * (1 + w * 4)] = 0; // filter type
    for (let x = 0; x < w; x++) {
      const off = y * (1 + w * 4) + 1 + x * 4;

      // Rounded-square mask.
      const inside =
        x >= r &&
        x < w - r &&
        y >= 0 &&
        y < h
          ? true
          : false;
      const topLeft = Math.hypot(x - r, y - r) <= r;
      const topRight = Math.hypot(x - (w - r - 1), y - r) <= r;
      const btmLeft = Math.hypot(x - r, y - (h - r - 1)) <= r;
      const btmRight = Math.hypot(x - (w - r - 1), y - (h - r - 1)) <= r;

      const inY = y >= r && y < h - r;
      const inX = x >= r && x < w - r;
      let isSquare = false;
      if (inX && y >= 0 && y < h) isSquare = true;
      else if (inY && x >= 0 && x < w) isSquare = true;
      else if (x < r && y < r && topLeft) isSquare = true;
      else if (x >= w - r && y < r && topRight) isSquare = true;
      else if (x < r && y >= h - r && btmLeft) isSquare = true;
      else if (x >= w - r && y >= h - r && btmRight) isSquare = true;

      // Circle (bee body) in the center.
      const dx = x - cx;
      const dy = y - cy;
      const dist = Math.hypot(dx, dy);
      const inBody = dist <= innerR;
      const bodyStripe =
        inBody &&
        Math.abs((dx + dy) % Math.max(2, Math.floor(size * 0.12))) <
          Math.max(1, Math.floor(size * 0.05));

      if (!isSquare || !inside) {
        raw[off] = 0;
        raw[off + 1] = 0;
        raw[off + 2] = 0;
        raw[off + 3] = 0;
        continue;
      }

      if (inBody) {
        if (bodyStripe) {
          raw[off] = DARK.r;
          raw[off + 1] = DARK.g;
          raw[off + 2] = DARK.b;
          raw[off + 3] = 255;
        } else {
          raw[off] = 0xff;
          raw[off + 1] = 0xd7;
          raw[off + 2] = 0x33;
          raw[off + 3] = 255;
        }
      } else {
        raw[off] = ACCENT.r;
        raw[off + 1] = ACCENT.g;
        raw[off + 2] = ACCENT.b;
        raw[off + 3] = 255;
      }

      // Dark outline for crispness near the body edge.
      if (Math.abs(dist - innerR) < stroke / 2) {
        raw[off] = DARK.r;
        raw[off + 1] = DARK.g;
        raw[off + 2] = DARK.b;
        raw[off + 3] = 255;
      }
    }
  }

  const compressed = zlib.deflateSync(raw, { level: 9 });

  // PNG chunks.
  const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr.writeUInt8(8, 8); // bit depth
  ihdr.writeUInt8(6, 9); // color type: RGBA
  ihdr.writeUInt8(0, 10);
  ihdr.writeUInt8(0, 11);
  ihdr.writeUInt8(0, 12);

  function chunk(type, data) {
    const len = Buffer.alloc(4);
    len.writeUInt32BE(data.length, 0);
    const typeBuf = Buffer.from(type, "ascii");
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(Buffer.concat([typeBuf, data])), 0);
    return Buffer.concat([len, typeBuf, data, crc]);
  }

  return Buffer.concat([
    sig,
    chunk("IHDR", ihdr),
    chunk("IDAT", compressed),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

function crc32(buf) {
  let c;
  if (!crc32.table) {
    crc32.table = new Uint32Array(256);
    for (let n = 0; n < 256; n++) {
      c = n;
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
      crc32.table[n] = c >>> 0;
    }
  }
  c = 0xffffffff;
  for (let i = 0; i < buf.length; i++)
    c = (crc32.table[(c ^ buf[i]) & 0xff] ^ (c >>> 8)) >>> 0;
  return (c ^ 0xffffffff) >>> 0;
}

function writeIco(pngBuffer, size, outPath) {
  // Single-image ICO wrapping a PNG (Vista+).
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2); // type: icon
  header.writeUInt16LE(1, 4); // 1 image

  const entry = Buffer.alloc(16);
  entry.writeUInt8(size >= 256 ? 0 : size, 0); // width
  entry.writeUInt8(size >= 256 ? 0 : size, 1); // height
  entry.writeUInt8(0, 2); // no palette
  entry.writeUInt8(0, 3); // reserved
  entry.writeUInt16LE(1, 4); // color planes
  entry.writeUInt16LE(32, 6); // bpp
  entry.writeUInt32LE(pngBuffer.length, 8);
  entry.writeUInt32LE(6 + 16, 12);

  fs.writeFileSync(outPath, Buffer.concat([header, entry, pngBuffer]));
}

function main() {
  fs.mkdirSync(OUT_DIR, { recursive: true });

  const targets = [
    { name: "32x32.png", size: 32 },
    { name: "128x128.png", size: 128 },
    { name: "128x128@2x.png", size: 256 },
    { name: "icon.png", size: 512 },
  ];

  for (const t of targets) {
    const png = makePngBuffer(t.size);
    fs.writeFileSync(path.join(OUT_DIR, t.name), png);
  }

  // Windows .ico using the 256x256 PNG.
  const icoPng = makePngBuffer(256);
  writeIco(icoPng, 256, path.join(OUT_DIR, "icon.ico"));

  // Minimal macOS icns placeholder — Tauri on Windows doesn't need it for
  // bundling, but the conf references it. Write a 256x256 PNG masquerading
  // as an icns so the file exists; replace later for real macOS packaging.
  fs.writeFileSync(path.join(OUT_DIR, "icon.icns"), icoPng);

  console.log("Generated icons in", OUT_DIR);
}

main();

// 生成 vineSync 应用图标（PNG + ICO），无需第三方依赖
// 用法: node tools/gen-icons.mjs
import { deflateSync } from "node:zlib";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const outDir = join(root, "src-tauri", "icons");

// ---------- CRC32 ----------
const crcTable = new Uint32Array(256);
for (let n = 0; n < 256; n++) {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  crcTable[n] = c >>> 0;
}
function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) c = crcTable[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

// ---------- PNG ----------
function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const t = Buffer.from(type, "ascii");
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([t, data])));
  return Buffer.concat([len, t, data, crc]);
}
function encodePNG(width, height, rgba) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // color type RGBA
  const raw = Buffer.alloc((width * 4 + 1) * height);
  for (let y = 0; y < height; y++) {
    raw[y * (width * 4 + 1)] = 0; // filter none
    rgba.copy(raw, y * (width * 4 + 1) + 1, y * width * 4, (y + 1) * width * 4);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

// ---------- 绘制 ----------
function render(size) {
  const rgba = Buffer.alloc(size * size * 4);
  const S = size / 512; // 设计基准 512
  const cx = 256 * S,
    cy = 256 * S;

  // 圆角矩形背景（渐变）
  const r0 = 40 * S,
    left = 24 * S,
    right = 488 * S,
    top = 24 * S,
    bottom = 488 * S,
    radius = 108 * S;
  // 环形箭头参数
  const ringR = 142 * S,
    halfW = 26 * S,
    a1s = (12 * Math.PI) / 180,
    a1e = (168 * Math.PI) / 180,
    a2s = (192 * Math.PI) / 180,
    a2e = (348 * Math.PI) / 180;

  const inRoundRect = (x, y) => {
    if (x < left || x > right || y < top || y > bottom) return false;
    const dx = Math.max(left + radius - x, 0, x - (right - radius));
    const dy = Math.max(top + radius - y, 0, y - (bottom - radius));
    return dx * dx + dy * dy <= radius * radius;
  };
  // 点到圆弧的距离（含角度范围）
  const onArc = (x, y, a0, a1) => {
    const dx = x - cx,
      dy = y - cy;
    const d = Math.sqrt(dx * dx + dy * dy);
    if (Math.abs(d - ringR) > halfW + 2 * S) return false;
    let ang = Math.atan2(dy, dx);
    if (ang < 0) ang += Math.PI * 2;
    const norm = (v) => {
      let a = v;
      while (a < 0) a += Math.PI * 2;
      while (a >= Math.PI * 2) a -= Math.PI * 2;
      return a;
    };
    const span = norm(a1 - a0) || Math.PI * 2;
    return norm(ang - a0) <= span;
  };
  // 箭头三角形
  const inArrow = (x, y, aEnd) => {
    const cos = Math.cos(aEnd),
      sin = Math.sin(aEnd);
    const ex = cx + ringR * cos,
      ey = cy + ringR * sin;
    // 切向（角度增大方向）与径向
    const tx = -sin,
      ty = cos;
    const px = cos,
      py = sin;
    const tipX = ex + tx * 52 * S,
      tipY = ey + ty * 52 * S;
    const b1x = ex - tx * 14 * S + px * 44 * S,
      b1y = ey - ty * 14 * S + py * 44 * S;
    const b2x = ex - tx * 14 * S - px * 44 * S,
      b2y = ey - ty * 14 * S - py * 44 * S;
    // 点在三角形内（重心法）
    const area = (x1, y1, x2, y2, x3, y3) => Math.abs((x2 - x1) * (y3 - y1) - (x3 - x1) * (y2 - y1));
    const a = area(tipX, tipY, b1x, b1y, b2x, b2y);
    if (a === 0) return false;
    const a1 = area(x, y, b1x, b1y, b2x, b2y);
    const a2 = area(tipX, tipY, x, y, b2x, b2y);
    const a3 = area(tipX, tipY, b1x, b1y, x, y);
    return Math.abs(a1 + a2 + a3 - a) < 0.6;
  };

  const gradT = [0x35, 0xd8, 0x94];
  const gradB = [0x14, 0x8f, 0x62];

  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const i = (y * size + x) * 4;
      const fx = x + 0.5,
        fy = y + 0.5;
      let r = 0,
        g = 0,
        b = 0,
        a = 0;
      if (inRoundRect(fx, fy)) {
        const t = (fy - top) / (bottom - top);
        r = gradT[0] + (gradB[0] - gradT[0]) * t;
        g = gradT[1] + (gradB[1] - gradT[1]) * t;
        b = gradT[2] + (gradB[2] - gradT[2]) * t;
        a = 255;
        const white =
          onArc(fx, fy, a1s, a1e) ||
          onArc(fx, fy, a2s, a2e) ||
          inArrow(fx, fy, a1e) ||
          inArrow(fx, fy, a2e);
        if (white) {
          r = 255;
          g = 255;
          b = 255;
        }
      }
      rgba[i] = Math.round(r);
      rgba[i + 1] = Math.round(g);
      rgba[i + 2] = Math.round(b);
      rgba[i + 3] = a;
    }
  }
  return rgba;
}

function encodeICO(pngBySize) {
  const entries = [...pngBySize.entries()].sort((a, b) => a[0] - b[0]);
  const header = Buffer.alloc(6 + entries.length * 16);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(entries.length, 4);
  let offset = 6 + entries.length * 16;
  const blobs = [];
  entries.forEach(([size, png], idx) => {
    const o = 6 + idx * 16;
    header[o] = size >= 256 ? 0 : size;
    header[o + 1] = size >= 256 ? 0 : size;
    header[o + 2] = 0;
    header[o + 3] = 0;
    header.writeUInt16LE(1, o + 4);
    header.writeUInt16LE(32, o + 6);
    header.writeUInt32LE(png.length, o + 8);
    header.writeUInt32LE(offset, o + 12);
    offset += png.length;
    blobs.push(png);
  });
  return Buffer.concat([header, ...blobs]);
}

mkdirSync(outDir, { recursive: true });
const files = [
  [16, "16x16.png"],
  [32, "32x32.png"],
  [48, "48x48.png"],
  [128, "128x128.png"],
  [256, "128x128@2x.png"],
  [512, "icon.png"],
];
const pngs = new Map();
for (const [size, name] of files) {
  const png = encodePNG(size, size, render(size));
  writeFileSync(join(outDir, name), png);
  pngs.set(size, png);
  console.log("generated", name);
}
writeFileSync(join(outDir, "icon.ico"), encodeICO(new Map([[16, pngs.get(16)], [32, pngs.get(32)], [48, pngs.get(48)], [256, pngs.get(256)]])));
console.log("generated icon.ico");

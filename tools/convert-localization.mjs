#!/usr/bin/env node
// 一次性转换器: origin/src/localization/*.json -> src/localization/*.ftl
// 规范: AGENTS.md §2 (JSON → FTL)
// 规则:
//   - 一条消息 = KEY = VALUE (首个 " = " 切分)
//   - 占位符 {{N}} 原样保留 (等价 Translate.js makeTranslation 语义)
//   - 禁用 FTL 高级特性; 每键单行输出
//   - 文件名映射: zh_Hans.json -> zh-Hans.ftl, 其余 <lang>.json -> <lang>.ftl
//   - --check 模式: 双向 diff 校验 (键零丢失 / 值逐字节相等 / 占位符数量一致)

import { readFileSync, writeFileSync, readdirSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const SRC = join(ROOT, "origin", "src", "localization");
const DST = join(ROOT, "src", "localization");

function ftlFileName(jsonName) {
  if (!jsonName.endsWith(".json")) return null;
  return jsonName.replace(/\.json$/, "").replace("_", "-") + ".ftl";
}

// 单行输出安全前提(2026-11-22 全语料扫描确认): 值不含 \n \r 制表符,
// 无首尾空格, 无控制字符。若扫描前提被破坏, 此处 fail fast 而非静默破坏。
function assertSafeValue(key, value) {
  if (/[\r\n\t]/.test(value)) throw new Error(`${key}: 含换行/制表符, 单行输出不安全`);
  if (value !== value.trim()) throw new Error(`${key}: 首尾空格将被 FTL trim 破坏`);
  for (const ch of value) {
    const c = ch.codePointAt(0);
    if (c < 32) throw new Error(`${key}: 含控制字符 U+${c.toString(16)}`);
  }
}

function placeholders(s) {
  return [...s.matchAll(/\{\{(\d+)\}\}/g)].map((m) => m[1]).sort().join(",");
}

function convert(jsonText, langFile) {
  const data = JSON.parse(jsonText);
  const entries = Object.entries(data);
  const lines = [];
  lines.push(`# Generated from origin/src/localization/${langFile} — do not edit.`);
  lines.push(`# Placeholders {{N}} are literal (Translate.js makeTranslation semantics).`);
  for (const [key, value] of entries) {
    if (typeof value !== "string") throw new Error(`${key}: 值非字符串`);
    if (!/^[A-Z0-9_]+$/.test(key)) throw new Error(`${key}: 键名非 [A-Z0-9_]`);
    assertSafeValue(key, value);
    lines.push(`${key} = ${value}`);
  }
  return { text: lines.join("\n") + "\n", count: entries.length, data };
}

function verify(ftlText, data) {
  const parsed = parseFtl(ftlText);
  const errors = [];
  const ftlKeys = [...parsed.keys()];
  const jsonKeys = Object.keys(data);
  for (const k of jsonKeys) {
    if (!parsed.has(k)) { errors.push(`丢失键: ${k}`); continue; }
    const ftlVal = parsed.get(k);
    if (ftlVal !== data[k]) errors.push(`值不等: ${k}`);
    if (placeholders(ftlVal) !== placeholders(data[k])) errors.push(`占位符不等: ${k}`);
  }
  const extra = ftlKeys.filter((k) => !(k in data));
  if (extra.length) errors.push(`多余键: ${extra.join(",")}`);
  return errors;
}

// 运行时同款解析逻辑: 按 " = " 首次出现切分, 忽略注释/空行
function parseFtl(text) {
  const map = new Map();
  for (const line of text.split("\n")) {
    if (!line || line.startsWith("#")) continue;
    const idx = line.indexOf(" = ");
    if (idx <= 0) continue;
    map.set(line.slice(0, idx), line.slice(idx + 3));
  }
  return map;
}

const check = process.argv.includes("--check");
mkdirSync(DST, { recursive: true });
const files = readdirSync(SRC).filter((f) => f.endsWith(".json")).sort();
let failed = 0;

for (const f of files) {
  const outName = ftlFileName(f);
  const jsonText = readFileSync(join(SRC, f), "utf8");
  const { text, count, data } = convert(jsonText, f);
  if (check) {
    const errors = verify(text, data);
    if (errors.length) {
      failed++;
      console.error(`[FAIL] ${f} -> ${outName}: ${errors.join("; ")}`);
    } else {
      console.log(`[OK]   ${f} -> ${outName} (${count} keys)`);
    }
  } else {
    writeFileSync(join(DST, outName), text, "utf8");
    console.log(`[GEN]  ${f} -> ${outName} (${count} keys)`);
  }
}

console.log(check ? (failed ? `CHECK FAILED: ${failed} file(s)` : "CHECK PASSED") : `Generated ${files.length} files`);
process.exit(check && failed ? 1 : 0);

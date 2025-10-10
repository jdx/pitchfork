#!/usr/bin/env bun
import { existsSync, readFileSync, writeFileSync, unlinkSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

const key = process.env.TEST_SUCCESS_ON_THIRD_TIMESTAMP;

if (!key) {
  console.error("Missing environment variable: TEST_SUCCESS_ON_THIRD_TIMESTAMP");
  process.exit(2);
}

const COUNT_FILE = join(tmpdir(), `retry_count_${key}`);

if (!existsSync(COUNT_FILE)) {
  writeFileSync(COUNT_FILE, "0");
}

let count = parseInt(readFileSync(COUNT_FILE, "utf8") || "0", 10);
count += 1;
writeFileSync(COUNT_FILE, count.toString());

console.log(`Attempt ${count} (key=${key})`);

if (count < 3) {
  process.exit(1);
} else {
  console.log("Success!");
  unlinkSync(COUNT_FILE);
  process.exit(0);
}

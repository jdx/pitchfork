// Usage: bun run tests/fail.ts [secs]

(async () => {
  const secs = parseInt(process.argv[2] ?? "0", 10);
  setTimeout(async () => {
    console.log(`Failed after ${secs}!`);
    process.exit(1);
  }, secs * 1000);
})();

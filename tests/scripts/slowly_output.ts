// Usage: bun run tests/slowly_output.ts [interval_secs] [total_outputs]

(async () => {
  let i = 0;
  const interval = parseInt(process.argv[2] ?? "1", 10);
  const total = parseInt(process.argv[3] ?? "5", 10);
  const id = setInterval(async () => {
    if (++i > total) {
      clearInterval(id);
      process.exit(0);
    } else {
      console.log(`Output ${i}/${total}`);
    }
  }, interval * 1000);
})();

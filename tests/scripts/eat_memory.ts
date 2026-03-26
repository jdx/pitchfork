// Usage: bun run tests/scripts/eat_memory.ts [MB]
// Allocates the specified amount of memory (default 64MB) and holds it.

const targetMB = parseInt(process.argv[2] ?? "64", 10);
const buffers: Buffer[] = [];

// Allocate in 1MB chunks
for (let i = 0; i < targetMB; i++) {
  const buf = Buffer.alloc(1024 * 1024); // 1MB
  // Touch every page to ensure RSS allocation (not just virtual)
  buf.fill(i & 0xff);
  buffers.push(buf);
}

console.log(`Allocated ${targetMB}MB of memory`);

// Hold the memory and stay alive
setInterval(() => {
  // Touch buffers to prevent GC
  for (const buf of buffers) {
    buf[0] = (buf[0] + 1) & 0xff;
  }
}, 1000);

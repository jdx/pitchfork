// Simple HTTP server that delays start by specified seconds
// Usage: bun run http_server.ts <delay_seconds> <port>

const args = process.argv.slice(2);
const delaySeconds = parseInt(args[0] || "1", 10);
const port = parseInt(args[1] || "18080", 10);

console.log(`Waiting ${delaySeconds}s before starting server...`);

setTimeout(async () => {
  console.log(`Starting HTTP server on port ${port}...`);

  const server = Bun.serve({
    port,
    fetch(req) {
      const url = new URL(req.url);
      if (url.pathname === "/health") {
        return new Response("OK", { status: 200 });
      }
      return new Response("Not Found", { status: 404 });
    },
  });

  console.log(`Server listening on http://localhost:${port}`);
  console.log("Health check available at /health");
}, delaySeconds * 1000);

// Keep the process running
setInterval(() => {}, 1000);

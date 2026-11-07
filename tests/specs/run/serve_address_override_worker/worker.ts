Deno.serve({
  hostname: "127.0.0.1",
  port: 0, // must not be overridden to 9003
  onListen({ port }) {
    self.postMessage(port);
  },
}, () => new Response("worker"));

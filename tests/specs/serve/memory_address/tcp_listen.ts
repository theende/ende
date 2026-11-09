const server = Ende.serve({
  hostname: "127.0.0.1",
  port: 0,
  onListen(addr) {
    console.log(addr.transport);
  },
}, () => new Response("ok"));
await server.shutdown();

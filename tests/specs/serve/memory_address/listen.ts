const server = Ende.serve({
  onListen(addr) {
    console.log(JSON.stringify(addr));
  },
}, () => new Response("ok"));
console.log(JSON.stringify(server.addr));
await server.shutdown();
console.log("shut down");

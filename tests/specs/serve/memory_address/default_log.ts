const server = Ende.serve(() => new Response("ok"));
await server.shutdown();

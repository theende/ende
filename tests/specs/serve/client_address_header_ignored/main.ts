// The Deno Deploy proxy header x-deno-client-address is an ordinary request
// header in Ende: it never overrides the remote address reported to the
// handler, even with the removed ENDE_TRUST_PROXY_HEADERS=1 set.
const server = Deno.serve(
  { port: 0, onListen: () => {} },
  (req, info) => {
    console.log("remoteAddr:", info.remoteAddr.hostname);
    console.log(
      "header visible:",
      req.headers.has("x-deno-client-address"),
    );
    return new Response("ok");
  },
);

const res = await fetch(`http://127.0.0.1:${server.addr.port}/`, {
  headers: { "x-deno-client-address": "10.1.2.3:4567" },
});
await res.body?.cancel();
console.log(
  "environment after first request:",
  Deno.env.get("ENDE_TRUST_PROXY_HEADERS"),
);

// A request with a body exercises the streaming/prebuffered raw record
// construction paths as well.
const res2 = await fetch(`http://127.0.0.1:${server.addr.port}/`, {
  method: "POST",
  body: "hello",
  headers: { "x-deno-client-address": "10.1.2.3:4567" },
});
await res2.body?.cancel();

await server.shutdown();

// The first Ende.serve takes ENDE_SERVE_ADDRESS=memory:app.example. A second
// one listens on TCP, and its requests may not name the memory host in
// `request.url`.
const memory = Ende.serve({ onListen() {} }, () => new Response("memory"));

const seen: string[] = [];
const server = Ende.serve({
  hostname: "127.0.0.1",
  port: 0,
  onListen() {},
}, (req) => {
  seen.push(req.url);
  return new Response("handled");
});

async function send(target: string, host: string): Promise<string> {
  const conn = await Ende.connect({
    hostname: "127.0.0.1",
    port: server.addr.port,
  });
  await conn.write(
    new TextEncoder().encode(
      `GET ${target} HTTP/1.1\r\nhost: ${host}\r\nconnection: close\r\n\r\n`,
    ),
  );
  let text = "";
  const buf = new Uint8Array(4096);
  while (true) {
    const n = await conn.read(buf);
    if (n === null) break;
    text += new TextDecoder().decode(buf.subarray(0, n));
  }
  conn.close();
  return text.split("\r\n")[0];
}

for (
  const [target, host] of [
    ["http://app.example/x", "other.example"],
    ["HTTP://APP.EXAMPLE:8000/x", "other.example"],
    ["/x", "app.example"],
    ["/x", "APP.Example:8000"],
    ["/x", "user@app.example"],
    ["/x", "other.example"],
    ["http://other.example/x", "app.example"],
  ]
) {
  console.log(`${target} (host: ${host}) ->`, await send(target, host));
}
console.log("handler saw:", JSON.stringify(seen));
await server.shutdown();
await memory.shutdown();

// Test that the CDN_LOOP env var is injected as a header on outbound fetch
// requests. X_DENO_FETCH_TOKEN was a Deno Deploy mechanism: Ende neither
// injects it nor touches a user-provided x-deno-fetch-token header.

const server = Deno.serve({ port: 0 }, (req: Request) => {
  return Response.json({
    "x-deno-fetch-token": req.headers.get("x-deno-fetch-token"),
    "cdn-loop": req.headers.get("cdn-loop"),
  });
});

const addr = server.addr;
const url = `http://localhost:${addr.port}/`;

// Test 1: Headers are injected from env vars
{
  const resp = await fetch(url);
  const headers = await resp.json();
  console.log("fetch token not set:", headers["x-deno-fetch-token"] === null);
  console.log(
    "cdn-loop set:",
    headers["cdn-loop"] === "us-east-1.example.deno-cluster.net",
  );
}

// Test 2: User-provided x-deno-fetch-token is sent as-is
{
  const resp = await fetch(url, {
    headers: { "x-deno-fetch-token": "user-spoofed-value" },
  });
  const headers = await resp.json();
  console.log(
    "user token kept:",
    headers["x-deno-fetch-token"] === "user-spoofed-value",
  );
}

await server.shutdown();

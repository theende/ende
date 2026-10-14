import { propagation } from "./otel_api.ts";

Deno.serve({
  onListen() {
    Deno.writeTextFileSync(Deno.args[0], "started");
  },
}, async () => {
  console.log("server 1");
  console.log(propagation.getActiveBaggage()?.getEntry("userId")?.value);
  await fetch("http://localhost:8001");
  setTimeout(() => Deno.exit(0), 1000);
  return new Response("Hello World!");
});

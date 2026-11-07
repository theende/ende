// ENDE_SERVE_ADDRESS is for the program's own server. A server started in a
// web Worker keeps the address it asked for.
const server = Deno.serve({
  port: 9004, // overridden to 9003 by ENDE_SERVE_ADDRESS
  onListen({ hostname, port }) {
    console.log(`Main listening on ${hostname}:${port}`);
  },
}, () => new Response("main"));

const worker = new Worker(import.meta.resolve("./worker.ts"), {
  type: "module",
});
const workerPort = await new Promise<number>((resolve, reject) => {
  worker.onmessage = (e) => resolve(e.data);
  worker.onerror = (e) => {
    e.preventDefault();
    reject(new Error(e.message));
  };
});
console.log(`Worker port overridden: ${workerPort === 9003}`);

const mainRes = await fetch("http://127.0.0.1:9003/");
console.log(`Main response: ${await mainRes.text()}`);
const workerRes = await fetch(`http://127.0.0.1:${workerPort}/`);
console.log(`Worker response: ${await workerRes.text()}`);

worker.terminate();
await server.shutdown();

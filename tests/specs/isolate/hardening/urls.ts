import { show } from "./show.ts";

// Every form of a sandboxed URL is still sandboxed: its functions are
// called async.
for (
  const url of [
    "https://localhost:5545/subdir/print_hello.ts",
    "https://localhost:5545/subdir/print_hello.ts?x=1",
    "https://localhost:5545/subdir/print_hello.ts#x",
    "https://localhost:5545/run/extension_lockfile/remote_ext.ts",
    "https://localhost:5545/run//extension_lockfile/remote_ext.ts",
  ]
) {
  await show(url, async () => {
    const module = await import(url);
    const result = (module.printHello ?? module.answer)();
    const isPromise = result instanceof Promise;
    await result;
    return isPromise;
  });
}

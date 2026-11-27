import { read } from "./vendor/probe/mod.ts";

Ende.test("the sandbox reads with the app's permission", async () => {
  if ((await read("secret.txt")).trim() !== "secret") throw new Error("read");
});

Ende.test({
  name: "a test that narrows the app narrows the sandbox",
  permissions: { read: false },
  async fn() {
    try {
      await read("secret.txt");
    } catch (e) {
      if (e instanceof Ende.errors.NotCapable) return;
    }
    throw new Error("the sandbox could still read");
  },
});

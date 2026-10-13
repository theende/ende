new Worker(import.meta.resolve("./worker.ts"), {
  type: "module",
  ende: {
    permissions: {
      import: ["127.0.0.1:4250"],
    },
  },
});

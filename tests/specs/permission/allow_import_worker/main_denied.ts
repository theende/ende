new Worker(import.meta.resolve("./worker.ts"), {
  type: "module",
  ende: {
    permissions: {
      import: [],
    },
  },
});

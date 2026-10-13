for (let i = 0; i < 2; i++) {
  const result = await Ende.bundle({
    entrypoints: ["./main.ts"],
    platform: "deno",
  });
  console.log(result.success);
}

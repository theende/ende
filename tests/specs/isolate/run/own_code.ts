import { importWritten } from "./vendor/writer/mod.ts";

console.log(await importWritten("own.ts", "export const own = 1;"));
console.log(
  await importWritten("leak.ts", "export * from '../../secret_module.ts';"),
);

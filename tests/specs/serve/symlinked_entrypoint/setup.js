Ende.mkdirSync("bin", { recursive: true });
Ende.mkdirSync("src/lib", { recursive: true });

Ende.writeTextFileSync(
  "src/main.ts",
  `import { x } from "./lib/x.ts";

export default {
  onListen() {
    console.log(x);
    Ende.exit(0);
  },
  fetch() {
    return new Response(x);
  },
};
`,
);
Ende.writeTextFileSync("src/lib/x.ts", "export const x = 'ok';\n");
Ende.symlinkSync("../src/main.ts", "bin/main.ts");

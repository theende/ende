Ende.mkdirSync("bin", { recursive: true });
Ende.mkdirSync("src/lib", { recursive: true });

Ende.writeTextFileSync(
  "src/tool.js",
  "import { x } from './lib/x.js';\nconsole.log(x);\n",
);
Ende.writeTextFileSync("src/lib/x.js", "export const x = 'ok';\n");
Ende.symlinkSync("../src/tool.js", "bin/tool");

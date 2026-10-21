// Decodes out.js.map and checks that `then` (which follows a specifier that
// got shorter when rewritten) still maps back to its original position.
const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

function decode(mappings: string): number[][][] {
  let srcLine = 0;
  let srcCol = 0;
  return mappings.split(";").map((line) => {
    let genCol = 0;
    const segments = [];
    for (const segment of line.split(",").filter((s) => s.length > 0)) {
      const values = [];
      let value = 0;
      let shift = 0;
      for (const ch of segment) {
        const digit = B64.indexOf(ch);
        value += (digit & 31) << shift;
        if (digit & 32) {
          shift += 5;
        } else {
          values.push(value & 1 ? -(value >> 1) : value >> 1);
          value = 0;
          shift = 0;
        }
      }
      genCol += values[0];
      if (values.length >= 4) {
        srcLine += values[2];
        srcCol += values[3];
        segments.push([genCol, srcLine, srcCol]);
      }
    }
    return segments;
  });
}

function position(text: string, needle: string): [number, number] {
  const lines = text.split("\n");
  const line = lines.findIndex((l) => l.includes(needle));
  return [line, lines[line].indexOf(needle)];
}

const out = Deno.readTextFileSync("out.js");
const map = JSON.parse(Deno.readTextFileSync("out.js.map"));
const source = Deno.readTextFileSync("rewrite/sourcemap.ts");

const [genLine, genCol] = position(out, "then");
const segment = decode(map.mappings)[genLine]
  .findLast(([col]) => col <= genCol)!;
console.log(out.split("\n")[genLine]);
console.log(segment[0] === genCol);
console.log(
  JSON.stringify([segment[1], segment[2]]) ===
    JSON.stringify(position(source, "then")),
);

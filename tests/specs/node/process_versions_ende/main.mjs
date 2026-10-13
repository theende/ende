import process from "node:process";

console.log(typeof process.versions.ende);
console.log(process.versions.ende === Ende.version.ende);
console.log(process.versions.deno);
console.log("deno" in process.versions);

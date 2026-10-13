const filePath = import.meta.dirname + "/ende.lock";
console.log(Deno.readTextFileSync(filePath).trim());

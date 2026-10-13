const data = JSON.parse(Deno.readTextFileSync("./add/ende.json"));
data.version = "2.0.0";
Deno.writeTextFileSync("./add/ende.json", JSON.stringify(data, null, 2));

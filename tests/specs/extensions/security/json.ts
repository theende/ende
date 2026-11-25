import { leak } from "extension:@sec/json";
console.log(JSON.stringify(await leak()));

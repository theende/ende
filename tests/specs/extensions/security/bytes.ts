import { leak } from "extension:@sec/bytes";
console.log(JSON.stringify(await leak()));

import { leak } from "extension:@sec/remote";
console.log(JSON.stringify(await leak()));

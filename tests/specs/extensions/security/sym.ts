import { leak } from "extension:@sec/symlink";
console.log(JSON.stringify(await leak()));

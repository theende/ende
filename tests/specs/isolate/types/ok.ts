import { shout, VERSION } from "./vendor/pkg/mod.ts";
import type { Options, Widget } from "./vendor/pkg/mod.ts";

const options: Options = { loud: true };
const text: string = await shout("hi");
const version: string = VERSION;
const widget: Widget | undefined = undefined;
console.log(text, version, options.loud, widget);

import { shout, Widget } from "./vendor/pkg/mod.ts";

// Used only as a type: TypeScript drops it from the import, so it doesn't
// cross the sandbox, and running accepts it.
const widget: Widget | undefined = undefined;
console.log(widget, await shout("hi"));

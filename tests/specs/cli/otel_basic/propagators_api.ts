import { context, propagation } from "./otel_api.ts";

const carrier = new Map<string, string>([
  ["baggage", "userId=alice"],
]);
const ctx = propagation.extract(context.active(), carrier, {
  get(carrier, key) {
    return carrier.get(key);
  },
  keys(carrier) {
    return Array.from(carrier.keys());
  },
});

console.log(propagation.getBaggage(ctx)?.getEntry("userId")?.value);

const newCarrier: Map<string, string> = new Map();
propagation.inject(ctx, newCarrier, {
  set(carrier, key, value) {
    carrier.set(key, value);
  },
});

console.log(newCarrier.get("baggage"));

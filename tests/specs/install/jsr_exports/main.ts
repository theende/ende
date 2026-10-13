import { add } from "@endetest/multiple-exports/add";
import { subtract } from "@endetest/multiple-exports/subtract";
import data from "@endetest/multiple-exports/data-json" with { type: "json" };

console.log(add(1, 2));
console.log(subtract(1, 2));
console.log(data);
